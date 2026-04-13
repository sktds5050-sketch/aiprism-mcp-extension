// File watcher implementation
use notify::{Watcher, RecursiveMode, Result as NotifyResult, EventKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::fs;

/// Excluded directories to ignore file changes
const EXCLUDE_DIRS: &[&str] = &[
    ".git", "target", "node_modules", ".venv",
    "__pycache__", "dist", "build", "workflow", ".claude",
];

/// Code file extensions to watch
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "cpp", "h",
    "cs", "rb", "swift", "kt", "scala", "php", "html", "css", "scss",
    "toml", "yaml", "yml", "md",
];

/// Check if a path is in an excluded directory
pub fn is_excluded(path: &Path) -> bool {
    path.components().any(|c| {
        if let Some(os_str) = c.as_os_str().to_str() {
            EXCLUDE_DIRS.contains(&os_str)
        } else {
            false
        }
    })
}

/// Check if a file has a code extension
pub fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| CODE_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

/// File change event callback trait
pub trait FileChangeHandler: Send + Sync {
    fn on_file_modified(&self, path: PathBuf);
}

/// File watcher for monitoring code changes
pub struct FileWatcher {
    source_roots: Vec<PathBuf>,
    handler: Arc<dyn FileChangeHandler>,
}

impl FileWatcher {
    pub fn new(source_roots: Vec<PathBuf>, handler: Arc<dyn FileChangeHandler>) -> Self {
        Self {
            source_roots,
            handler,
        }
    }

    pub async fn run(&self) -> NotifyResult<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);

        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.blocking_send(res);
        })?;

        // Watch all source roots recursively
        for root in &self.source_roots {
            if root.exists() {
                watcher.watch(root, RecursiveMode::Recursive)?;
            }
        }

        // Process events
        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => {
                    // Process modify, create, and remove events
                    let is_modify = matches!(event.kind, EventKind::Modify(_));
                    let is_create = matches!(event.kind, EventKind::Create(_));
                    let is_remove = matches!(event.kind, EventKind::Remove(_));
                    if is_modify || is_create || is_remove {
                        for path in event.paths {
                            if !is_excluded(&path) && is_code_file(&path) {
                                tracing::debug!(path = ?path, "File change detected");
                                self.handler.on_file_modified(path);
                            }
                        }
                    }
                }
                Err(_e) => {
                    // Ignore watch errors
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn excludes_target_directory() {
        assert!(is_excluded(Path::new("/proj/target/debug/foo.rs")));
    }

    #[test]
    fn excludes_node_modules() {
        assert!(is_excluded(Path::new("/proj/node_modules/pkg/index.js")));
    }

    #[test]
    fn excludes_git_directory() {
        assert!(is_excluded(Path::new("/proj/.git/objects/abc123")));
    }

    #[test]
    fn allows_non_excluded_paths() {
        assert!(!is_excluded(Path::new("/proj/src/main.rs")));
    }

    #[test]
    fn allows_filename_with_excluded_word() {
        // "target" in filename should not trigger exclusion
        assert!(!is_excluded(Path::new("/proj/src/target_utils.rs")));
    }

    #[test]
    fn excludes_venv() {
        assert!(is_excluded(Path::new("/proj/.venv/lib/site.py")));
    }

    #[test]
    fn excludes_pycache() {
        assert!(is_excluded(Path::new("/proj/__pycache__/module.cpython-39.pyc")));
    }

    // Mock handler for testing
    struct MockHandler {
        events: Arc<Mutex<Vec<PathBuf>>>,
    }

    impl MockHandler {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_events(&self) -> Vec<PathBuf> {
            self.events.lock().unwrap().clone()
        }
    }

    impl FileChangeHandler for MockHandler {
        fn on_file_modified(&self, path: PathBuf) {
            self.events.lock().unwrap().push(path);
        }
    }

    #[tokio::test]
    async fn watcher_detects_file_modification() {
        let dir = tempfile::tempdir().unwrap();
        let test_file = dir.path().join("test.rs");
        fs::write(&test_file, "fn main() {}").unwrap();

        let handler = Arc::new(MockHandler::new());
        let watcher = FileWatcher::new(vec![dir.path().to_path_buf()], handler.clone());

        // Start watcher in background
        let watcher_task = tokio::spawn(async move {
            let _ = watcher.run().await;
        });

        // Give watcher time to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Modify the file
        fs::write(&test_file, "fn main() { println!(\"hi\"); }").unwrap();

        // Give watcher time to detect change
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Cancel the watcher
        watcher_task.abort();

        let events = handler.get_events();
        assert!(!events.is_empty());
        assert!(events[0].ends_with("test.rs"));
    }

    #[tokio::test]
    async fn watcher_ignores_excluded_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target_dir = dir.path().join("target");
        fs::create_dir(&target_dir).unwrap();
        let target_file = target_dir.join("artifact.rlib");
        fs::write(&target_file, "binary").unwrap();

        let handler = Arc::new(MockHandler::new());
        let watcher = FileWatcher::new(vec![dir.path().to_path_buf()], handler.clone());

        let watcher_task = tokio::spawn(async move {
            let _ = watcher.run().await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Modify file in excluded directory
        fs::write(&target_file, "new binary").unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        watcher_task.abort();

        let events = handler.get_events();
        assert!(events.is_empty(), "Should not detect changes in excluded directories");
    }
}
