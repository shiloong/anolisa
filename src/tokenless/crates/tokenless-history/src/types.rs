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

impl std::str::FromStr for Phase {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "exploration" => Phase::Exploration,
            "implementation" => Phase::Implementation,
            "verification" => Phase::Verification,
            "planning" => Phase::Planning,
            _ => Phase::Exploration,
        })
    }
}

/// Compression snapshot for incremental compression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionSnapshot {
    pub xml: String,
    pub timestamp: DateTime<Local>,
    pub history_index: usize,
}