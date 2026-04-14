// Pair module - pair management and diff generation
pub mod diff;

use crate::models::{Pair, ActivePair, PairMetadata};
use crate::watcher::FileChangeHandler;
use crate::watcher::file_watcher::{is_excluded, is_code_file};
use crate::config::{DEFAULT_EXTENSIONS, DEFAULT_EXCLUDE_DIRS};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// Trait for handling completed pairs
pub trait CollectionManagerTrait: Send + Sync {
    fn submit(&self, pair: Pair);
}

/// Manages ActivePair lifecycle: creation → snapshot → quiet period → diff → submission
pub struct PairManager {
    active: Arc<Mutex<Option<ActivePair>>>,
    collection_manager: Arc<dyn CollectionManagerTrait>,
    quiet_period_secs: u64,
    cancel_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    source_roots: Vec<PathBuf>,
}

impl PairManager {
    pub fn new(
        collection_manager: Arc<dyn CollectionManagerTrait>,
        quiet_period_secs: u64,
        source_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
            collection_manager,
            quiet_period_secs,
            cancel_handle: Arc::new(Mutex::new(None)),
            source_roots,
        }
    }

    /// Collect snapshots of all code files in source_roots
    fn collect_snapshots(&self) -> HashMap<PathBuf, String> {
        let mut snapshots = HashMap::new();
        for root in &self.source_roots {
            collect_snapshots_recursive(root, &mut snapshots);
        }
        snapshots
    }

    /// Called when user sends a prompt
    pub async fn on_user_prompt(&self, metadata: PairMetadata, user_text: String) {
        // Cancel existing timer and flush previous pair if active
        if let Some(handle) = self.cancel_handle.lock().await.take() {
            handle.abort();
        }

        if let Some(pair) = self.active.lock().await.take() {
            if pair.quiet_timer_active {
                self.flush_pair(pair).await;
            }
        }

        // Register pair first so on_file_modified can track changes during snapshot collection
        let new_pair = ActivePair {
            request_id: uuid::Uuid::new_v4().to_string(), // Will be updated in on_completion
            user_query: user_text,
            ai_response: String::new(),
            context_file: metadata.context_file,
            log_file_path: metadata.log_file_path,
            source: metadata.source,
            timestamp: metadata.timestamp,
            model_id: metadata.model_id,
            snapshots: HashMap::new(),
            dirty_files: std::collections::HashSet::new(),
            last_activity: Instant::now(),
            quiet_timer_active: true,
        };
        *self.active.lock().await = Some(new_pair);

        // Collect snapshots after pair is registered
        let snapshots = self.collect_snapshots();
        if let Some(ref mut pair) = *self.active.lock().await {
            pair.snapshots = snapshots;
        }

        // Start idle timer
        self.spawn_quiet_timer().await;
    }

    /// Called when AI sends completion — save response only, timer already running
    pub async fn on_completion(&self, ai_text: String, request_id: String) {
        if let Some(ref mut pair) = *self.active.lock().await {
            pair.ai_response = ai_text;
            pair.request_id = request_id;
            pair.last_activity = Instant::now();
        }
    }

    /// Called when intermediate assistant line arrives — reset idle timer
    pub async fn on_assistant_line(&self) {
        let is_active = self.active.lock().await.is_some();
        if is_active {
            self.reset_quiet_timer().await;
        }
    }

    fn spawn_quiet_timer_task(
        active: Arc<Mutex<Option<ActivePair>>>,
        cm: Arc<dyn CollectionManagerTrait>,
        quiet_secs: u64,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(quiet_secs)).await;

            if let Some(pair) = active.lock().await.take() {
                if pair.quiet_timer_active {
                    let current: Vec<(PathBuf, String)> = pair.dirty_files.iter()
                        .filter_map(|p| std::fs::read_to_string(p).ok().map(|c| (p.clone(), c)))
                        .collect();
                    let code_changes = diff::generate_diff_from_content(&pair.snapshots, &current);
                    let completed = Pair {
                        source: pair.source,
                        request_id: pair.request_id,
                        timestamp: pair.timestamp,
                        model_id: pair.model_id,
                        user_query: pair.user_query,
                        ai_response: pair.ai_response,
                        context_file: pair.context_file,
                        code_changes,
                        log_file_path: pair.log_file_path,
                    };
                    cm.submit(completed);
                }
            }
        })
    }

    async fn spawn_quiet_timer(&self) {
        let handle = Self::spawn_quiet_timer_task(
            self.active.clone(),
            self.collection_manager.clone(),
            self.quiet_period_secs,
        );
        *self.cancel_handle.lock().await = Some(handle);
    }

    async fn reset_quiet_timer(&self) {
        if let Some(handle) = self.cancel_handle.lock().await.take() {
            handle.abort();
        }
        self.spawn_quiet_timer().await;
    }

    /// Called when a file is modified
    pub async fn on_file_modified(&self, path: &Path) {
        let mut active_lock = self.active.lock().await;

        if let Some(ref mut pair) = *active_lock {
            pair.dirty_files.insert(path.to_path_buf());
            pair.last_activity = Instant::now();
            drop(active_lock);

            self.reset_quiet_timer().await;
        }
    }

    async fn flush_pair(&self, pair: ActivePair) {
        let current: Vec<(PathBuf, String)> = pair.dirty_files.iter()
            .filter_map(|p| std::fs::read_to_string(p).ok().map(|c| (p.clone(), c)))
            .collect();
        let code_changes = diff::generate_diff_from_content(&pair.snapshots, &current);

        let completed = Pair {
            source: pair.source,
            request_id: pair.request_id,
            timestamp: pair.timestamp,
            model_id: pair.model_id,
            user_query: pair.user_query,
            ai_response: pair.ai_response,
            context_file: pair.context_file,
            code_changes,
            log_file_path: pair.log_file_path,
        };
        self.collection_manager.submit(completed);
    }
}

/// Adapter for FileChangeHandler that bridges sync and async
pub struct FileChangeHandlerAdapter {
    tx: tokio::sync::mpsc::UnboundedSender<PathBuf>,
}

impl FileChangeHandlerAdapter {
    pub fn new(pair_manager: Arc<PairManager>) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, mut rx): (tokio::sync::mpsc::UnboundedSender<PathBuf>, tokio::sync::mpsc::UnboundedReceiver<PathBuf>) =
            tokio::sync::mpsc::unbounded_channel();

        let task = tokio::spawn(async move {
            while let Some(path) = rx.recv().await {
                pair_manager.on_file_modified(&path).await;
            }
        });

        (Self { tx }, task)
    }
}

impl FileChangeHandler for FileChangeHandlerAdapter {
    fn on_file_modified(&self, path: PathBuf) {
        let _ = self.tx.send(path);
    }
}

/// Events from LogWatcher → PairManager
enum LogEvent {
    UserPrompt(PairMetadata, String),
    Completion(String, String), // (ai_text, request_id)
    AssistantLine,
}

/// Adapter: sync PairManagerTrait → async PairManager via mpsc channel
pub struct PairManagerAdapter {
    tx: tokio::sync::mpsc::UnboundedSender<LogEvent>,
}

impl PairManagerAdapter {
    pub fn new(pair_manager: Arc<PairManager>) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LogEvent>();

        let task = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    LogEvent::UserPrompt(metadata, text) => {
                        pair_manager.on_user_prompt(metadata, text).await;
                    }
                    LogEvent::Completion(text, request_id) => {
                        pair_manager.on_completion(text, request_id).await;
                    }
                    LogEvent::AssistantLine => {
                        pair_manager.on_assistant_line().await;
                    }
                }
            }
        });

        (Self { tx }, task)
    }
}

/// Recursively collect snapshots of code files under a directory
fn collect_snapshots_recursive(dir: &PathBuf, snapshots: &mut HashMap<PathBuf, String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let exclude_dirs: Vec<String> = DEFAULT_EXCLUDE_DIRS.iter().map(|s| s.to_string()).collect();
        let extensions: Vec<String> = DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect();
        if is_excluded(&path, &exclude_dirs) {
            continue;
        }
        if path.is_dir() {
            collect_snapshots_recursive(&path, snapshots);
        } else if path.is_file() && is_code_file(&path, &extensions) {
            // Skip files larger than 1MB
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.len() > 1_000_000 {
                    continue;
                }
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                snapshots.insert(path, content);
            }
        }
    }
}

impl crate::watcher::PairManagerTrait for PairManagerAdapter {
    fn on_user_prompt(&self, metadata: PairMetadata, text: String) {
        let _ = self.tx.send(LogEvent::UserPrompt(metadata, text));
    }

    fn on_completion(&self, text: String, request_id: String) {
        let _ = self.tx.send(LogEvent::Completion(text, request_id));
    }

    fn on_assistant_line(&self) {
        let _ = self.tx.send(LogEvent::AssistantLine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn metadata() -> PairMetadata {
        PairMetadata {
            timestamp: 1700000000,
            model_id: "claude-3-sonnet".to_string(),
            context_file: "/proj".to_string(),
            log_file_path: PathBuf::from("/proj/.claude/conversations.jsonl"),
            source: "claudecode".to_string(),
        }
    }

    struct MockCollectionManager {
        pairs: Arc<Mutex<Vec<Pair>>>,
        sender: mpsc::UnboundedSender<Pair>,
    }

    impl MockCollectionManager {
        fn new() -> (Self, mpsc::UnboundedReceiver<Pair>) {
            let (tx, rx) = mpsc::unbounded_channel();
            (
                Self {
                    pairs: Arc::new(Mutex::new(Vec::new())),
                    sender: tx,
                },
                rx,
            )
        }
    }

    impl CollectionManagerTrait for MockCollectionManager {
        fn submit(&self, pair: Pair) {
            let _ = self.sender.send(pair);
        }
    }

    #[tokio::test]
    async fn flushes_pair_after_quiet_period() {
        let (mock_cm, mut rx) = MockCollectionManager::new();
        let pm = PairManager::new(Arc::new(mock_cm), 1, vec![]);

        pm.on_user_prompt(metadata(), "질문".to_string()).await;
        pm.on_completion("답변".to_string(), "req-1".to_string()).await;

        let pair = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .expect("timeout")
            .expect("no pair");
        assert_eq!(pair.user_query, "질문");
        assert_eq!(pair.request_id, "req-1");
    }

    #[tokio::test]
    async fn second_user_prompt_flushes_previous_pair() {
        let (mock_cm, mut rx) = MockCollectionManager::new();
        let pm = PairManager::new(Arc::new(mock_cm), 60, vec![]); // Long timer

        pm.on_user_prompt(metadata(), "첫질문".to_string()).await;
        pm.on_completion("첫답변".to_string(), "req-1".to_string()).await;

        // Second user prompt before timer expires
        pm.on_user_prompt(metadata(), "두번째질문".to_string()).await;

        // Previous pair should flush immediately
        let pair = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout")
            .expect("no pair");
        assert_eq!(pair.user_query, "첫질문");
    }

    #[tokio::test]
    async fn file_modify_resets_quiet_timer() {
        let (mock_cm, mut rx) = MockCollectionManager::new();
        let pm = PairManager::new(Arc::new(mock_cm), 1, vec![]);

        pm.on_user_prompt(metadata(), "q".to_string()).await;
        pm.on_completion("a".to_string(), "req-1".to_string()).await;

        // Wait 0.8 seconds then modify file
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        pm.on_file_modified(Path::new("/proj/src/main.rs")).await;

        // Should not flush immediately after reset
        assert!(rx.try_recv().is_err());

        // Should flush after additional 1+ seconds
        let pair = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout")
            .expect("no pair");
        assert_eq!(pair.user_query, "q");
    }
}
