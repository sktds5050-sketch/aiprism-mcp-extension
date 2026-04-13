// Windows path strategy
use crate::strategy::PathStrategy;
use std::path::PathBuf;

pub struct WindowsPathStrategy;

impl PathStrategy for WindowsPathStrategy {
    fn log_directories(&self) -> Vec<(String, PathBuf)> {
        let mut dirs = Vec::new();
        if let Some(app_data) = dirs::config_dir() {
            dirs.push((
                "claudecode".into(),
                app_data.join("Claude/projects"),
            ));
            dirs.push((
                "GitHub Copilot".into(),
                app_data.join("Code/User/workspaceStorage"),
            ));
        }
        dirs
    }

    fn offset_store_path(&self) -> PathBuf {
        dirs::config_dir()
            .map(|d| d.join(".aiprism/offsets.json"))
            .unwrap_or_else(|| PathBuf::from(".aiprism/offsets.json"))
    }

    fn registry_store_path(&self) -> PathBuf {
        dirs::config_dir()
            .map(|d| d.join(".aiprism/registry.json"))
            .unwrap_or_else(|| PathBuf::from(".aiprism/registry.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_offset_path_valid() {
        let strategy = WindowsPathStrategy;
        let path = strategy.offset_store_path();
        let path_str = path.to_str().unwrap();
        assert!(path_str.contains(".aiprism") && path_str.contains("offsets.json"));
    }

    #[test]
    fn windows_registry_path_valid() {
        let strategy = WindowsPathStrategy;
        let path = strategy.registry_store_path();
        let path_str = path.to_str().unwrap();
        assert!(path_str.contains(".aiprism") && path_str.contains("registry.json"));
    }
}
