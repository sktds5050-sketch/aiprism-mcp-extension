// Copilot log watcher - completion leads pair boundary, with pending user text buffering
use crate::models::PairMetadata;
use crate::strategy::{LogParsingStrategy, log::CopilotStrategy};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use super::log_watcher::{OffsetStore, PairManagerTrait};

/// CopilotLogWatcher: Copilot 응답시간 기반 추론
/// - kind==1 (result) 감지 → pending_user_texts에 저장 (이벤트 없음)
/// - kind==2 (response) 감지 → pending에서 user_text 꺼내 Pair 구성
pub struct CopilotLogWatcher {
    file_path: PathBuf,
    strategy: Arc<CopilotStrategy>,
    pair_manager: Arc<dyn PairManagerTrait>,
    offsets: OffsetStore,
    pub is_initial_load: bool,

    // Copilot 전용: N(request index) → user_text 버퍼
    pending_user_texts: HashMap<String, String>,
}

impl CopilotLogWatcher {
    pub fn new(
        file_path: PathBuf,
        strategy: Arc<CopilotStrategy>,
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
            pending_user_texts: HashMap::new(),
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
        let mut result_count = 0;
        let mut response_count = 0;

        for line in reader.lines() {
            let line = line?;
            line_count += 1;
            new_offset += (line.len() as u64) + 1;

            if line_count % 100 == 0 {
                tracing::trace!(line_count = line_count, "Processed lines");
            }

            // kind==1 (user result): pending 버퍼에 저장
            if self.strategy.is_user_prompt(&line) {
                result_count += 1;

                if let Some(request_id) = self.strategy.extract_request_id(&line) {
                    if let Some(user_text) = self.strategy.extract_user_text(&line) {
                        tracing::trace!(
                            request_id = ?request_id,
                            user_text = ?user_text.chars().take(100).collect::<String>(),
                            "Copilot: User text buffered for request"
                        );
                        self.pending_user_texts.insert(request_id, user_text);
                    } else {
                        tracing::warn!("Copilot: User text extraction failed from result line");
                    }
                } else {
                    tracing::warn!("Copilot: request_id extraction failed from result line");
                }
            }

            // kind==2 (AI response): pending에서 user_text 꺼내 Pair 구성
            if self.strategy.is_completion_signal(&line) {
                response_count += 1;

                if self.is_initial_load {
                    // 초기 로드 중에는 이벤트 발생 안 함
                    continue;
                }

                if let Some(request_id) = self.strategy.extract_request_id(&line) {
                    // pending에서 user_text 꺼내기
                    let user_text = self.pending_user_texts.remove(&request_id)
                        .unwrap_or_else(|| {
                            tracing::warn!(
                                request_id = ?request_id,
                                "Copilot: No pending user text found for request, using empty"
                            );
                            String::new()
                        });

                    if let Some(ai_text) = self.strategy.extract_ai_text(&line) {
                        if let Some(mut metadata) = self.strategy.extract_metadata(&line) {
                            metadata.log_file_path = self.file_path.clone();

                            tracing::info!(
                                request_id = ?request_id,
                                user_text = ?user_text.chars().take(100).collect::<String>(),
                                ai_text = ?ai_text.chars().take(100).collect::<String>(),
                                "Copilot: Pair assembled, triggering on_user_prompt + on_completion"
                            );

                            // Pair 시작 (user_text와 함께)
                            self.pair_manager.on_user_prompt(metadata, user_text);
                            // Pair 즉시 완성 (quiet period 타이머 시작)
                            self.pair_manager.on_completion(ai_text, request_id.clone());
                        } else {
                            tracing::warn!("Copilot: Metadata extraction failed");
                        }
                    } else {
                        tracing::warn!("Copilot: AI text extraction failed from response line");
                    }
                } else {
                    tracing::warn!("Copilot: request_id extraction failed from response line");
                }
            }
        }

        if line_count > 0 {
            tracing::trace!(
                path = ?self.file_path,
                total_lines = line_count,
                result_lines = result_count,
                response_lines = response_count,
                pending_count = self.pending_user_texts.len(),
                "Copilot: Batch processing complete"
            );
        }

        if new_offset > current_offset {
            self.offsets.set_offset(self.file_path.clone(), new_offset);
            tracing::trace!(new_offset = new_offset, "Copilot: Offset updated");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        fn on_assistant_line(&self) {}
    }

    #[tokio::test]
    async fn buffers_user_text_and_pairs_on_response() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("copilot.jsonl");
        fs::File::create(&log_path).unwrap();

        let pair_manager = Arc::new(MockPairManager::new());
        let strategy = Arc::new(CopilotStrategy);
        let offsets = OffsetStore::new();

        let mut watcher = CopilotLogWatcher::new(log_path.clone(), strategy, pair_manager.clone(), offsets);
        watcher.is_initial_load = false;

        // Write user result line (kind==1)
        let user_line = r#"{"kind":1,"k":["requests",0,"result"],"v":{"metadata":{"renderedUserMessage":[{"type":1,"text":"<userRequest>\n사용자 질문\n</userRequest>"}]}}}"#;
        let mut f = OpenOptions::new().append(true).open(&log_path).unwrap();
        writeln!(f, "{}", user_line).unwrap();
        drop(f);

        watcher.process_file().await.unwrap();

        // Write response line (kind==2)
        let response_line = r#"{"kind":2,"k":["requests",0,"response"],"v":[{"value":"AI 응답"}]}"#;
        let mut f = OpenOptions::new().append(true).open(&log_path).unwrap();
        writeln!(f, "{}", response_line).unwrap();
        drop(f);

        watcher.process_file().await.unwrap();

        let events = pair_manager.get_events();
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("user:사용자 질문"));
        assert!(events[1].contains("complete:AI 응답"));
    }
}
