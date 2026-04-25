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
            { "role": "user", "content": "[tool_result: read_file /src/auth.ts] file content" },
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