//! Environment readiness checker for Tool Ready feature.
//!
//! Loads per-tool dependency declarations from tool-ready-spec.json.
//! Supports both string format ("jq") and object format ({binary, version, package, manager, ...}).
//! Checks binary availability (with version constraints), config files,
//! permissions, and network connectivity. Generates a structured
//! ready checklist and supports auto-fix via config-driven install engine.

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A single dependency entry — normalized from either string or object format.
#[derive(Debug, Clone)]
struct DepEntry {
    binary: String,
    version: Option<String>,
    package: String,
    manager: String,
    pip_name: Option<String>,
    uv_name: Option<String>,
    npm_name: Option<String>,
    use_npx: bool,
    fallback: Vec<FallbackEntry>,
}

/// A fallback install strategy.
#[derive(Debug, Clone)]
struct FallbackEntry {
    method: String,
    package: Option<String>,
    binary: Option<String>,
    source: Option<String>,
    manifest: Option<String>,
    features: Option<String>,
}

/// Per-tool dependency specification.
#[derive(Debug, Clone)]
struct ToolDepSpec {
    required: Vec<DepEntry>,
    recommended: Vec<DepEntry>,
    config_files: Vec<String>,
    permissions: Vec<String>,
    network: Vec<String>,
}

/// Result of checking a single dependency item.
#[derive(Debug, Clone, PartialEq)]
enum DepStatus {
    Available,
    Missing,
    VersionLow { installed: String, required: String },
}

/// Overall readiness status for a tool.
#[derive(Debug, Clone, PartialEq)]
enum ReadyStatus {
    Ready,
    Partial,
    NotReady,
}

/// Combined result for a tool's environment check.
struct ToolReadyResult {
    tool_name: String,
    status: ReadyStatus,
    required_results: Vec<(DepEntry, DepStatus)>,
    recommended_results: Vec<(DepEntry, DepStatus)>,
    config_results: Vec<(String, bool)>,
    permission_results: Vec<(String, bool)>,
    network_results: Vec<(String, bool)>,
}

/// Normalize a JSON value (string or object) into a DepEntry.
/// String "jq" → DepEntry { binary: "jq", package: "jq", manager: "apt" }
/// Object {binary, version, package, manager, ...} → DepEntry
fn normalize_dep(value: &Value) -> DepEntry {
    match value {
        Value::String(s) => {
            // Handle version constraints: "rtk>=0.35"
            if let Some(idx) = s.find(">=") {
                let binary = s[..idx].to_string();
                let version = Some(s[idx..].to_string());
                DepEntry {
                    binary,
                    version,
                    package: s[..idx].to_string(),
                    manager: "apt".to_string(),
                    pip_name: None,
                    uv_name: None,
                    npm_name: None,
                    use_npx: false,
                    fallback: Vec::new(),
                }
            } else {
                DepEntry {
                    binary: s.clone(),
                    version: None,
                    package: s.clone(),
                    manager: "apt".to_string(),
                    pip_name: None,
                    uv_name: None,
                    npm_name: None,
                    use_npx: false,
                    fallback: Vec::new(),
                }
            }
        }
        Value::Object(obj) => {
            let binary = obj
                .get("binary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let version = obj
                .get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let package = obj
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or(&binary)
                .to_string();
            let manager = obj
                .get("manager")
                .and_then(|v| v.as_str())
                .unwrap_or("apt")
                .to_string();
            let pip_name = obj
                .get("pip_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let uv_name = obj
                .get("uv_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let npm_name = obj
                .get("npm_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let use_npx = obj
                .get("use_npx")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let fallback = obj
                .get("fallback")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|fb| {
                            if let Value::Object(fb_obj) = fb {
                                Some(FallbackEntry {
                                    method: fb_obj
                                        .get("method")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    package: fb_obj
                                        .get("package")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    binary: fb_obj
                                        .get("binary")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    source: fb_obj
                                        .get("source")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    manifest: fb_obj
                                        .get("manifest")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    features: fb_obj
                                        .get("features")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            DepEntry {
                binary,
                version,
                package,
                manager,
                pip_name,
                uv_name,
                npm_name,
                use_npx,
                fallback,
            }
        }
        _ => DepEntry {
            binary: "".to_string(),
            version: None,
            package: "".to_string(),
            manager: "apt".to_string(),
            pip_name: None,
            uv_name: None,
            npm_name: None,
            use_npx: false,
            fallback: Vec::new(),
        },
    }
}

/// Normalize an array of dep values (strings or objects) into Vec<DepEntry>.
fn normalize_deps(array: &Value) -> Vec<DepEntry> {
    array
        .as_array()
        .map(|arr| arr.iter().map(normalize_dep).collect())
        .unwrap_or_default()
}

/// Parse version constraint string ">=0.35" into (operator, version).
fn parse_version_constraint(version: &str) -> (&str, &str) {
    if version.starts_with(">=") {
        (">= ", version.strip_prefix(">=").unwrap_or("0.0.0"))
    } else if version.starts_with(">") {
        ("> ", version.strip_prefix(">").unwrap_or("0.0.0"))
    } else {
        ("any", version)
    }
}

/// Compare version strings (semver-like: major.minor.patch).
fn version_ge(installed: &str, required: &str) -> bool {
    let i_parts: Vec<u32> = installed
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    let r_parts: Vec<u32> = required
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    for i in 0..3 {
        let iv = i_parts.get(i).copied().unwrap_or(0);
        let rv = r_parts.get(i).copied().unwrap_or(0);
        if iv > rv {
            return true;
        }
        if iv < rv {
            return false;
        }
    }
    true
}

/// Check if a binary is available and meets version constraints.
fn check_dep(dep: &DepEntry) -> DepStatus {
    let which_result = Command::new("which")
        .arg(&dep.binary)
        .output();

    match which_result {
        Ok(output) if output.status.success() => {
            if let Some(ref version) = dep.version {
                let (_op, required_version) = parse_version_constraint(version);
                let version_output = Command::new(&dep.binary)
                    .arg("--version")
                    .output();
                let installed_version = match version_output {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        stdout
                            .lines()
                            .next()
                            .unwrap_or("")
                            .split_whitespace()
                            .last()
                            .unwrap_or("0.0.0")
                            .to_string()
                    }
                    Err(_) => "0.0.0".to_string(),
                };

                if version_ge(&installed_version, required_version) {
                    DepStatus::Available
                } else {
                    DepStatus::VersionLow {
                        installed: installed_version,
                        required: required_version.to_string(),
                    }
                }
            } else {
                DepStatus::Available
            }
        }
        _ => DepStatus::Missing,
    }
}

/// Expand ~ in paths to HOME directory.
fn expand_path(path: &str) -> String {
    if path.starts_with("~") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        path.replacen("~", &home, 1)
    } else {
        path.to_string()
    }
}

/// Check if a config file exists.
fn check_config_file(path: &str) -> bool {
    let expanded = expand_path(path);
    fs::metadata(&expanded).is_ok()
}

/// Check a permission type.
fn check_permission(perm: &str) -> bool {
    match perm {
        "file_read" => fs::metadata("/").is_ok(),
        "file_write" => {
            let test_path = format!(
                "{}/.tokenless-ready-test",
                std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
            );
            let can_write = fs::write(&test_path, "").is_ok();
            if can_write {
                let _ = fs::remove_file(&test_path);
            }
            can_write
        }
        "exec_shell" => Command::new("which")
            .arg("bash")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        _ => true,
    }
}

/// Check network connectivity.
fn check_network(net: &str) -> bool {
    match net {
        "https_outbound" => Command::new("curl")
            .args(["-s", "--max-time", "2", "https://example.com"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        _ => true,
    }
}

/// Load tool-ready-spec.json with both string and object format support.
fn load_spec(spec_path: &PathBuf) -> Result<std::collections::HashMap<String, ToolDepSpec>, String> {
    let content = fs::read_to_string(spec_path)
        .map_err(|e| format!("Failed to read spec file: {}", e))?;
    let value: Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse spec JSON: {}", e))?;

    let mut specs = std::collections::HashMap::new();
    // Skip _comment key
    if let Value::Object(obj) = value {
        for (tool_name, tool_spec) in obj {
            if tool_name.starts_with('_') {
                continue;
            }
            if let Value::Object(spec_obj) = tool_spec {
                let required = normalize_deps(spec_obj.get("required").unwrap_or(&Value::Array(Vec::new())));
                let recommended = normalize_deps(spec_obj.get("recommended").unwrap_or(&Value::Array(Vec::new())));
                let config_files = spec_obj
                    .get("config_files")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let permissions = spec_obj
                    .get("permissions")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let network = spec_obj
                    .get("network")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                specs.insert(
                    tool_name,
                    ToolDepSpec {
                        required,
                        recommended,
                        config_files,
                        permissions,
                        network,
                    },
                );
            }
        }
    }
    Ok(specs)
}

/// Check a specific tool's environment readiness.
fn check_tool(tool_name: &str, spec: &ToolDepSpec) -> ToolReadyResult {
    let required_results: Vec<(DepEntry, DepStatus)> = spec
        .required
        .iter()
        .map(|d| (d.clone(), check_dep(d)))
        .collect();

    let recommended_results: Vec<(DepEntry, DepStatus)> = spec
        .recommended
        .iter()
        .map(|d| (d.clone(), check_dep(d)))
        .collect();

    let config_results: Vec<(String, bool)> = spec
        .config_files
        .iter()
        .map(|f| (f.clone(), check_config_file(f)))
        .collect();

    let permission_results: Vec<(String, bool)> = spec
        .permissions
        .iter()
        .map(|p| (p.clone(), check_permission(p)))
        .collect();

    let network_results: Vec<(String, bool)> = spec
        .network
        .iter()
        .map(|n| (n.clone(), check_network(n)))
        .collect();

    let has_required_missing = required_results
        .iter()
        .any(|(_, s)| s == &DepStatus::Missing || matches!(s, DepStatus::VersionLow { .. }));
    let has_perm_missing = permission_results.iter().any(|(_, ok)| !ok);
    let has_recommended_missing = recommended_results
        .iter()
        .any(|(_, s)| s == &DepStatus::Missing);
    let has_config_missing = config_results.iter().any(|(_, ok)| !ok);
    let has_net_missing = network_results.iter().any(|(_, ok)| !ok);

    let status = if has_required_missing || has_perm_missing {
        ReadyStatus::NotReady
    } else if has_recommended_missing || has_config_missing || has_net_missing {
        ReadyStatus::Partial
    } else {
        ReadyStatus::Ready
    };

    ToolReadyResult {
        tool_name: tool_name.to_string(),
        status,
        required_results,
        recommended_results,
        config_results,
        permission_results,
        network_results,
    }
}

/// Format a DepStatus as a human-readable string.
fn format_dep_status(status: &DepStatus) -> String {
    match status {
        DepStatus::Available => "✓".to_string(),
        DepStatus::Missing => "missing".to_string(),
        DepStatus::VersionLow { installed, required } => {
            format!("version low ({} < {})", installed, required)
        }
    }
}

/// Format a ReadyStatus as a human-readable label.
fn format_status(status: &ReadyStatus) -> &'static str {
    match status {
        ReadyStatus::Ready => "READY",
        ReadyStatus::Partial => "PARTIAL",
        ReadyStatus::NotReady => "NOT READY",
    }
}

/// Generate a full checklist string.
fn generate_checklist(results: &[ToolReadyResult]) -> String {
    let mut output = String::new();
    output.push_str("Tool Environment Ready Checklist\n");
    output.push_str("=================================\n");

    for result in results {
        let status_icon = match result.status {
            ReadyStatus::Ready => "✅",
            ReadyStatus::Partial => "⚠️",
            ReadyStatus::NotReady => "❌",
        };

        let mut details = Vec::new();
        for (dep, status) in &result.required_results {
            details.push(format!("{} {} ({})", dep.binary, format_dep_status(status), dep.manager));
        }
        for (dep, status) in &result.recommended_results {
            details.push(format!("{} {} ({})", dep.binary, format_dep_status(status), dep.manager));
        }
        let details_str = if details.is_empty() {
            "no dependencies"
        } else {
            &details.join(", ")
        };

        output.push_str(&format!(
            "{} {:10} — {:9} ({})\n",
            status_icon,
            result.tool_name,
            format_status(&result.status),
            details_str
        ));
    }

    let ready_count = results.iter().filter(|r| r.status == ReadyStatus::Ready).count();
    let partial_count = results.iter().filter(|r| r.status == ReadyStatus::Partial).count();
    let not_ready_count = results.iter().filter(|r| r.status == ReadyStatus::NotReady).count();

    output.push_str("\n");
    output.push_str(&format!(
        "Summary: {} ready, {} partial, {} not ready (total: {})\n",
        ready_count,
        partial_count,
        not_ready_count,
        results.len()
    ));

    output
}

/// Auto-fix missing dependencies via tokenless-env-fix.sh.
fn auto_fix(missing_deps: &[DepEntry]) -> Result<String, String> {
    let fix_script = std::env::var("TOKENLESS_ENV_FIX_SCRIPT")
        .unwrap_or_else(|_| "/usr/share/tokenless/core/env-check/tokenless-env-fix.sh".to_string());

    // Build JSON array of missing deps
    let deps_json: Vec<Value> = missing_deps
        .iter()
        .map(|dep| {
            let mut obj = serde_json::Map::new();
            obj.insert("binary".to_string(), Value::String(dep.binary.clone()));
            if let Some(ref v) = dep.version {
                obj.insert("version".to_string(), Value::String(v.clone()));
            }
            obj.insert("package".to_string(), Value::String(dep.package.clone()));
            obj.insert("manager".to_string(), Value::String(dep.manager.clone()));
            if let Some(ref pn) = dep.pip_name {
                obj.insert("pip_name".to_string(), Value::String(pn.clone()));
            }
            if let Some(ref un) = dep.uv_name {
                obj.insert("uv_name".to_string(), Value::String(un.clone()));
            }
            if let Some(ref nn) = dep.npm_name {
                obj.insert("npm_name".to_string(), Value::String(nn.clone()));
            }
            if dep.use_npx {
                obj.insert("use_npx".to_string(), Value::Bool(true));
            }
            if !dep.fallback.is_empty() {
                let fb_arr: Vec<Value> = dep
                    .fallback
                    .iter()
                    .map(|fb| {
                        let mut fb_obj = serde_json::Map::new();
                        fb_obj.insert("method".to_string(), Value::String(fb.method.clone()));
                        if let Some(ref p) = fb.package {
                            fb_obj.insert("package".to_string(), Value::String(p.clone()));
                        }
                        if let Some(ref b) = fb.binary {
                            fb_obj.insert("binary".to_string(), Value::String(b.clone()));
                        }
                        if let Some(ref s) = fb.source {
                            fb_obj.insert("source".to_string(), Value::String(s.clone()));
                        }
                        if let Some(ref m) = fb.manifest {
                            fb_obj.insert("manifest".to_string(), Value::String(m.clone()));
                        }
                        if let Some(ref f) = fb.features {
                            fb_obj.insert("features".to_string(), Value::String(f.clone()));
                        }
                        Value::Object(fb_obj)
                    })
                    .collect();
                obj.insert("fallback".to_string(), Value::Array(fb_arr));
            }
            Value::Object(obj)
        })
        .collect();

    let json_str = serde_json::to_string(&deps_json)
        .map_err(|e| format!("Failed to serialize deps: {}", e))?;

    let output = Command::new("bash")
        .arg(&fix_script)
        .arg("fix-all")
        .arg(&json_str)
        .output()
        .map_err(|e| format!("Failed to run env-fix: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

/// Find the spec file path.
fn find_spec_path() -> PathBuf {
    let candidates = [
        std::env::var("TOKENLESS_TOOL_READY_SPEC").ok().map(PathBuf::from),
        Some(PathBuf::from("/usr/share/tokenless/core/env-check/tool-ready-spec.json")),
        Some(PathBuf::from(format!(
            "{}/.tokenless/tool-ready-spec.json",
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
        ))),
    ];

    for candidate in candidates.iter().flatten() {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    PathBuf::from("/usr/share/tokenless/core/env-check/tool-ready-spec.json")
}

/// Run the env-check command.
pub fn run(
    tool: Option<&str>,
    all: bool,
    fix: bool,
    checklist: bool,
) -> Result<(), (String, i32)> {
    let spec_path = find_spec_path();
    let specs = load_spec(&spec_path).map_err(|e| (e, 1))?;

    if checklist {
        let results: Vec<ToolReadyResult> = specs
            .keys()
            .map(|name| check_tool(name, specs.get(name).unwrap()))
            .collect();
        println!("{}", generate_checklist(&results));
        return Ok(());
    }

    let tool_names: Vec<String> = if all {
        specs.keys().cloned().collect()
    } else if let Some(t) = tool {
        if !specs.contains_key(t) {
            return Err((format!("Unknown tool: {}", t), 1));
        }
        vec![t.to_string()]
    } else {
        return Err(("Specify --tool <name> or --all".to_string(), 1));
    };

    for tool_name in &tool_names {
        let spec = specs.get(tool_name).unwrap();
        let result = check_tool(tool_name, spec);

        println!("{}: {}", tool_name, format_status(&result.status));

        for (dep, status) in &result.required_results {
            println!("  required: {} — {} [{}]", dep.binary, format_dep_status(status), dep.manager);
        }
        for (dep, status) in &result.recommended_results {
            println!("  recommended: {} — {} [{}]", dep.binary, format_dep_status(status), dep.manager);
        }
        for (cfg, ok) in &result.config_results {
            println!("  config: {} — {}", cfg, if *ok { "✓" } else { "missing" });
        }
        for (perm, ok) in &result.permission_results {
            println!("  permission: {} — {}", perm, if *ok { "✓" } else { "missing" });
        }
        for (net, ok) in &result.network_results {
            println!("  network: {} — {}", net, if *ok { "✓" } else { "missing" });
        }

        if fix {
            let missing_deps: Vec<DepEntry> = result
                .required_results
                .iter()
                .chain(result.recommended_results.iter())
                .filter(|(_, s)| s == &DepStatus::Missing)
                .map(|(d, _)| d.clone())
                .collect();

            if !missing_deps.is_empty() {
                println!("  Attempting auto-fix for: {}", missing_deps.iter().map(|d| d.binary.clone()).collect::<Vec<_>>().join(", "));
                let fix_output = auto_fix(&missing_deps).map_err(|e| (e, 1))?;
                for line in fix_output.lines() {
                    println!("  {}", line);
                }
            } else {
                println!("  No missing dependencies to fix.");
            }
        }

        println!();
    }

    Ok(())
}