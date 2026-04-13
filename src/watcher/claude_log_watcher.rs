// Claude log watcher - user prompt leads pair boundary
use crate::strategy::LogParsingStrategy;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use super::log_watcher::{OffsetStore, PairManagerTrait};

/// ClaudeLogWatcher: 단일 Claude 로그 파일을 tail하고 이벤트 발생
pub struct ClaudeLogWatcher {
    file_path: PathBuf,
    strategy: Arc<dyn LogParsingStrategy>,
    pair_manager: Arc<dyn PairManagerTrait>,
    offsets: OffsetStore,
    pub is_initial_load: bool,
    pending_ai_text: String,
}

impl ClaudeLogWatcher {
    pub fn new(
        file_path: PathBuf,
        strategy: Arc<dyn LogParsingStrategy>,
        pair_manager: Arc<dyn PairManagerTrait>,
        offsets: OffsetStore,
    ) -> Self {
        let is_initial_load = offsets.get_offset(&file_path) == 0;

        Self {
            file_path,
            strategy,
            pair_manager,
            offsets,
            is_initial_load,
            pending_ai_text: String::new(),
        }
    }

    pub async fn run(&mut self, offset_store_path: &PathBuf) -> std::io::Result<()> {

        loop {
            self.process_file().await?;
            self.offsets.save_to_file(offset_store_path)?;
            tracing::trace!(path = ?self.file_path, "Offset saved");

            if self.is_initial_load {
                self.is_initial_load = false;
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn process_file(&mut self) -> std::io::Result<()> {
        if !self.file_path.exists() {
            tracing::trace!(path = ?self.file_path, "Log file does not exist");
            return Ok(());
        }

        let mut file = OpenOptions::new().read(true).open(&self.file_path)?;
        let current_offset = self.offsets.get_offset(&self.file_path);

        tracing::trace!(path = ?self.file_path, offset = current_offset, "Opening log file");

        if current_offset > 0 {
            file.seek(SeekFrom::Start(current_offset))?;
        }

        let reader = BufReader::new(file);
        let mut new_offset = current_offset;
        let mut line_count = 0;
        let mut user_prompt_count = 0;
        let mut completion_count = 0;

        for line in reader.lines() {
            let line = line?;
            line_count += 1;
            new_offset += (line.len() as u64) + 1;

            if line_count % 100 == 0 {
                tracing::trace!(line_count = line_count, "Processed lines");
            }

            let is_user = self.strategy.is_user_prompt(&line);
            let is_completion = self.strategy.is_completion_signal(&line);

            if is_completion {
                completion_count += 1;
                if !self.is_initial_load {
                    // completion signal = next user(human) message
                    // flush accumulated ai text from previous assistant lines BEFORE starting new pair
                    let ai_text = std::mem::take(&mut self.pending_ai_text);
                    let request_id = uuid::Uuid::new_v4().to_string();
                    tracing::info!(
                        ai_text_len = ai_text.len(),
                        request_id = ?request_id,
                        "Claude: completion signal (user line), flushing ai_text"
                    );
                    self.pair_manager.on_completion(ai_text, request_id);
                }
            }

            if is_user {
                user_prompt_count += 1;
                if !self.is_initial_load {
                    if let Some(mut metadata) = self.strategy.extract_metadata(&line) {
                        metadata.log_file_path = self.file_path.clone();

                        if let Some(text) = self.strategy.extract_user_text(&line) {
                            tracing::info!(
                                user_text = ?text.chars().take(200).collect::<String>(),
                                "Claude: User prompt extracted, calling on_user_prompt"
                            );
                            self.pair_manager.on_user_prompt(metadata, text);
                        } else {
                            tracing::warn!("Claude: User text extraction failed");
                        }
                    } else {
                        tracing::warn!("Claude: Metadata extraction failed for user prompt");
                    }
                }
            }

            if !is_completion && line.contains("\"type\":\"assistant\"") {
                if !self.is_initial_load {
                    if let Some(text) = self.strategy.extract_ai_text(&line) {
                        self.pending_ai_text.push_str(&text);
                    }
                    tracing::trace!("Claude: intermediate assistant line, resetting idle timer");
                    self.pair_manager.on_assistant_line();
                }
            }
        }

        if line_count > 0 {
            tracing::trace!(
                path = ?self.file_path,
                total_lines = line_count,
                user_prompts = user_prompt_count,
                completions = completion_count,
                "Claude: Batch processing complete"
            );
        }

        if new_offset > current_offset {
            self.offsets.set_offset(self.file_path.clone(), new_offset);
            tracing::trace!(new_offset = new_offset, "Claude: Offset updated");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PairMetadata;
    use std::fs;
    use std::io::Write;
    use std::sync::Mutex;

    struct MockPairManager {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl MockPairManager {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl PairManagerTrait for MockPairManager {
        fn on_user_prompt(&self, _metadata: PairMetadata, text: String) {
            self.events.lock().unwrap().push(format!("user:{}", text));
        }

        fn on_completion(&self, text: String, _request_id: String) {
            self.events.lock().unwrap().push(format!("complete:{}", text));
        }

        fn on_assistant_line(&self) {
            self.events.lock().unwrap().push("assistant_line".to_string());
        }
    }

    #[tokio::test]
    async fn detects_user_prompt_on_new_line() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("test.jsonl");

        // initial_load를 건너뛰기 위해 파일에 내용을 먼저 쓰고 offset을 앞으로 설정
        let user_line = r#"{"type":"human","uuid":"u1","timestamp":"2026-04-03T12:31:08.189Z","message":{"role":"user","content":[{"type":"text","text":"테스트"}]},"cwd":"/proj"}"#;
        fs::write(&log_path, format!("{}\n", user_line)).unwrap();

        let pair_manager = Arc::new(MockPairManager::new());
        let strategy = Arc::new(crate::strategy::log::ClaudeStrategy);
        let mut offsets = OffsetStore::new();
        // offset을 0보다 크게 설정하여 initial_load=false로 만들고, 다시 처음부터 읽기 위해 0으로 리셋
        offsets.set_offset(log_path.clone(), 1);
        offsets.set_offset(log_path.clone(), 0);

        let mut watcher = ClaudeLogWatcher::new(log_path.clone(), strategy, pair_manager.clone(), offsets);
        // is_initial_load를 false로 만들기 위해 빈 파일로 초기 로드 완료
        watcher.is_initial_load = false;

        watcher.process_file().await.unwrap();

        let events = pair_manager.get_events();
        // user 라인 = is_user_prompt (pair 시작) + is_completion_signal (이전 pair 완료) 둘 다 트리거
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|e| e.contains("user:테스트")));
        assert!(events.iter().any(|e| e.starts_with("complete:")));
    }

    #[tokio::test]
    async fn intermediate_assistant_line_triggers_idle_reset() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("assistant.jsonl");

        let assistant_line = r#"{"type":"assistant","uuid":"a1","message":{"role":"assistant","content":[{"type":"text","text":"중간 응답"}]},"stop_reason":null}"#;
        fs::write(&log_path, format!("{}\n", assistant_line)).unwrap();

        let pair_manager = Arc::new(MockPairManager::new());
        let strategy = Arc::new(crate::strategy::log::ClaudeStrategy);
        let offsets = OffsetStore::new();

        let mut watcher = ClaudeLogWatcher::new(log_path.clone(), strategy, pair_manager.clone(), offsets);
        watcher.is_initial_load = false;

        watcher.process_file().await.unwrap();

        let events = pair_manager.get_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], "assistant_line");
    }

    #[tokio::test]
    async fn user_line_triggers_completion_and_user_prompt() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("completion.jsonl");

        // user 라인 = completion signal + user_prompt 둘 다 트리거
        let user_line = r#"{"type":"human","uuid":"u1","timestamp":"2026-04-03T12:31:08.189Z","message":{"role":"user","content":[{"type":"text","text":"완료 후 질문"}]},"cwd":"/proj"}"#;
        fs::write(&log_path, format!("{}\n", user_line)).unwrap();

        let pair_manager = Arc::new(MockPairManager::new());
        let strategy = Arc::new(crate::strategy::log::ClaudeStrategy);
        let offsets = OffsetStore::new();

        let mut watcher = ClaudeLogWatcher::new(log_path.clone(), strategy, pair_manager.clone(), offsets);
        watcher.is_initial_load = false;

        watcher.process_file().await.unwrap();

        let events = pair_manager.get_events();
        // is_user_prompt + is_completion_signal 둘 다 true → user + complete 이벤트
        assert!(events.iter().any(|e| e.starts_with("user:")));
        assert!(events.iter().any(|e| e.starts_with("complete:")));
    }
}
