// Linux path strategy
use crate::strategy::PathStrategy;
use std::path::PathBuf;

pub struct LinuxPathStrategy;

impl PathStrategy for LinuxPathStrategy {
    fn log_directories(&self) -> Vec<(String, PathBuf)> {
        let mut dirs = Vec::new();
        if let Some(home) = dirs::home_dir() {
            dirs.push((
                "claudecode".into(),
                home.join(".claude/projects"),
            ));
            dirs.push((
                "GitHub Copilot".into(),
                home.join(".config/Code/User/workspaceStorage"),
            ));
        }
        dirs
    }

    fn offset_store_path(&self) -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(".aiprism/offsets.json"))
            .unwrap_or_else(|| PathBuf::from(".aiprism/offsets.json"))
    }

    fn registry_store_path(&self) -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(".aiprism/registry.json"))
            .unwrap_or_else(|| PathBuf::from(".aiprism/registry.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_offset_path_under_home() {
        let strategy = LinuxPathStrategy;
        let path = strategy.offset_store_path();
        assert!(path.to_str().unwrap().contains(".aiprism/offsets.json"));
    }

    #[test]
    fn linux_registry_path_under_home() {
        let strategy = LinuxPathStrategy;
        let path = strategy.registry_store_path();
        assert!(path.to_str().unwrap().contains(".aiprism/registry.json"));
    }

    #[test]
    fn linux_log_directories_contains_both() {
        let strategy = LinuxPathStrategy;
        let dirs = strategy.log_directories();
        let names: Vec<_> = dirs.iter().map(|(name, _)| name.clone()).collect();
        assert!(names.contains(&"claudecode".to_string()));
        assert!(names.contains(&"GitHub Copilot".to_string()));
    }
}
