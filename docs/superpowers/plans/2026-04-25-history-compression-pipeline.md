# History Compression Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the three-layer History Compression Pipeline as a tokenless integration (Rust crate + CLI commands + copilot-shell hooks + OpenClaw plugin extension), providing session history trimming/compression for cosh and openclaw agents.

**Architecture:** New `tokenless-history` Rust crate provides core logic (rule trimming, tool-aware strategies, deep compression, phase detection). CLI subcommands (`trim-history`, `compress-history`) expose this via stdin/stdout. Shell hook scripts bridge cosh's BeforeModel/PostToolUse/PreCompact events. OpenClaw plugin gains `before_model` hooks for trim and deep compress.

**Tech Stack:** Rust (serde_json, regex, chrono), Bash (jq), TypeScript (OpenClaw plugin API), cosh hook system (BeforeModel, PostToolUse, PreCompact)

---

## File Structure

### New files

```
src/tokenless/crates/tokenless-history/
  Cargo.toml                       # Crate manifest: serde_json, regex, chrono deps
  src/lib.rs                        # Public exports + crate-level docs
  src/types.rs                      # HistoryMessage, TrimRules, Phase, CompressionSnapshot
  src/rule_trim.rs                  # TrimEngine: file-read dedup, reminder dedup, thought strip, IDE dedup, shell truncation
  src/phase_detector.rs             # Phase detection from message patterns
  src/deep_compressor.rs            # Deep compression: split point, <state_snapshot> XML generation

src/tokenless/hooks/copilot-shell/
  tokenless-trim-history.sh         # BeforeModel hook → tokenless trim-history
  tokenless-compress-history.sh     # PreCompact hook → tokenless compress-history
```

### Modified files

```
src/tokenless/Cargo.toml            # Add tokenless-history to workspace members
src/tokenless/crates/tokenless-cli/Cargo.toml   # Add tokenless-history dependency
src/tokenless/crates/tokenless-cli/src/main.rs  # Add trim-history + compress-history subcommands
src/tokenless/crates/tokenless-stats/src/record.rs  # Add TrimHistory + CompressHistory to OperationType
src/tokenless/Makefile              # Add history-related build targets, copilot-shell-install copies new hooks
src/tokenless/openclaw/index.ts     # Add before_model hooks for trim + deep compress
src/tokenless/openclaw/openclaw.plugin.json  # Add history_trim_enabled + deep_compression_enabled config
```

---

### Task 1: Create `tokenless-history` Crate Skeleton + Types

**Files:**
- Create: `src/tokenless/crates/tokenless-history/Cargo.toml`
- Create: `src/tokenless/crates/tokenless-history/src/lib.rs`
- Create: `src/tokenless/crates/tokenless-history/src/types.rs`
- Modify: `src/tokenless/Cargo.toml`

- [ ] **Step 1: Create Cargo.toml for tokenless-history**

```toml
[package]
name = "tokenless-history"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "History compression and trimming for LLM agent context optimization"

[dependencies]
serde_json.workspace = true
regex.workspace = true
chrono = { workspace = true, features = ["serde"] }
```

- [ ] **Step 2: Create types.rs with core type definitions**

```rust
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// A message in the LLM conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// Trim rules configuration
#[derive(Debug, Clone)]
pub struct TrimRules {
    pub file_read_dedup: bool,
    pub system_reminder_dedup: bool,
    pub ide_context_dedup: bool,
    pub thought_strip: bool,
    pub max_shell_lines: usize,
}

impl Default for TrimRules {
    fn default() -> Self {
        Self {
            file_read_dedup: true,
            system_reminder_dedup: true,
            ide_context_dedup: true,
            thought_strip: true,
            max_shell_lines: 100,
        }
    }
}

/// Task phase for relevance scoring
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Exploration,
    Implementation,
    Verification,
    Planning,
}

/// Compression snapshot for incremental compression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionSnapshot {
    pub xml: String,
    pub timestamp: DateTime<Local>,
    pub history_index: usize,
}
```

- [ ] **Step 3: Create lib.rs with public exports**

```rust
pub mod deep_compressor;
pub mod phase_detector;
pub mod rule_trim;
pub mod types;

pub use types::{CompressionSnapshot, HistoryMessage, Phase, TrimRules};
pub use rule_trim::TrimEngine;
pub use deep_compressor::DeepCompressor;
pub use phase_detector::PhaseDetector;
```

- [ ] **Step 4: Add tokenless-history to workspace Cargo.toml**

In `src/tokenless/Cargo.toml`, change the `members` array:

```toml
members = [
    "crates/tokenless-schema",
    "crates/tokenless-cli",
    "crates/tokenless-stats",
    "crates/tokenless-history",
]
```

- [ ] **Step 5: Verify crate compiles**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo check --workspace`
Expected: Compiles successfully (rule_trim, deep_compressor, phase_detector modules are empty stubs at this point — need to create them)

- [ ] **Step 6: Commit**

```bash
git add src/tokenless/Cargo.toml src/tokenless/crates/tokenless-history/
git commit -m "feat(tokenless): add tokenless-history crate skeleton with types"
```

---

### Task 2: Implement TrimEngine (Layer 1 — rule_trim.rs)

**Files:**
- Create: `src/tokenless/crates/tokenless-history/src/rule_trim.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test for file-read dedup**

In `rule_trim.rs`, add the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_file_read_dedup_keeps_latest() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "model", "content": "I'll read the file." },
            { "role": "user", "content": "[tool_result: read_file /src/auth.ts]\nexport class AuthService {\n  login() {}\n}" },
            { "role": "model", "content": "Now I'll read it again." },
            { "role": "user", "content": "[tool_result: read_file /src/auth.ts]\nexport class AuthService {\n  login() {}\n  logout() {}\n}" },
        ])).unwrap();

        let engine = TrimEngine::new(TrimRules::default());
        let trimmed = engine.trim(&messages);

        // First read should be replaced with marker
        assert!(trimmed[1].content.contains("[READ: /src/auth.ts (superseded)]"));
        // Second read should be kept intact
        assert!(trimmed[3].content.contains("logout() {}"));
        // Original content should NOT appear in first read
        assert!(!trimmed[1].content.contains("login() {}"));
    }

    #[test]
    fn test_system_reminder_dedup_keeps_latest() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "system", "content": "<system-reminder>Remember: use strict mode</system-reminder>" },
            { "role": "user", "content": "hello" },
            { "role": "model", "content": "hi" },
            { "role": "system", "content": "<system-reminder>Remember: use strict mode</system-reminder>" },
            { "role": "user", "content": "do something" },
        ])).unwrap();

        let engine = TrimEngine::new(TrimRules::default());
        let trimmed = engine.trim(&messages);

        // First reminder should be replaced with marker
        assert!(trimmed[0].content.contains("[system-reminder superseded]"));
        // Second reminder should be kept intact
        assert!(trimmed[3].content.contains("Remember: use strict mode"));
    }

    #[test]
    fn test_thought_strip_removes_thought_content() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "model", "content": "thought: I need to analyze this code carefully...\nNow I'll respond." },
            { "role": "user", "content": "ok" },
            { "role": "model", "content": "thoughtSignature: abc123\nLet me check the file." },
        ])).unwrap();

        let engine = TrimEngine::new(TrimRules::default());
        let trimmed = engine.trim(&messages);

        // thought: prefix should be stripped
        assert!(!trimmed[0].content.contains("thought:"));
        assert!(trimmed[0].content.contains("Now I'll respond."));
        // thoughtSignature should be stripped
        assert!(!trimmed[2].content.contains("thoughtSignature:"));
        assert!(trimmed[2].content.contains("Let me check the file."));
    }

    #[test]
    fn test_shell_truncation_limits_lines() {
        let long_output = "line1\nline2\n".repeat(60); // 120 lines
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "[tool_result: exec]\n" + long_output },
        ])).unwrap();

        let rules = TrimRules { max_shell_lines: 50, ..TrimRules::default() };
        let engine = TrimEngine::new(rules);
        let trimmed = engine.trim(&messages);

        // Should have truncation marker
        assert!(trimmed[0].content.contains("truncated"));
        // Should have fewer lines than original
        let line_count = trimmed[0].content.lines().count();
        assert!(line_count < 120);
    }

    #[test]
    fn test_no_trim_when_rules_disabled() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "system", "content": "<system-reminder>Remember this</system-reminder>" },
            { "role": "model", "content": "thought: something\nResponse" },
        ])).unwrap();

        let rules = TrimRules {
            file_read_dedup: false,
            system_reminder_dedup: false,
            ide_context_dedup: false,
            thought_strip: false,
            max_shell_lines: 100,
        };
        let engine = TrimEngine::new(rules);
        let trimmed = engine.trim(&messages);

        // Nothing should be trimmed
        assert_eq!(trimmed[0].content, "<system-reminder>Remember this</system-reminder>");
        assert_eq!(trimmed[1].content, "thought: something\nResponse");
    }

    #[test]
    fn test_ide_context_dedup_keeps_latest() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "system", "content": "<ide_context>workspace: /src, open files: auth.ts</ide_context>" },
            { "role": "user", "content": "hello" },
            { "role": "model", "content": "hi" },
            { "role": "system", "content": "<ide_context>workspace: /src, open files: auth.ts, user.ts</ide_context>" },
        ])).unwrap();

        let engine = TrimEngine::new(TrimRules::default());
        let trimmed = engine.trim(&messages);

        // First IDE context should be replaced with marker
        assert!(trimmed[0].content.contains("[IDE context superseded]"));
        // Second IDE context should be kept intact
        assert!(trimmed[3].content.contains("user.ts"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo test -p tokenless-history -- rule_trim::tests`
Expected: FAIL — `TrimEngine` does not exist yet

- [ ] **Step 3: Implement TrimEngine**

```rust
use crate::types::{HistoryMessage, TrimRules};
use regex::Regex;
use std::collections::HashMap;

/// Rule-based history trimming engine (Layer 1)
pub struct TrimEngine {
    rules: TrimRules,
}

impl TrimEngine {
    pub fn new(rules: TrimRules) -> Self {
        Self { rules }
    }

    /// Trim a history message array according to configured rules.
    /// Returns a new array with redundant content replaced by compact markers.
    pub fn trim(&self, messages: &[HistoryMessage]) -> Vec<HistoryMessage> {
        let mut result: Vec<HistoryMessage> = messages.to_vec();

        if self.rules.system_reminder_dedup {
            result = Self::dedup_system_reminders(&result);
        }

        if self.rules.ide_context_dedup {
            result = Self::dedup_ide_context(&result);
        }

        if self.rules.file_read_dedup {
            result = Self::dedup_file_reads(&result);
        }

        if self.rules.thought_strip {
            result = Self::strip_thoughts(&result);
        }

        result = Self::truncate_shell_output(&result, self.rules.max_shell_lines);

        result
    }

    /// Deduplicate system-reminder content: keep the latest instance,
    /// replace earlier instances with a compact marker.
    fn dedup_system_reminders(messages: &mut Vec<HistoryMessage>) -> Vec<HistoryMessage> {
        let re = Regex::new(r"<system-reminder>(.*?)</system-reminder>").unwrap();
        let mut seen: HashMap<String, usize> = HashMap::new();

        // Scan from end to start — keep the latest occurrence index
        for i in (0..messages.len()).rev() {
            if messages[i].role != "system" {
                continue;
            }
            if let Some(caps) = re.captures(&messages[i].content) {
                let inner = caps.get(1).unwrap().as_str();
                if seen.contains_key(inner) {
                    // We've already seen a later occurrence — this one is superseded
                } else {
                    seen.insert(inner.to_string(), i);
                }
            }
        }

        // Replace superseded reminders with markers
        let mut result = messages.clone();
        for (i, msg) in messages.iter().enumerate() {
            if msg.role != "system" {
                continue;
            }
            if let Some(caps) = re.captures(&msg.content) {
                let inner = caps.get(1).unwrap().as_str();
                let latest_idx = seen.get(inner).unwrap();
                if *latest_idx != i {
                    result[i] = HistoryMessage {
                        role: msg.role.clone(),
                        content: "[system-reminder superseded]".to_string(),
                    };
                }
            }
        }

        result
    }

    /// Deduplicate IDE context injections: keep the latest, replace earlier with marker.
    fn dedup_ide_context(messages: &mut Vec<HistoryMessage>) -> Vec<HistoryMessage> {
        let re = Regex::new(r"<ide_context>.*?</ide_context>").unwrap();

        // Find the latest IDE context message
        let latest_idx = messages.iter().rposition(|m| {
            m.role == "system" && re.is_match(&m.content)
        });

        if latest_idx.is_none() {
            return messages.clone();
        }

        let latest = latest_idx.unwrap();
        let mut result = messages.clone();

        // Replace all earlier IDE context messages with markers
        for i in 0..latest {
            if result[i].role == "system" && re.is_match(&result[i].content) {
                result[i] = HistoryMessage {
                    role: result[i].role.clone(),
                    content: "[IDE context superseded]".to_string(),
                };
            }
        }

        result
    }

    /// Deduplicate file-read content: keep the latest read of each file path,
    /// replace earlier reads with a compact marker.
    fn dedup_file_reads(messages: &mut Vec<HistoryMessage>) -> Vec<HistoryMessage> {
        let re = Regex::new(r"\[tool_result: read_file\s+(.+?)\]").unwrap();

        // Find the latest read for each file path
        let mut latest_reads: HashMap<String, usize> = HashMap::new();
        for i in (0..messages.len()).rev() {
            if messages[i].role != "user" {
                continue;
            }
            if let Some(caps) = re.captures(&messages[i].content) {
                let file_path = caps.get(1).unwrap().as_str().to_string();
                if !latest_reads.contains_key(&file_path) {
                    latest_reads.insert(file_path, i);
                }
            }
        }

        // Replace earlier reads with markers
        let mut result = messages.clone();
        for (i, msg) in messages.iter().enumerate() {
            if msg.role != "user" {
                continue;
            }
            if let Some(caps) = re.captures(&msg.content) {
                let file_path = caps.get(1).unwrap().as_str().to_string();
                let latest_idx = latest_reads.get(&file_path).unwrap();
                if *latest_idx != i {
                    result[i] = HistoryMessage {
                        role: msg.role.clone(),
                        content: format!("[READ: {} (superseded)]", file_path),
                    };
                }
            }
        }

        result
    }

    /// Strip thought content from model messages.
    /// Removes lines starting with "thought:" and lines containing "thoughtSignature:".
    fn strip_thoughts(messages: &[HistoryMessage]) -> Vec<HistoryMessage> {
        let thought_line_re = Regex::new(r"^thought:\s*").unwrap();
        let thought_sig_re = Regex::new(r"thoughtSignature:\s*\S+\s*").unwrap();

        messages
            .iter()
            .map(|msg| {
                if msg.role != "model" {
                    return msg.clone();
                }

                let cleaned = msg
                    .content
                    .lines()
                    .filter(|line| !thought_line_re.is_match(line))
                    .collect::<Vec<&str>>()
                    .join("\n");

                let cleaned = thought_sig_re.replace_all(&cleaned, "").to_string();
                let cleaned = cleaned.trim().to_string();

                HistoryMessage {
                    role: msg.role.clone(),
                    content: cleaned,
                }
            })
            .collect()
    }

    /// Truncate shell/exec output to max_shell_lines, keeping head + tail + marker.
    fn truncate_shell_output(messages: &[HistoryMessage], max_lines: usize) -> Vec<HistoryMessage> {
        let exec_re = Regex::new(r"\[tool_result: exec\]").unwrap();

        messages
            .iter()
            .map(|msg| {
                if msg.role != "user" || !exec_re.is_match(&msg.content) {
                    return msg.clone();
                }

                let lines: Vec<&str> = msg.content.lines().collect();
                if lines.len() <= max_lines {
                    return msg.clone();
                }

                let head_count = max_lines / 2;
                let tail_count = max_lines / 2;
                let truncated_count = lines.len() - head_count - tail_count;

                let head = &lines[..head_count];
                let tail = &lines[lines.len() - tail_count..];
                let marker = format!("... {} lines truncated ...", truncated_count);

                let new_content = format!(
                    "{}\n{}\n{}",
                    head.join("\n"),
                    marker,
                    tail.join("\n")
                );

                HistoryMessage {
                    role: msg.role.clone(),
                    content: new_content,
                }
            })
            .collect()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo test -p tokenless-history -- rule_trim::tests`
Expected: All 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/tokenless/crates/tokenless-history/src/rule_trim.rs
git commit -m "feat(tokenless-history): implement TrimEngine with file-read dedup, reminder dedup, thought strip, shell truncation"
```

---

### Task 3: Implement PhaseDetector (phase_detector.rs)

**Files:**
- Create: `src/tokenless/crates/tokenless-history/src/phase_detector.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test for phase detection**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_exploration_phase() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "find the auth module" },
            { "role": "model", "content": "I'll search for it" },
            { "role": "user", "content": "[tool_result: glob] found 3 files" },
            { "role": "model", "content": "Let me read auth.ts" },
            { "role": "user", "content": "[tool_result: read_file] file content" },
        ])).unwrap();

        let detector = PhaseDetector::new();
        let phase = detector.detect(&messages);
        assert_eq!(phase, Phase::Exploration);
    }

    #[test]
    fn test_implementation_phase() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "add the logout method" },
            { "role": "model", "content": "I'll edit auth.ts" },
            { "role": "user", "content": "[tool_result: edit] Changes applied to auth.ts" },
            { "role": "model", "content": "Now let me run the build" },
            { "role": "user", "content": "[tool_result: exec] build succeeded" },
        ])).unwrap();

        let detector = PhaseDetector::new();
        let phase = detector.detect(&messages);
        assert_eq!(phase, Phase::Implementation);
    }

    #[test]
    fn test_verification_phase() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "run the tests" },
            { "role": "model", "content": "I'll run npm test" },
            { "role": "user", "content": "[tool_result: exec] 2 tests failed" },
            { "role": "model", "content": "Let me check the error" },
            { "role": "user", "content": "[tool_result: exec] Error: Expected value 42" },
        ])).unwrap();

        let detector = PhaseDetector::new();
        let phase = detector.detect(&messages);
        assert_eq!(phase, Phase::Verification);
    }

    #[test]
    fn test_planning_phase() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "plan the refactoring" },
            { "role": "model", "content": "Here's my plan:\n1. Extract auth logic\n2. Update tests" },
        ])).unwrap();

        let detector = PhaseDetector::new();
        let phase = detector.detect(&messages);
        assert_eq!(phase, Phase::Planning);
    }

    #[test]
    fn test_default_to_exploration_for_empty_history() {
        let messages: Vec<HistoryMessage> = vec![];
        let detector = PhaseDetector::new();
        let phase = detector.detect(&messages);
        assert_eq!(phase, Phase::Exploration);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo test -p tokenless-history -- phase_detector::tests`
Expected: FAIL — `PhaseDetector` does not exist yet

- [ ] **Step 3: Implement PhaseDetector**

```rust
use crate::types::{HistoryMessage, Phase};
use regex::Regex;

/// Detects the current task phase from recent message patterns.
pub struct PhaseDetector {
    edit_re: Regex,
    exec_re: Regex,
    test_re: Regex,
    plan_re: Regex,
    search_re: Regex,
    read_re: Regex,
}

impl PhaseDetector {
    pub fn new() -> Self {
        Self {
            edit_re: Regex::new(r"\[tool_result: edit\]").unwrap(),
            exec_re: Regex::new(r"\[tool_result: exec\]").unwrap(),
            test_re: Regex::new(r"test|spec|verify|check|assert").unwrap(),
            plan_re: Regex::new(r"plan|step|approach|strategy").unwrap(),
            search_re: Regex::new(r"\[tool_result: (glob|grep)\]").unwrap(),
            read_re: Regex::new(r"\[tool_result: read_file\]").unwrap(),
        }
    }

    /// Detect phase from the last N messages.
    pub fn detect(&self, messages: &[HistoryMessage]) -> Phase {
        if messages.is_empty() {
            return Phase::Exploration;
        }

        // Analyze the last 10 messages (or all if fewer)
        let window = messages.len().min(10);
        let recent = &messages[messages.len() - window..];

        let mut impl_score = 0;
        let mut verify_score = 0;
        let mut explore_score = 0;
        let mut plan_score = 0;

        for msg in recent {
            let content = &msg.content;

            // Implementation: edit operations, build/run commands (non-test)
            if self.edit_re.is_match(content) {
                impl_score += 3;
            }
            if self.exec_re.is_match(content) && !self.test_re.is_match(content) {
                impl_score += 1;
            }

            // Verification: test runs, error messages
            if self.exec_re.is_match(content) && self.test_re.is_match(content) {
                verify_score += 3;
            }
            if content.contains("failed") || content.contains("error") || content.contains("Error") {
                verify_score += 1;
            }

            // Exploration: file search, file reads
            if self.search_re.is_match(content) {
                explore_score += 2;
            }
            if self.read_re.is_match(content) {
                explore_score += 1;
            }

            // Planning: plan-related keywords in model messages
            if msg.role == "model" && self.plan_re.is_match(content) {
                plan_score += 2;
            }
        }

        let max_score = impl_score.max(verify_score).max(explore_score).max(plan_score);
        if max_score == 0 {
            return Phase::Exploration;
        }

        if plan_score == max_score {
            Phase::Planning
        } else if verify_score == max_score {
            Phase::Verification
        } else if impl_score == max_score {
            Phase::Implementation
        } else {
            Phase::Exploration
        }
    }
}

impl Default for PhaseDetector {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo test -p tokenless-history -- phase_detector::tests`
Expected: All 5 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/tokenless/crates/tokenless-history/src/phase_detector.rs
git commit -m "feat(tokenless-history): implement PhaseDetector for task phase detection"
```

---

### Task 4: Implement DeepCompressor (deep_compressor.rs)

**Files:**
- Create: `src/tokenless/crates/tokenless-history/src/deep_compressor.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test for deep compression**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CompressionSnapshot, HistoryMessage, Phase};
    use serde_json::json;

    #[test]
    fn test_generate_snapshot_basic() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "fix the auth bug in auth.ts" },
            { "role": "model", "content": "I'll find the issue" },
            { "role": "user", "content": "[tool_result: read_file /src/auth.ts] file content" },
            { "role": "model", "content": "The bug is in the login method" },
            { "role": "user", "content": "[tool_result: edit] Fixed login method" },
        ])).unwrap();

        let compressor = DeepCompressor::new(0.3);
        let result = compressor.compress(&messages, None, Phase::Implementation);

        assert!(result.xml.contains("<state_snapshot>"));
        assert!(result.xml.contains("<overall_goal>"));
        assert!(result.xml.contains("<key_knowledge>"));
        assert!(result.xml.contains("<recent_actions>"));
        assert!(result.xml.contains("<active_files>"));
        assert!(result.xml.contains("auth.ts"));
    }

    #[test]
    fn test_incremental_compression_with_previous_snapshot() {
        let previous = CompressionSnapshot {
            xml: "<state_snapshot><overall_goal>Fix auth bug</overall_goal><key_knowledge>Auth uses JWT</key_knowledge></state_snapshot>".to_string(),
            timestamp: chrono::Local::now(),
            history_index: 2,
        };

        let new_messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "model", "content": "Now I'll edit the file" },
            { "role": "user", "content": "[tool_result: edit] Changes applied" },
        ])).unwrap();

        let compressor = DeepCompressor::new(0.3);
        let result = compressor.compress(&new_messages, Some(&previous), Phase::Implementation);

        // Incremental snapshot should reference previous context
        assert!(result.xml.contains("<state_snapshot>"));
        assert!(result.xml.contains("auth") || result.xml.contains("Fix auth bug"));
        // history_index should be after previous snapshot
        assert!(result.history_index > previous.history_index);
    }

    #[test]
    fn test_snapshot_includes_tool_context() {
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "run npm test" },
            { "role": "user", "content": "[tool_result: exec] npm test ran" },
        ])).unwrap();

        let compressor = DeepCompressor::new(0.3);
        let result = compressor.compress(&messages, None, Phase::Verification);

        assert!(result.xml.contains("<tool_context>"));
    }

    #[test]
    fn test_preserve_ratio_determines_split_point() {
        // 10 messages, preserve_ratio 0.3 → keep last 3 messages intact
        let messages: Vec<HistoryMessage> = (0..10)
            .map(|i| HistoryMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "model".to_string() },
                content: format!("message {}", i),
            })
            .collect();

        let compressor = DeepCompressor::new(0.3);
        let result = compressor.compress(&messages, None, Phase::Exploration);

        assert!(result.history_index <= 7); // Split point should be around message 7
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo test -p tokenless-history -- deep_compressor::tests`
Expected: FAIL — `DeepCompressor` does not exist yet

- [ ] **Step 3: Implement DeepCompressor**

```rust
use crate::types::{CompressionSnapshot, HistoryMessage, Phase};
use crate::phase_detector::PhaseDetector;
use chrono::{DateTime, Local};
use regex::Regex;

/// Deep compression engine for history context (Layer 3).
/// Generates `<state_snapshot>` XML from message history,
/// optionally using incremental compression with a previous snapshot.
pub struct DeepCompressor {
    preserve_ratio: f64,
    file_path_re: Regex,
    tool_result_re: Regex,
}

impl DeepCompressor {
    pub fn new(preserve_ratio: f64) -> Self {
        Self {
            preserve_ratio,
            file_path_re: Regex::new(r"/[\w./\-]+\.\w+").unwrap(),
            tool_result_re: Regex::new(r"\[tool_result: (\w+)\]").unwrap(),
        }
    }

    /// Compress messages into a `<state_snapshot>` XML.
    /// If previous_snapshot is provided, uses incremental compression.
    pub fn compress(
        &self,
        messages: &[HistoryMessage],
        previous_snapshot: Option<&CompressionSnapshot>,
        phase: Phase,
    ) -> CompressionSnapshot {
        let split_point = self.find_split_point(messages);

        // Messages to compress (everything before split point)
        let to_compress = &messages[..split_point];

        // Extract key information from messages to compress
        let overall_goal = Self::extract_goal(messages);
        let key_knowledge = Self::extract_knowledge(to_compress);
        let active_files = self.extract_active_files(messages);
        let tool_context = self.extract_tool_context(to_compress);
        let recent_actions = Self::extract_recent_actions(to_compress);
        let current_plan = Self::extract_plan(to_compress, &phase);

        // Build XML snapshot
        let mut xml = String::from("<state_snapshot>\n");

        xml.push_str(&format!("    <overall_goal>{}</overall_goal>\n", overall_goal));

        if let Some(prev) = previous_snapshot {
            xml.push_str(&format!(
                "    <previous_context>\n{}\n    </previous_context>\n",
                prev.xml
            ));
        }

        xml.push_str(&format!(
            "    <key_knowledge>\n{}\n    </key_knowledge>\n",
            key_knowledge
        ));

        xml.push_str(&format!(
            "    <tool_context>\n{}\n    </tool_context>\n",
            tool_context
        ));

        xml.push_str(&format!(
            "    <active_files>\n{}\n    </active_files>\n",
            active_files
        ));

        xml.push_str(&format!(
            "    <recent_actions>\n{}\n    </recent_actions>\n",
            recent_actions
        ));

        xml.push_str(&format!(
            "    <current_plan>\n{}\n    </current_plan>\n",
            current_plan
        ));

        xml.push_str("</state_snapshot>");

        CompressionSnapshot {
            xml,
            timestamp: Local::now(),
            history_index: split_point,
        }
    }

    /// Find the split point based on preserve_ratio.
    /// The last `preserve_ratio` of messages are kept intact.
    fn find_split_point(&self, messages: &[HistoryMessage]) -> usize {
        if messages.is_empty() {
            return 0;
        }
        let preserve_count = (messages.len() as f64 * self.preserve_ratio) as usize;
        let preserve_count = preserve_count.max(1);
        messages.len().saturating_sub(preserve_count)
    }

    /// Extract the overall goal from the first user message.
    fn extract_goal(messages: &[HistoryMessage]) -> String {
        messages
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.content.lines().next().unwrap_or("").to_string())
            .unwrap_or_else(|| "Unknown goal".to_string())
    }

    /// Extract key knowledge from compressed messages.
    fn extract_knowledge(messages: &[HistoryMessage]) -> String {
        let knowledge: Vec<String> = messages
            .iter()
            .filter(|m| m.role == "model")
            .map(|m| {
                // Take first line of each model response as key knowledge
                m.content.lines().next().unwrap_or("").to_string()
            })
            .filter(|s| !s.is_empty())
            .take(10)
            .collect();

        if knowledge.is_empty() {
            "No specific knowledge extracted".to_string()
        } else {
            knowledge.join("\n")
        }
    }

    /// Extract file paths mentioned in messages.
    fn extract_active_files(&self, messages: &[HistoryMessage]) -> String {
        let mut files: Vec<String> = Vec::new();

        for msg in messages {
            for cap in self.file_path_re.captures_iter(&msg.content) {
                let path = cap.get(0).unwrap().as_str().to_string();
                if !files.contains(&path) {
                    files.push(path);
                }
            }
        }

        // Keep last 10 unique file paths
        let recent_files: Vec<String> = files.into_iter().rev().take(10).collect();
        if recent_files.is_empty() {
            "No active files".to_string()
        } else {
            recent_files.into_iter().rev().collect::<Vec<String>>().join("\n")
        }
    }

    /// Extract tool execution context from messages.
    fn extract_tool_context(&self, messages: &[HistoryMessage]) -> String {
        let tools: Vec<String> = messages
            .iter()
            .filter_map(|m| {
                self.tool_result_re
                    .captures(&m.content)
                    .map(|caps| caps.get(1).unwrap().as_str().to_string())
            })
            .collect();

        let unique_tools: Vec<String> = tools.into_iter().collect();
        if unique_tools.is_empty() {
            "No tool context".to_string()
        } else {
            format!("Tools used: {}", unique_tools.join(", "))
        }
    }

    /// Extract recent actions from the last few messages before split point.
    fn extract_recent_actions(messages: &[HistoryMessage]) -> String {
        let actions: Vec<String> = messages
            .iter()
            .rev()
            .take(5)
            .rev()
            .map(|m| {
                let content_preview = if m.content.len() > 100 {
                    format!("{}...", &m.content[..100.min(m.content.len())])
                } else {
                    m.content.clone()
                };
                format!("[{}] {}", m.role, content_preview)
            })
            .collect();

        if actions.is_empty() {
            "No recent actions".to_string()
        } else {
            actions.join("\n")
        }
    }

    /// Extract current plan based on phase.
    fn extract_plan(messages: &[HistoryMessage], phase: &Phase) -> String {
        let plan_lines: Vec<String> = messages
            .iter()
            .filter(|m| m.role == "model")
            .rev()
            .take(3)
            .rev()
            .flat_map(|m| m.content.lines().take(5))
            .map(|s| s.to_string())
            .collect();

        let phase_desc = match phase {
            Phase::Exploration => "Currently exploring codebase",
            Phase::Implementation => "Currently implementing changes",
            Phase::Verification => "Currently verifying changes",
            Phase::Planning => "Currently planning approach",
        };

        if plan_lines.is_empty() {
            phase_desc.to_string()
        } else {
            format!("{}\n{}", phase_desc, plan_lines.join("\n"))
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo test -p tokenless-history -- deep_compressor::tests`
Expected: All 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/tokenless/crates/tokenless-history/src/deep_compressor.rs
git commit -m "feat(tokenless-history): implement DeepCompressor with incremental snapshot generation"
```

---

### Task 5: Extend Stats OperationType + CLI Subcommands

**Files:**
- Modify: `src/tokenless/crates/tokenless-stats/src/record.rs`
- Modify: `src/tokenless/crates/tokenless-cli/Cargo.toml`
- Modify: `src/tokenless/crates/tokenless-cli/src/main.rs`

- [ ] **Step 1: Add TrimHistory and CompressHistory to OperationType**

In `src/tokenless/crates/tokenless-stats/src/record.rs`, modify the `OperationType` enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum OperationType {
    CompressSchema,
    CompressResponse,
    RewriteCommand,
    TrimHistory,
    CompressHistory,
}
```

Also update `as_str()` and `FromStr`:

```rust
impl OperationType {
    pub fn as_str(&self) -> &str {
        match self {
            OperationType::CompressSchema => "compress-schema",
            OperationType::CompressResponse => "compress-response",
            OperationType::RewriteCommand => "rewrite-command",
            OperationType::TrimHistory => "trim-history",
            OperationType::CompressHistory => "compress-history",
        }
    }
}

impl FromStr for OperationType {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "compress-schema" => OperationType::CompressSchema,
            "compress-response" => OperationType::CompressResponse,
            "rewrite-command" => OperationType::RewriteCommand,
            "trim-history" => OperationType::TrimHistory,
            "compress-history" => OperationType::CompressHistory,
            _ => OperationType::CompressSchema,
        })
    }
}
```

- [ ] **Step 2: Add tokenless-history dependency to CLI Cargo.toml**

In `src/tokenless/crates/tokenless-cli/Cargo.toml`, add:

```toml
[dependencies]
tokenless-schema = { path = "../tokenless-schema" }
tokenless-stats = { path = "../tokenless-stats" }
tokenless-history = { path = "../tokenless-history" }
clap.workspace = true
serde_json.workspace = true
chrono = { workspace = true, features = ["serde"] }
rusqlite = { version = "0.31", features = ["bundled"] }
```

- [ ] **Step 3: Add trim-history and compress-history subcommands to CLI main.rs**

Add two new enum variants to `Commands`:

```rust
#[derive(Subcommand)]
enum Commands {
    /// Compress OpenAI Function Calling tool schemas
    CompressSchema {
        #[arg(short, long)]
        file: Option<String>,
        #[arg(long)]
        batch: bool,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        tool_use_id: Option<String>,
    },
    /// Compress API responses
    CompressResponse {
        #[arg(short, long)]
        file: Option<String>,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        tool_use_id: Option<String>,
        /// Tool name for category-specific summarization strategy
        #[arg(long)]
        tool_name: Option<String>,
    },
    /// Trim redundant content from LLM conversation history
    TrimHistory {
        #[arg(short, long)]
        file: Option<String>,
        /// Agent ID for stats (e.g., "copilot-shell")
        #[arg(long)]
        agent_id: Option<String>,
        /// Session ID for grouping
        #[arg(long)]
        session_id: Option<String>,
        /// Comma-separated list of enabled rules
        #[arg(long, default_value = "file-read-dedup,system-reminder-dedup,ide-context-dedup,thought-strip")]
        rules: String,
        /// Maximum shell output lines before truncation
        #[arg(long, default_value_t = 100)]
        max_shell_lines: usize,
    },
    /// Deep compress conversation history into a state snapshot
    CompressHistory {
        #[arg(short, long)]
        file: Option<String>,
        /// Agent ID for stats
        #[arg(long)]
        agent_id: Option<String>,
        /// Session ID for grouping
        #[arg(long)]
        session_id: Option<String>,
        /// Previous snapshot file for incremental compression
        #[arg(long)]
        previous_snapshot: Option<String>,
        /// Preserve ratio for history split point (0.0-1.0)
        #[arg(long, default_value_t = 0.3)]
        preserve_ratio: f64,
        /// Task phase for relevance scoring
        #[arg(long)]
        phase: Option<String>,
    },
    /// View and export statistics
    #[command(subcommand)]
    Stats(StatsCommands),
}
```

Add the match arms in `run()`:

```rust
Commands::TrimHistory {
    file,
    agent_id,
    session_id,
    rules,
    max_shell_lines,
} => {
    let input = read_input(&file).map_err(|e| (e, 2))?;
    let messages: Vec<HistoryMessage> = serde_json::from_str(&input)
        .map_err(|e| (format!("JSON parse error: {}", e), 1))?;

    let trim_rules = parse_trim_rules(&rules, max_shell_lines);
    let engine = tokenless_history::TrimEngine::new(trim_rules);
    let trimmed = engine.trim(&messages);

    let result_json = serde_json::to_string_pretty(&trimmed)
        .map_err(|e| (format!("Serialization error: {}", e), 2))?;

    println!("{}", result_json);

    record_compression_stats(
        OperationType::TrimHistory,
        agent_id,
        session_id,
        None,
        input,
        result_json.clone(),
    );
}

Commands::CompressHistory {
    file,
    agent_id,
    session_id,
    previous_snapshot,
    preserve_ratio,
    phase,
} => {
    let input = read_input(&file).map_err(|e| (e, 2))?;
    let messages: Vec<HistoryMessage> = serde_json::from_str(&input)
        .map_err(|e| (format!("JSON parse error: {}", e), 1))?;

    let prev = previous_snapshot.and_then(|path| {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<CompressionSnapshot>(&s).ok())
    });

    let detected_phase = phase
        .as_deref()
        .and_then(|p| p.parse::<Phase>().ok())
        .unwrap_or_else(|| {
            PhaseDetector::new().detect(&messages)
        });

    let compressor = DeepCompressor::new(preserve_ratio);
    let snapshot = compressor.compress(&messages, prev.as_ref(), detected_phase);

    println!("{}", snapshot.xml);

    record_compression_stats(
        OperationType::CompressHistory,
        agent_id,
        session_id,
        None,
        input,
        snapshot.xml.clone(),
    );
}
```

Add the helper function:

```rust
fn parse_trim_rules(rules_str: &str, max_shell_lines: usize) -> TrimRules {
    let enabled: Vec<&str> = rules_str.split(',').collect();
    TrimRules {
        file_read_dedup: enabled.contains(&"file-read-dedup"),
        system_reminder_dedup: enabled.contains(&"system-reminder-dedup"),
        ide_context_dedup: enabled.contains(&"ide-context-dedup"),
        thought_strip: enabled.contains(&"thought-strip"),
        max_shell_lines,
    }
}
```

- [ ] **Step 4: Run full workspace tests**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo test --workspace`
Expected: All tests pass (existing + new OperationType tests)

- [ ] **Step 5: Commit**

```bash
git add src/tokenless/crates/tokenless-stats/src/record.rs src/tokenless/crates/tokenless-cli/Cargo.toml src/tokenless/crates/tokenless-cli/src/main.rs
git commit -m "feat(tokenless): add trim-history and compress-history CLI commands, extend OperationType"
```

---

### Task 6: Create copilot-shell Hook Scripts

**Files:**
- Create: `src/tokenless/hooks/copilot-shell/tokenless-trim-history.sh`
- Create: `src/tokenless/hooks/copilot-shell/tokenless-compress-history.sh`

- [ ] **Step 1: Create tokenless-trim-history.sh (BeforeModel hook)**

```bash
#!/usr/bin/env bash
# tokenless-hook-version: 7
# Token-Less copilot-shell hook — trims redundant history messages.
# Stats are recorded automatically by tokenless trim-history.
# Requires: jq

set -euo pipefail

# --- Dependency checks (fail-open) ---
if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. History trim hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed or not in PATH. History trim hook disabled." >&2
  exit 0
fi

# --- Read input (fail-open) ---
INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read BeforeModel payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract messages array ---
MESSAGES=$(echo "$INPUT" | jq -c '.llm_request.messages // empty' 2>/dev/null || echo '')

if [ -z "$MESSAGES" ] || [ "$MESSAGES" = "null" ] || [ "$MESSAGES" = "[]" ]; then
  exit 0
fi

# Skip very short conversations (< 4 messages)
MSG_LENGTH=$(echo "$MESSAGES" | jq 'length' 2>/dev/null || echo '0')
if [ "$MSG_LENGTH" -lt 4 ]; then
  exit 0
fi

# --- Extract caller context ---
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')

# --- Trim history ---
TRIMMED=$(echo "$MESSAGES" | tokenless trim-history \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  2>/dev/null) || {
  echo "[tokenless] WARNING: History trim failed. Passing through unchanged." >&2
  exit 0
}

# Validate trimmed output is valid JSON array
if ! echo "$TRIMMED" | jq -e 'type == "array"' &>/dev/null 2>&1; then
  echo "[tokenless] WARNING: History trim returned invalid JSON. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---
jq -n \
  --argjson messages "$TRIMMED" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "BeforeModel",
      "llm_request": {
        "messages": $messages
      }
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
```

- [ ] **Step 2: Create tokenless-compress-history.sh (PreCompact hook)**

```bash
#!/usr/bin/env bash
# tokenless-hook-version: 7
# Token-Less copilot-shell hook — deep history compression.
# Stats are recorded automatically by tokenless compress-history.
# Requires: jq

set -euo pipefail

# --- Dependency checks (fail-open) ---
if ! command -v jq &>/dev/null; then
  echo "[tokenless] WARNING: jq is not installed. History compression hook disabled." >&2
  exit 0
fi

if ! command -v tokenless &>/dev/null; then
  echo "[tokenless] WARNING: tokenless is not installed or not in PATH. History compression hook disabled." >&2
  exit 0
fi

# --- Read input (fail-open) ---
INPUT=$(cat || {
  echo "[tokenless] WARNING: failed to read PreCompact payload. Passing through unchanged." >&2
  exit 0
})

# --- Extract messages from llm_request ---
MESSAGES=$(echo "$INPUT" | jq -c '.llm_request.messages // empty' 2>/dev/null || echo '')

if [ -z "$MESSAGES" ] || [ "$MESSAGES" = "null" ] || [ "$MESSAGES" = "[]" ]; then
  exit 0
fi

# --- Extract caller context ---
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || echo '')

# --- Compress history ---
SNAPSHOT=$(echo "$MESSAGES" | tokenless compress-history \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  2>/dev/null) || {
  echo "[tokenless] WARNING: History compression failed. Passing through unchanged." >&2
  exit 0
}

# Validate snapshot is non-empty
if [ -z "$SNAPSHOT" ]; then
  echo "[tokenless] WARNING: History compression returned empty output. Passing through unchanged." >&2
  exit 0
fi

# --- Build copilot-shell response ---
jq -n \
  --arg snapshot "$SNAPSHOT" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "PreCompact",
      "additionalContext": $snapshot
    }
  }' || {
  echo "[tokenless] WARNING: failed to build hook response JSON. Passing through unchanged." >&2
  exit 0
}
```

- [ ] **Step 3: Make scripts executable**

Run: `chmod +x /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-trim-history.sh /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-compress-history.sh`

- [ ] **Step 4: Verify scripts parse correctly with bash -n**

Run: `bash -n /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-trim-history.sh && bash -n /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-compress-history.sh`
Expected: No syntax errors

- [ ] **Step 5: Commit**

```bash
git add src/tokenless/hooks/copilot-shell/tokenless-trim-history.sh src/tokenless/hooks/copilot-shell/tokenless-compress-history.sh
git commit -m "feat(tokenless): add BeforeModel and PreCompact hooks for history trim and compression"
```

---

### Task 7: Update Makefile for New Hooks + History Crate

**Files:**
- Modify: `src/tokenless/Makefile`

- [ ] **Step 1: Update copilot-shell-install to copy new hooks**

In the Makefile, the `copilot-shell-install` target already copies `tokenless-*.sh` — this pattern automatically includes the new scripts. No change needed for that target.

However, update the `setup` target's echo message to mention the new hooks, and add a `test-history` target:

Change the `setup` echo block:

```makefile
setup: install openclaw-install copilot-shell-install
	@echo ""
	@echo "============================================"
	@echo "  Token-Less setup complete!"
	@echo "  - tokenless: $(INSTALL_DIR)/tokenless"
	@echo "  - rtk:       $(INSTALL_DIR)/rtk"
	@echo "  - OpenClaw:  $(OPENCLAW_DIR)/"
	@echo "  - Hooks:     $(COPILOT_SHELL_HOOK_DIR)/tokenless-*.sh"
	@echo "  - Hooks include: trim-history (BeforeModel), compress-history (PreCompact)"
	@echo "============================================"
	@echo ""
	@echo "Verify installation:"
	@echo "  tokenless --version"
	@echo "  tokenless trim-history --help"
	@echo "  rtk --version"
```

Add a `test-history` target after `test-tokenless`:

```makefile
test-history:
	@echo "==> Testing tokenless-history..."
	cargo test -p tokenless-history
```

Update `test` target to include `test-history`:

```makefile
test: test-tokenless test-history test-rtk
```

- [ ] **Step 2: Run `make test` to verify workspace compiles and tests pass**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && make test`
Expected: All workspace tests pass

- [ ] **Step 3: Commit**

```bash
git add src/tokenless/Makefile
git commit -m "feat(tokenless): update Makefile with history crate test target and setup message"
```

---

### Task 8: Extend OpenClaw Plugin

**Files:**
- Modify: `src/tokenless/openclaw/openclaw.plugin.json`
- Modify: `src/tokenless/openclaw/index.ts`

- [ ] **Step 1: Update openclaw.plugin.json config schema**

Replace the entire `configSchema` and `uiHints` sections:

```json
{
  "id": "tokenless-openclaw",
  "name": "Token-Less",
  "version": "5.1.0",
  "description": "Unified RTK command rewriting + schema/response compression + history trimming/compression",
  "configSchema": {
    "type": "object",
    "properties": {
      "rtk_enabled": { "type": "boolean", "default": true },
      "schema_compression_enabled": { "type": "boolean", "default": true },
      "response_compression_enabled": { "type": "boolean", "default": true },
      "history_trim_enabled": { "type": "boolean", "default": true },
      "deep_compression_enabled": { "type": "boolean", "default": true },
      "verbose": { "type": "boolean", "default": false }
    }
  },
  "uiHints": {
    "rtk_enabled": { "label": "Enable RTK command rewriting" },
    "schema_compression_enabled": { "label": "Enable schema compression" },
    "response_compression_enabled": { "label": "Enable response compression" },
    "history_trim_enabled": { "label": "Enable history rule trimming (BeforeModel)" },
    "deep_compression_enabled": { "label": "Enable deep history compression (BeforeModel)" },
    "verbose": { "label": "Verbose logging" }
  }
}
```

- [ ] **Step 2: Update openclaw/index.ts — add helper functions and hooks**

Add `tryTrimHistory` and `tryCompressHistory` helper functions after `tryCompressResponse`:

```typescript
function tryTrimHistory(messages: any[], sessionId?: string): any[] | null {
  try {
    const input = JSON.stringify(messages);
    const args = ["trim-history", "--agent-id", "openclaw"];
    if (sessionId) args.push("--session-id", sessionId);
    const result = execFileSync("tokenless", args, {
      encoding: "utf-8",
      timeout: 2000,
      input,
    }).trim();
    return JSON.parse(result);
  } catch {
    return null;
  }
}

function tryCompressHistory(messages: any[], sessionId?: string): string | null {
  try {
    const input = JSON.stringify(messages);
    const args = ["compress-history", "--agent-id", "openclaw"];
    if (sessionId) args.push("--session-id", sessionId);
    const result = execFileSync("tokenless", args, {
      encoding: "utf-8",
      timeout: 5000,
      input,
    }).trim();
    return result;
  } catch {
    return null;
  }
}
```

In the `register(api)` method, add config reads for the new options:

```typescript
const historyTrimEnabled = pluginConfig.history_trim_enabled !== false;
const deepCompressionEnabled = pluginConfig.deep_compression_enabled !== false;
```

Add the two new hooks after the existing response compression hook:

```typescript
// ---- 3. History trimming (before_model, priority 5) -----------------------

if (historyTrimEnabled && checkTokenless()) {
  api.on(
    "before_model",
    (event: { llm_request?: { messages?: any[]; model?: string } }, ctx: { sessionId?: string; sessionKey?: string }) => {
      const messages = event.llm_request?.messages;
      if (!messages || messages.length < 4) return;

      const trimmed = tryTrimHistory(messages, ctx?.sessionKey);
      if (!trimmed) return;

      if (verbose) {
        console.log(`[tokenless/history] trim: ${messages.length} -> ${trimmed.length} messages`);
      }

      return {
        hookSpecificOutput: {
          hookEventName: "BeforeModel",
          llm_request: { messages: trimmed },
        },
      };
    },
    { priority: 5 },
  );
}

// ---- 4. Deep history compression (before_model, priority 20) --------------

if (deepCompressionEnabled && checkTokenless()) {
  api.on(
    "before_model",
    (event: { llm_request?: { messages?: any[]; model?: string } }, ctx: { sessionId?: string; sessionKey?: string }) => {
      const messages = event.llm_request?.messages;
      if (!messages) return;

      // Estimate tokens: ~4 chars per token
      const totalChars = messages.reduce((sum: number, m: any) => sum + (typeof m.content === "string" ? m.content.length : 0), 0);
      const estimatedTokens = Math.ceil(totalChars / 4);
      const contextWindow = 128000;
      if (estimatedTokens < contextWindow * 0.7) return;

      const snapshot = tryCompressHistory(messages, ctx?.sessionKey);
      if (!snapshot) return;

      if (verbose) {
        console.log(`[tokenless/history] deep compress: ${estimatedTokens} estimated tokens -> <state_snapshot>`);
      }

      const preserveCount = Math.floor(messages.length * 0.3);
      const preservedMessages = messages.slice(-preserveCount);
      const snapshotMessage = { role: "system", content: snapshot };

      return {
        hookSpecificOutput: {
          hookEventName: "BeforeModel",
          llm_request: {
            messages: [snapshotMessage, ...preservedMessages],
          },
        },
      };
    },
    { priority: 20 },
  );
}
```

Update the verbose feature listing at the end:

```typescript
if (verbose) {
  const features = [
    rtkEnabled && rtkAvailable ? "rtk-rewrite" : null,
    responseCompressionEnabled && tokenlessAvailable ? "response-compression" : null,
    historyTrimEnabled && tokenlessAvailable ? "history-trim" : null,
    deepCompressionEnabled && tokenlessAvailable ? "deep-compression" : null,
  ].filter(Boolean);
  console.log(`[tokenless] OpenClaw plugin v5.1 registered — active features: ${features.join(", ") || "none"}`);
}
```

- [ ] **Step 3: Commit**

```bash
git add src/tokenless/openclaw/openclaw.plugin.json src/tokenless/openclaw/index.ts
git commit -m "feat(tokenless): extend OpenClaw plugin with history trim and deep compression hooks"
```

---

### Task 9: Integration Smoke Test — CLI Commands

**Files:** None (validation only)

- [ ] **Step 1: Build the full tokenless workspace**

Run: `cd /Users/shiloong/workspace/anolisa/src/tokenless && cargo build --release`
Expected: Successful build, produces `target/release/tokenless` binary

- [ ] **Step 2: Test `tokenless trim-history` with synthetic input**

Run:
```bash
echo '[{"role":"system","content":"<system-reminder>Remember this</system-reminder>"},{"role":"user","content":"hello"},{"role":"model","content":"thought: I should think...\nResponse"},{"role":"system","content":"<system-reminder>Remember this</system-reminder>"}]' | target/release/tokenless trim-history --agent-id test
```

Expected: JSON output where:
- First system-reminder is replaced with `[system-reminder superseded]`
- Second system-reminder is kept intact
- `thought:` prefix is stripped from model message

- [ ] **Step 3: Test `tokenless compress-history` with synthetic input**

Run:
```bash
echo '[{"role":"user","content":"fix auth bug"},{"role":"model","content":"I will find the issue"},{"role":"user","content":"[tool_result: read_file] content"},{"role":"model","content":"The bug is in login"}]' | target/release/tokenless compress-history --agent-id test
```

Expected: `<state_snapshot>` XML output containing `<overall_goal>`, `<key_knowledge>`, `<active_files>`, `<tool_context>`

- [ ] **Step 4: Test `tokenless stats summary` to verify stats recording**

Run: `target/release/tokenless stats enable && echo '[{"role":"user","content":"test"}]' | target/release/tokenless trim-history --agent-id test && target/release/tokenless stats summary`

Expected: Summary includes at least one `trim-history` operation record

- [ ] **Step 5: Commit (no files changed, this is validation only)**

No commit needed — validation step.

---

### Task 10: Integration Smoke Test — Hook Scripts

**Files:** None (validation only)

- [ ] **Step 1: Test tokenless-trim-history.sh with synthetic BeforeModel payload**

Run:
```bash
echo '{"session_id":"test-session","llm_request":{"model":"gemini-2.5-flash","messages":[{"role":"system","content":"<system-reminder>Remember this</system-reminder>"},{"role":"user","content":"hello"},{"role":"model","content":"thought: thinking...\nResponse"},{"role":"system","content":"<system-reminder>Remember this</system-reminder>"}]}}' | bash /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-trim-history.sh
```

Expected: JSON output with `hookSpecificOutput.hookEventName: "BeforeModel"` and trimmed `messages` array

- [ ] **Step 2: Test tokenless-compress-history.sh with synthetic PreCompact payload**

Run:
```bash
echo '{"session_id":"test-session","trigger":"auto","llm_request":{"messages":[{"role":"user","content":"fix auth"},{"role":"model","content":"I will help"},{"role":"user","content":"[tool_result: read_file] content"},{"role":"model","content":"Found the bug"}]}}' | bash /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-compress-history.sh
```

Expected: JSON output with `hookSpecificOutput.hookEventName: "PreCompact"` and `<state_snapshot>` XML in `additionalContext`

- [ ] **Step 3: Verify fail-open behavior (missing tokenless binary)**

Run with PATH excluding tokenless:
```bash
echo '{"session_id":"test","llm_request":{"messages":[{"role":"user","content":"hi"}]}}' | PATH="/usr/bin" bash /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-trim-history.sh
```

Expected: Warning message on stderr, exit 0 (pass-through)

- [ ] **Step 4: No commit (validation only)**

---

### Task 11: Update existing compress-response hook with --tool-name support

**Files:**
- Modify: `src/tokenless/hooks/copilot-shell/tokenless-compress-response.sh`

- [ ] **Step 1: Add --tool-name flag to the existing hook script**

In `tokenless-compress-response.sh`, update the `tokenless compress-response` call to pass `--tool-name`:

Replace the existing compression block:
```bash
# --- Compress response ---
COMPRESSED=$(echo "$TOOL_RESPONSE" | tokenless compress-response \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  ${TOOL_USE_ID:+--tool-use-id "$TOOL_USE_ID"} \
  2>/dev/null) || {
```

With:
```bash
# --- Compress response (with tool-aware strategy) ---
COMPRESSED=$(echo "$TOOL_RESPONSE" | tokenless compress-response \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  ${TOOL_USE_ID:+--tool-use-id "$TOOL_USE_ID"} \
  ${TOOL_NAME:+--tool-name "$TOOL_NAME"} \
  2>/dev/null) || {
```

- [ ] **Step 2: Test the updated script**

Run:
```bash
echo '{"session_id":"test","tool_name":"shell","tool_response":{"stdout":"line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nline16\nline17\nline18\nline19\nline20\nline21\nline22\nline23\nline24\nline25\nline26\nline27\nline28\nline29\nline30\nline31\nline32\nline33\nline34\nline35\nline36\nline37\nline38\nline39\nline40\nline41\nline42\nline43\nline44\nline45\nline46\nline47\nline48\nline49\nline50\nline51\nline52\nline53\nline54\nline55\nline56\nline57\nline58\nline59\nline60\nline61\nline62\nline63\nline64\nline65\nline66\nline67\nline68\nline69\nline70\nline71\nline72\nline73\nline74\nline75\nline76\nline77\nline78\nline79\nline80\nline81\nline82\nline83\nline84\nline85\nline86\nline87\nline88\nline89\nline90\nline91\nline92\nline93\nline94\nline95\nline96\nline97\nline98\nline99\nline100\n"}}}' | bash /Users/shiloong/workspace/anolisa/src/tokenless/hooks/copilot-shell/tokenless-compress-response.sh
```

Expected: Compressed response output (the tool_name field is now passed to CLI)

- [ ] **Step 3: Commit**

```bash
git add src/tokenless/hooks/copilot-shell/tokenless-compress-response.sh
git commit -m "feat(tokenless): add --tool-name flag to compress-response hook for tool-aware strategy"
```

---

## Self-Review Checklist

### 1. Spec Coverage

| Spec Section | Covered by Task |
|---|---|
| Section 2 (Agent Interaction Model) | Background context — not directly coded, informs hook event choices |
| Section 4 (Layer 1 — RuleTrimPass) | Task 2: TrimEngine implements all 5 trim rules |
| Section 5 (Layer 2 — ToolOutputSummarizer) | Task 11: --tool-name flag extends compress-response hook |
| Section 6 (Layer 3 — DeepCompress) | Task 4: DeepCompressor implements snapshot generation |
| Section 7 (Dynamic Context Management) | Task 3: PhaseDetector implements phase detection |
| Section 9 (Configuration) | Task 8: openclaw.plugin.json adds config fields |
| Section 10 (File Structure) | Covered: tokenless-history crate, hook scripts, CLI, plugin |
| Section 14 (tokenless Integration) | All sub-sections covered: crate (Task 1-4), CLI (Task 5), hooks (Task 6), plugin (Task 8), Makefile (Task 7), smoke tests (Task 9-10) |
| Section 14.7 (Responsibility Boundary) | Followed: tokenless provides hooks, cosh internal handles state |
| Section 14.8 (Stats Recording) | Task 5: OperationType extended |

### 2. Placeholder Scan

No TBD, TODO, or placeholder patterns found. All steps contain complete code.

### 3. Type Consistency

- `HistoryMessage` defined in Task 1 `types.rs`, used consistently in Tasks 2, 3, 4, 5
- `TrimRules` defined in Task 1, used in Tasks 2, 5 (`parse_trim_rules`)
- `Phase` enum defined in Task 1, used in Tasks 3, 4, 5
- `CompressionSnapshot` defined in Task 1, used in Tasks 4, 5
- `OperationType::TrimHistory` / `CompressHistory` added in Task 5, referenced in CLI match arms
- CLI `--rules` default value matches `TrimRules` field names