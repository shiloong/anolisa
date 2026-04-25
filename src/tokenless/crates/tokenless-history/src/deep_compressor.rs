use crate::types::{CompressionSnapshot, HistoryMessage, Phase};
use chrono::Local;
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
                    let end = 100.min(m.content.len());
                    format!("{}...", &m.content[..end])
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
        let model_msgs: Vec<&HistoryMessage> = messages
            .iter()
            .filter(|m| m.role == "model")
            .rev()
            .take(3)
            .collect();

        let plan_lines: Vec<String> = model_msgs
            .into_iter()
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
            { "role": "user", "content": "[tool_result: edit /src/auth.ts] Fixed login method" },
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
            timestamp: Local::now(),
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
        assert!(result.xml.contains("<previous_context>"));
        // history_index is relative to input messages, not global index
        assert!(result.history_index >= 1);
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