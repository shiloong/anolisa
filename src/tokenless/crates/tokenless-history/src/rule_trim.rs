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
    fn dedup_system_reminders(messages: &[HistoryMessage]) -> Vec<HistoryMessage> {
        let re = Regex::new(r"<system-reminder>(.*?)</system-reminder>").unwrap();
        let mut seen: HashMap<String, usize> = HashMap::new();

        // Scan from end to start — keep the latest occurrence index
        for i in (0..messages.len()).rev() {
            if messages[i].role != "system" {
                continue;
            }
            if let Some(caps) = re.captures(&messages[i].content) {
                let inner = caps.get(1).unwrap().as_str();
                if !seen.contains_key(inner) {
                    seen.insert(inner.to_string(), i);
                }
            }
        }

        // Replace superseded reminders with markers
        let mut result = messages.to_vec();
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
    fn dedup_ide_context(messages: &[HistoryMessage]) -> Vec<HistoryMessage> {
        let re = Regex::new(r"<ide_context>.*?</ide_context>").unwrap();

        // Find the latest IDE context message
        let latest_idx = messages.iter().rposition(|m| {
            m.role == "system" && re.is_match(&m.content)
        });

        if latest_idx.is_none() {
            return messages.to_vec();
        }

        let latest = latest_idx.unwrap();
        let mut result = messages.to_vec();

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
    fn dedup_file_reads(messages: &[HistoryMessage]) -> Vec<HistoryMessage> {
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
        let mut result = messages.to_vec();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TrimRules;
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
        let mut long_output = String::new();
        for i in 1..=120 {
            long_output.push_str(&format!("line{}\n", i));
        }
        let messages: Vec<HistoryMessage> = serde_json::from_value(json!([
            { "role": "user", "content": "[tool_result: exec]\n".to_owned() + &long_output },
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