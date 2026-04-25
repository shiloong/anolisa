pub mod deep_compressor;
pub mod phase_detector;
pub mod rule_trim;
pub mod types;

pub use types::{CompressionSnapshot, HistoryMessage, Phase, TrimRules};
pub use rule_trim::TrimEngine;
pub use deep_compressor::DeepCompressor;
pub use phase_detector::PhaseDetector;