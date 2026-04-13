// Strategy module - log parsing and path strategies
pub mod log;
pub mod path;

use crate::models::PairMetadata;
use std::path::PathBuf;

#[async_trait::async_trait]
pub trait LogParsingStrategy: Send + Sync {
    fn agent_name(&self) -> &str;
    fn target_extension(&self) -> &str;
    fn is_user_prompt(&self, line: &str) -> bool;
    fn is_completion_signal(&self, line: &str) -> bool;
    fn extract_request_id(&self, line: &str) -> Option<String>;
    fn extract_metadata(&self, line: &str) -> Option<PairMetadata>;
    fn extract_user_text(&self, line: &str) -> Option<String>;
    fn extract_ai_text(&self, line: &str) -> Option<String>;
}

pub trait PathStrategy: Send + Sync {
    fn log_directories(&self) -> Vec<(String, PathBuf)>;
    fn offset_store_path(&self) -> PathBuf;
    fn registry_store_path(&self) -> PathBuf;
}
