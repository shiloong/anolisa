# Token-Less copilot-shell Hooks

Intercept and optimize LLM interactions via copilot-shell hooks for **significant token savings** and **environment readiness detection**.

## Features

| Feature | Hook Event | Status | Savings |
|---------|-----------|--------|---------|
| Tool environment ready check | PreToolUse (all tools) | ✅ Fully available | Reduces retry token waste |
| Command rewriting (RTK) | PreToolUse (Shell) | ✅ Fully available | 60–90% |
| Response compression + failure attribution → TOON | PostToolUse | ✅ Fully available | 30–60% (combined) + retry reduction |
| Schema compression | BeforeModel | ⏳ Placeholder (waiting for anolisa protocol to include `tools` in LLMRequest) | ~57% |

## How It Works

### Tool Environment Ready Check (`tokenless-tool-ready.sh`)

1. copilot-shell fires `PreToolUse` before every tool call.
2. The hook reads the JSON payload from stdin and extracts `tool_name`.
3. Loads per-tool dependency declarations from `tool-ready-spec.json`.
4. Scans required binaries (with version constraints), config files, permissions, and network.
5. If dependencies are missing, attempts auto-fix via `tokenless-env-fix.sh`.
6. Injects environment status into `additionalContext` for LLM understanding.
7. When `NOT_READY`, includes "Skip retry" guidance to prevent wasted token consumption.

**Key benefit**: LLM receives explicit environment diagnostics before tool execution, avoiding unnecessary retries on environment failures.

### Environment Auto-Fix (`tokenless-env-fix.sh`)

Auto-installs missing dependencies using config-driven strategies:
1. **System packages** (apt/yum/dnf/apk): jq, curl, bash
2. **Cargo install** (primary for Rust tools): rtk, tokenless, toon
3. **Local binaries** (symlink from INSTALL_DIR): rtk, tokenless, toon
4. **Build from source** (cargo build from local manifest): rtk, tokenless, toon
5. **Python/Node packages** (pip/uv/npm/npx): extensible for any tool

Fix results are logged to `~/.tokenless/env-fix.log`. Duplicate fixes within 24h are skipped.

### Command Rewriting (`tokenless-rewrite.sh`)

1. copilot-shell fires `PreToolUse` before every `Shell` tool call.
2. The hook reads the JSON payload from stdin (`{ "tool_input": { "command": "..." } }`).
3. Delegates to `rtk rewrite` — the single source of truth for all rewrite rules.
4. Returns a JSON response with `hookSpecificOutput.tool_input` containing the rewritten command.

### Response Compression + Failure Attribution → TOON Pipeline (`tokenless-compress-response.sh`)

The response compression hook runs a **sequential pipeline**:

1. copilot-shell fires `PostToolUse` after every tool call completes.
2. The hook reads the JSON payload from stdin (includes `tool_response`).
3. **Failure Attribution**: If the response contains errors, pattern-matches to classify as environment issue vs logic error. Injects "Skip retry — environment issue" for environment failures.
4. **Step 1 — Response Compression**: via `tokenless compress-response`:
   - Removes debug fields (debug, trace, stack, logs)
   - Removes null values and empty objects/arrays
   - Truncates long strings (>512 chars) and large arrays (>16 items)
5. **Step 2 — TOON Encoding** (if compressed result is valid JSON and `toon` is installed):
   - Encodes the compressed JSON to TOON format via `toon -e`
   - Eliminates JSON syntax overhead (quotes, commas, braces)
6. Returns a JSON response with `suppressOutput: true` and combined attribution + compressed content as `additionalContext`.

```
Original JSON ──▶ Attribution ──▶ Response Compression ──▶ TOON Encoding ──▶ Agent
                   (classify)       (strip noise)            (format)
```

**Attribution categories**:

| Pattern | Category | Example Fix Hint |
|---------|----------|-----------------|
| "command not found" | ENV_DEPENDENCY_MISSING | Install missing binary |
| "Permission denied" | ENV_PERMISSION | Check file/dir permissions |
| "No such file or directory" | ENV_FILE_MISSING | Create required file/directory |
| "Connection refused/timeout" | ENV_NETWORK | Check network connectivity |
| "ModuleNotFoundError" | ENV_PACKAGE_MISSING | Install required module |

> **Note:** TOON encoding only applies if the response-compressed result is still valid JSON. If compression produces non-JSON output (e.g., truncated marker breaks JSON), TOON is skipped and the response-compressed JSON is returned directly.

### Schema Compression (`tokenless-compress-schema.sh`)

1. copilot-shell fires `BeforeModel` before each LLM request.
2. The hook reads the JSON payload from stdin (includes `llm_request`).
3. Compresses tool schemas via `tokenless compress-schema --batch`.
4. Returns a JSON response with the compressed `tools` array.

> **Note:** Schema compression is currently a functional placeholder. The anolisa copilot-shell protocol does not yet include `tools` in the decoupled `LLMRequest` type. The hook will activate automatically once the protocol is extended — no code changes required.

All hooks are **fail-open**: if dependencies are missing or processing fails, the original data passes through unchanged.

## Prerequisites

| Dependency | Version   | Required |
|------------|-----------|----------|
| jq         | any       | Yes |
| rtk        | >= 0.35.0 | Yes (for command rewriting) |
| tokenless  | any       | Yes (for schema/response compression) |
| toon       | any       | Recommended (for TOON encoding step) |

## Installation

### Automatic

```bash
make copilot-shell-install
```

### Manual

1. Copy the hook scripts and spec:
```bash
mkdir -p /usr/share/tokenless/hooks/copilot-shell
cp hooks/copilot-shell/tokenless-*.sh /usr/share/tokenless/hooks/copilot-shell/
cp hooks/copilot-shell/tool-ready-spec.json /usr/share/tokenless/hooks/copilot-shell/
chmod +x /usr/share/tokenless/hooks/copilot-shell/tokenless-*.sh
```

2. Add the following to your settings file (`~/.copilot-shell/settings.json` or `~/.qwen-code/settings.json`):
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-tool-ready.sh",
            "name": "tokenless-tool-ready",
            "timeout": 3000
          }
        ]
      },
      {
        "matcher": "Shell",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-rewrite.sh",
            "name": "tokenless-rewrite",
            "timeout": 5000
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-response.sh",
            "name": "tokenless-compress-response",
            "timeout": 10000
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-schema.sh",
            "name": "tokenless-compress-schema",
            "timeout": 10000
          }
        ]
      }
    ]
  }
}
```

The `tokenless-compress-response.sh` hook includes failure attribution analysis and TOON encoding — no separate hooks needed.

## Verification

Test each hook manually:

```bash
# Tool environment ready check
echo '{"tool_name":"Shell","tool_input":{"command":"ls"}}' | bash hooks/copilot-shell/tokenless-tool-ready.sh

# Command rewriting
echo '{"tool_input":{"command":"cargo test"}}' | bash hooks/copilot-shell/tokenless-rewrite.sh

# Response compression + failure attribution → TOON
echo '{"tool_name":"Shell","tool_response":"{\"exit_code\":1,\"stderr\":\"command not found: rtk\"}"}' | bash hooks/copilot-shell/tokenless-compress-response.sh

# Schema compression (currently no-op until protocol adds tools support)
echo '{"llm_request":{"tools":[{"name":"test","description":"A test tool","parameters":{}}]}}' | bash hooks/copilot-shell/tokenless-compress-schema.sh
```

### CLI Environment Check

```bash
# Check a specific tool's environment
tokenless env-check --tool Shell

# Check all tools
tokenless env-check --all

# Check and auto-fix missing dependencies
tokenless env-check --all --fix

# Generate full checklist
tokenless env-check --checklist
```

## Token Savings Examples

| Original Command       | Rewritten          | Typical Savings |
|------------------------|--------------------|-----------------|
| `cargo build`          | `rtk build`        | ~70%            |
| `cargo test`           | `rtk test`         | ~80%            |
| `npm run build`        | `rtk build`        | ~65%            |
| `go test ./...`        | `rtk test`         | ~75%            |
| `python -m pytest`     | `rtk test`         | ~85%            |
| `git diff --stat`      | `rtk diff --stat`  | ~60%            |

## Compression Pipeline Examples

| Input | Response Compression | + TOON | Combined Savings |
|-------|---------------------|--------|-----------------|
| `{"data":"ok","debug":"x","trace":"y","null":null}` | `{"data":"ok"}` (50%) | N/A (too small) | 50% |
| Large JSON with debug fields | Strips noise, truncates | Encodes clean JSON | 40–60% |
| Tabular JSON array `[{"id":1,...},{"id":2,...}]` | Keeps structure, removes nulls | Table format `users[2]{id,name}:` | 50–70% |

## Tool Ready Spec Configuration

The `tool-ready-spec.json` file declares per-tool dependencies using **object format** (config-driven):

```json
{
  "_comment": "Tool Ready dependency specification...",
  "Shell": {
    "required": [
      { "binary": "jq", "package": "jq", "manager": "apt" },
      { "binary": "bash", "package": "bash", "manager": "apt" }
    ],
    "recommended": [
      { "binary": "rtk", "version": ">=0.35", "package": "rtk", "manager": "cargo",
        "fallback": [
          { "method": "symlink", "binary": "rtk", "source": "/usr/share/tokenless/bin/rtk" },
          { "method": "cargo_build", "manifest": "/usr/share/tokenless/third_party/rtk/Cargo.toml", "binary": "rtk" }
        ]
      }
    ],
    "config_files": ["~/.copilot-shell/settings.json"],
    "permissions": [],
    "network": []
  }
}
```

**Backward compatibility**: String format `"jq"` is still supported and auto-converts to `{binary:"jq", package:"jq", manager:"apt"}`. Version constraints like `"rtk>=0.35"` are also parsed from strings.

### Dependency entry fields

| Field | Required | Description |
|-------|----------|-------------|
| `binary` | Yes | Binary/command name to check (`command -v` target) |
| `version` | No | Version constraint, e.g. `>=0.35` (default: any version) |
| `package` | Yes | Package name in the declared manager (may differ from binary) |
| `manager` | Yes | Package manager: apt/rpm/pip/uv/npm/npx/cargo/cargo_build/symlink/path/dir |
| `pip_name` | No | pip install package name (default = package) |
| `uv_name` | No | uv install package name (default = package) |
| `npm_name` | No | npm install package name (default = package) |
| `use_npx` | No | Whether to use npx for npm packages |
| `fallback` | No | Fallback install strategies array |

### Supported package managers

| Manager | Install method | Example |
|---------|---------------|---------|
| apt | apt-get / yum / dnf / apk | `{binary:"jq", package:"jq", manager:"apt"}` |
| rpm | yum / dnf / rpm | `{binary:"curl", package:"curl", manager:"rpm"}` |
| pip | pip / pip3 | `{binary:"uv", package:"uv", manager:"pip", pip_name:"uv"}` |
| uv | uv tool install / uv pip install | `{binary:"pytest", package:"pytest", manager:"uv"}` |
| npm | npm install -g | `{binary:"tsc", package:"typescript", manager:"npm", npm_name:"typescript"}` |
| npx | npx -y (verify availability) | `{binary:"tsc", package:"typescript", manager:"npx", use_npx:true}` |
| cargo | cargo install --locked | `{binary:"rtk", package:"rtk", manager:"cargo"}` |
| cargo_build | cargo build from local manifest | Use in fallback: `{method:"cargo_build", manifest:"...", binary:"rtk"}` |
| symlink | ln -sf from source | `{binary:"rtk", manager:"symlink", source:"/usr/share/tokenless/bin/rtk"}` |
| path | Add directory to PATH | `{manager:"path", source:"/usr/share/tokenless/bin"}` |
| dir | mkdir -p | `{manager:"dir", source:"/some/path"}` |

### Fallback strategies

When the primary manager fails, fallback strategies are tried in order:

```json
{
  "binary": "rtk", "manager": "cargo",
  "fallback": [
    { "method": "symlink", "binary": "rtk", "source": "/usr/share/tokenless/bin/rtk" },
    { "method": "cargo_build", "manifest": "/usr/share/tokenless/third_party/rtk/Cargo.toml", "binary": "rtk" }
  ]
}
```

### Top-level spec fields

| Field | Description |
|-------|-------------|
| `required` | Must-have dependencies. Missing = NOT_READY |
| `recommended` | Optional but beneficial. Missing = PARTIAL |
| `config_files` | Required config file paths (supports `~` expansion). Missing = PARTIAL |
| `permissions` | Required permissions (file_read, file_write, exec_shell). Missing = NOT_READY |
| `network` | Required network capabilities (https_outbound). Missing = PARTIAL |

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Hook not firing | Verify `settings.json` path and restart copilot-shell |
| `jq not installed` warning | Install jq: `brew install jq` (macOS) or `apt install jq` (Linux) |
| `rtk too old` warning | Upgrade: `cargo install rtk` |
| Command not rewritten | Not all commands have RTK equivalents — check `rtk rewrite "cmd"` directly |
| `tokenless not installed` warning | Build and install: `make install` |
| Response not compressed | Responses shorter than 200 bytes are skipped (not worth compressing) |
| TOON step skipped | Install toon: `make build-toon && make install`. Response compression still works without toon. |
| Schema compression not active | Expected — waiting for anolisa protocol to add `tools` to LLMRequest |
| JSON parse error | Ensure the settings JSON is valid — use `jq . < settings.json` to validate |
| Tool ready check not running | Ensure matcher="" hook is configured in settings.json PreToolUse section |
| Auto-fix not working | Check ~/.tokenless/env-fix.log for fix results; ensure tokenless-env-fix.sh is executable |
| Env attribution not appearing | Only appears when tool_response contains error indicators (exit_code!=0, stderr, error fields) |