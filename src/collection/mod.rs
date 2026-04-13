// Collection module - collection management and registry
use crate::models::{Pair, PairPayload, CollectionState};
use crate::network::sender::SenderTrait;
use crate::pair::CollectionManagerTrait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use chrono::Local;

/// CollectionManager: Manages collection_id registry and pair submission
pub struct CollectionManager {
    registry: Arc<Mutex<HashMap<PathBuf, CollectionState>>>,
    registry_path: PathBuf,
    sender: Arc<dyn SenderTrait>,
}

impl CollectionManager {
    pub async fn new(
        registry_path: PathBuf,
        sender: Arc<dyn SenderTrait>,
    ) -> Result<Self, std::io::Error> {
        let registry = Self::load_registry(&registry_path).await?;
        Ok(Self {
            registry: Arc::new(Mutex::new(registry)),
            registry_path,
            sender,
        })
    }

    /// Public async submit method
    pub async fn submit(&self, pair: Pair) -> Result<(), String> {
        self.submit_async(pair).await
    }

    /// Load registry from JSON file, or return empty HashMap if not found
    async fn load_registry(path: &PathBuf) -> Result<HashMap<PathBuf, CollectionState>, std::io::Error> {
        if !path.exists() {
            return Ok(HashMap::new());
        }

        let content = tokio::fs::read_to_string(path).await?;
        match serde_json::from_str::<HashMap<PathBuf, CollectionState>>(&content) {
            Ok(registry) => Ok(registry),
            Err(_) => Ok(HashMap::new()), // If JSON parse fails, start fresh
        }
    }

    /// Save registry to JSON file
    async fn save_registry(&self) -> Result<(), std::io::Error> {
        let registry = self.registry.lock().await;
        let json = serde_json::to_string(&*registry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        // Ensure parent directory exists
        if let Some(parent) = self.registry_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&self.registry_path, json).await?;
        Ok(())
    }

    /// Submit a pair and manage collection state (async version)
    async fn submit_async(&self, pair: Pair) -> Result<(), String> {
        let log_file_path = pair.log_file_path.clone();
        let registry = self.registry.lock().await;

        // Check if this log file is already in a collection
        let is_new = !registry.contains_key(&log_file_path);
        let collection_state = registry.get(&log_file_path).cloned();

        drop(registry); // Release lock before async send

        let payload = if let Some(state) = collection_state {
            // Existing collection: include collection_id, no tags/title
            PairPayload {
                user_query: pair.user_query,
                ai_response: format!("{}\n\n{}", pair.ai_response, pair.code_changes),
                project_path: pair.context_file,
                collection_id: state.collection_id,
                tags: None,
                title: None,
            }
        } else {
            // New collection: include tags and title
            let agent_tag = pair.source.clone();
            let title = format!("{} {}", agent_tag, Local::now().to_rfc3339());

            PairPayload {
                user_query: pair.user_query,
                ai_response: format!("{}\n\n{}", pair.ai_response, pair.code_changes),
                project_path: pair.context_file,
                collection_id: None,
                tags: Some(vec![agent_tag]),
                title: Some(title),
            }
        };

        // Send to remote server
        let collection_id = self.sender.send(&payload).await?;

        // Update registry if new collection
        if is_new {
            let mut registry = self.registry.lock().await;
            registry.insert(
                log_file_path,
                CollectionState {
                    collection_id: Some(collection_id),
                    source: pair.source,
                },
            );
            drop(registry);
            self.save_registry().await.map_err(|e| e.to_string())?;
        }

        Ok(())
    }
}

/// Implement CollectionManagerTrait for CollectionManager (sync wrapper)
impl CollectionManagerTrait for CollectionManager {
    fn submit(&self, pair: Pair) {
        let registry = self.registry.clone();
        let registry_path = self.registry_path.clone();
        let sender = self.sender.clone();

        tokio::spawn(async move {
            let cm = CollectionManager { registry, registry_path, sender };
            if let Err(e) = cm.submit_async(pair).await {
                tracing::error!("Failed to submit pair: {}", e);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    struct MockSender {
        sender: mpsc::UnboundedSender<PairPayload>,
        returns_id: u64,
    }

    impl MockSender {
        fn new(returns_id: u64) -> (Self, mpsc::UnboundedReceiver<PairPayload>) {
            let (tx, rx) = mpsc::unbounded_channel();
            (
                Self {
                    sender: tx,
                    returns_id,
                },
                rx,
            )
        }
    }

    #[async_trait::async_trait]
    impl SenderTrait for MockSender {
        async fn send(&self, payload: &PairPayload) -> Result<u64, String> {
            let _ = self.sender.send(payload.clone());
            Ok(self.returns_id)
        }
    }

    fn make_pair(log_path: &str, source: &str) -> Pair {
        Pair {
            source: source.to_string(),
            request_id: "req-1".to_string(),
            timestamp: 1700000000,
            model_id: "claude-3-sonnet".to_string(),
            user_query: "question".to_string(),
            ai_response: "answer".to_string(),
            context_file: "/proj".to_string(),
            code_changes: "".to_string(),
            log_file_path: PathBuf::from(log_path),
        }
    }

    #[tokio::test]
    async fn new_log_file_sends_payload_with_tags_and_title() {
        let (mock_sender, mut rx) = MockSender::new(1239);
        let dir = tempfile::tempdir().unwrap();

        let cm = CollectionManager::new(
            dir.path().join("registry.json"),
            Arc::new(mock_sender),
        )
        .await
        .unwrap();

        let pair = make_pair("/logs/new.jsonl", "claudecode");
        cm.submit(pair).await.unwrap();

        let payload = rx.recv().await.unwrap();
        assert!(payload.tags.is_some());
        assert!(payload.title.is_some());
        assert!(payload
            .title
            .as_ref()
            .unwrap()
            .starts_with("claudecode"));
        assert_eq!(payload.collection_id, None);
    }

    #[tokio::test]
    async fn existing_log_file_sends_payload_without_tags() {
        let (mock_sender, mut rx) = MockSender::new(1239);
        let dir = tempfile::tempdir().unwrap();

        let cm = CollectionManager::new(
            dir.path().join("registry.json"),
            Arc::new(mock_sender),
        )
        .await
        .unwrap();

        let pair1 = make_pair("/logs/existing.jsonl", "claudecode");
        cm.submit(pair1).await.unwrap();
        let _ = rx.recv().await.unwrap(); // Consume first payload

        let pair2 = make_pair("/logs/existing.jsonl", "claudecode");
        cm.submit(pair2).await.unwrap();

        let payload = rx.recv().await.unwrap();
        assert_eq!(payload.collection_id, Some(1239));
        assert!(payload.tags.is_none());
        assert!(payload.title.is_none());
    }

    #[tokio::test]
    async fn registry_persists_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = dir.path().join("registry.json");

        // First run: submit and save registry
        {
            let (mock_sender, mut _rx) = MockSender::new(1239);
            let cm = CollectionManager::new(registry_path.clone(), Arc::new(mock_sender))
                .await
                .unwrap();

            let pair = make_pair("/logs/a.jsonl", "claudecode");
            cm.submit(pair).await.unwrap();
        }

        // Second run: load registry and reuse collection_id
        {
            let (mock_sender, mut rx) = MockSender::new(9999); // Different ID but shouldn't be used
            let cm = CollectionManager::new(registry_path, Arc::new(mock_sender))
                .await
                .unwrap();

            let pair = make_pair("/logs/a.jsonl", "claudecode");
            cm.submit(pair).await.unwrap();

            let payload = rx.recv().await.unwrap();
            assert_eq!(payload.collection_id, Some(1239)); // Reuse saved ID
        }
    }
}
