// Watcher module - file and log watchers
pub mod log_watcher;
pub mod claude_log_watcher;
pub mod copilot_log_watcher;
pub mod file_watcher;

pub use log_watcher::{OffsetStore, PairManagerTrait};
pub use claude_log_watcher::ClaudeLogWatcher;
pub use copilot_log_watcher::CopilotLogWatcher;
pub use file_watcher::{FileWatcher, FileChangeHandler, is_excluded};
