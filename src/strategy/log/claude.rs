// Claude log parsing strategy
use crate::models::PairMetadata;
use crate::strategy::LogParsingStrategy;
use std::path::PathBuf;

// ISO8601 문자열을 Unix timestamp(u64)로 변환
// 예: "2026-04-03T12:31:08.189Z" → 1748937068
fn parse_iso8601_to_unix(iso_str: &str) -> Option<u64> {
    // 간단한 파서: YYYY-MM-DDTHH:MM:SS.sssZ 형식만 지원
    use chrono::DateTime;
    let dt = DateTime::parse_from_rfc3339(iso_str).ok()?;
    Some(dt.timestamp() as u64)
}

pub struct ClaudeStrategy;

#[async_trait::async_trait]
impl LogParsingStrategy for ClaudeStrategy {
    fn agent_name(&self) -> &str {
        "claudecode"
    }

    fn target_extension(&self) -> &str {
        ".jsonl"
    }

    fn is_user_prompt(&self, line: &str) -> bool {
        // 빠른 사전 필터: 최소 조건 미충족 시 JSON 파싱 생략
        if !line.contains("\"type\":\"human\"") && !line.contains("\"type\":\"user\"") {
            return false;
        }

        // JSON 파싱 후 message.content[] 배열에 type:text 블록이 실제로 있는지 확인
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return false,
        };

        let top_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if top_type != "human" && top_type != "user" {
            return false;
        }

        let result = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().any(|item| {
                if item.get("type").and_then(|t| t.as_str()) != Some("text") {
                    return false;
                }
                let t = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
                !t.starts_with("<ide_opened_file>")
                    && !t.starts_with("<ide_selection>")
                    && !t.starts_with("<user-prompt-submit-hook>")
            }))
            .unwrap_or(false);

        if result {
            tracing::trace!("Claude user prompt matched");
        }
        result
    }

    fn is_completion_signal(&self, line: &str) -> bool {
        self.is_user_prompt(line)
    }

    fn extract_request_id(&self, line: &str) -> Option<String> {
        if !line.contains("\"type\":\"assistant\"") {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        v.get("uuid")?.as_str().map(|s| s.to_string())
    }

    fn extract_metadata(&self, line: &str) -> Option<PairMetadata> {
        if !self.is_user_prompt(line) {
            return None;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(val) => val,
            Err(e) => {
                tracing::warn!(error = ?e, "JSON parse failed");
                return None;
            }
        };

        let context_file = match v.get("cwd")?.as_str() {
            Some(cwd) => cwd.to_string(),
            None => {
                tracing::warn!("cwd not found in Claude user prompt");
                return None;
            }
        };
        
        let timestamp = match parse_iso8601_to_unix(v.get("timestamp")?.as_str()?) {
            Some(ts) => ts,
            None => {
                tracing::warn!("timestamp parse failed or not ISO8601 format");
                return None;
            }
        };
        
        let model_id = v
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
            .to_string();

        tracing::trace!(
            cwd = ?context_file,
            timestamp = timestamp,
            model_id = ?model_id,
            "Claude metadata extracted"
        );

        Some(PairMetadata {
            timestamp,
            model_id,
            context_file,
            log_file_path: PathBuf::new(), // Set by watcher
            source: self.agent_name().to_string(),
        })
    }

    fn extract_user_text(&self, line: &str) -> Option<String> {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(val) => val,
            Err(e) => {
                tracing::warn!(error = ?e, "JSON parse failed in extract_user_text");
                return None;
            }
        };
        
        let content = match v
            .get("message")?
            .get("content")?
            .as_array() {
            Some(arr) => arr,
            None => {
                tracing::warn!("content is not an array in extract_user_text");
                return None;
            }
        };

        let mut text = String::new();
        for item in content {
            match item.get("type")?.as_str() {
                Some("text") => {
                    if let Some(t) = item.get("text")?.as_str() {
                        // IDE 컨텍스트 태그 블록은 유저 입력이 아니므로 제외
                        if t.starts_with("<ide_opened_file>")
                            || t.starts_with("<ide_selection>")
                            || t.starts_with("<user-prompt-submit-hook>")
                        {
                            tracing::trace!("Skipping IDE context block");
                            continue;
                        }
                        text.push_str(t);
                    }
                }
                Some(other) => {
                    tracing::trace!(content_type = other, "Skipping non-text content");
                }
                None => {}
            }
        }

        if text.is_empty() {
            tracing::trace!("No text content found after filtering");
            None
        } else {
            tracing::trace!(text_len = text.len(), "User text extracted");
            Some(text)
        }
    }

    fn extract_ai_text(&self, line: &str) -> Option<String> {
        if !line.contains("\"type\":\"assistant\"") {
            return None;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(val) => val,
            Err(e) => {
                tracing::warn!(error = ?e, "JSON parse failed in extract_ai_text");
                return None;
            }
        };
        
        let content = match v
            .get("message")?
            .get("content")?
            .as_array() {
            Some(arr) => arr,
            None => {
                tracing::warn!("content is not an array in extract_ai_text");
                return None;
            }
        };

        let mut text = String::new();
        for item in content {
            match item.get("type")?.as_str() {
                Some("text") => {
                    if let Some(t) = item.get("text")?.as_str() {
                        text.push_str(t);
                    }
                }
                Some(other) => {
                    tracing::trace!(content_type = other, "Skipping non-text content in AI response");
                }
                None => {}
            }
        }

        if text.is_empty() {
            tracing::trace!("No text content found in AI response");
            None
        } else {
            tracing::trace!(text_len = text.len(), "AI text extracted");
            Some(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER_LINE: &str = r#"{"type":"human","uuid":"u1","timestamp":"2026-04-03T12:31:08.189Z","message":{"role":"user","content":[{"type":"text","text":"리팩토링 해줘"},{"type":"tool_result","text":"..."}]},"cwd":"/proj"}"#;
    const ASSISTANT_LINE: &str = r#"{"type":"assistant","uuid":"a1","timestamp":"2026-04-03T12:31:10.084Z","message":{"role":"assistant","content":[{"type":"text","text":"네 리팩토링 완료"}]},"stop_reason":null}"#;
    const TOOL_RESULT_LINE: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01","content":"The file updated successfully."}]},"uuid":"e3f07f16","timestamp":"2026-04-13T11:52:12.870Z"}"#;
    const USER_LINE_NEW_FORMAT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"바이너리임"}]},"uuid":"81592cdf","timestamp":"2026-04-13T11:52:09.066Z","cwd":"/Users/sktds92/project/rust-project/aiprism"}"#;

    #[test]
    fn claude_detects_user_prompt() {
        assert!(ClaudeStrategy.is_user_prompt(USER_LINE));
        assert!(!ClaudeStrategy.is_user_prompt(ASSISTANT_LINE));
    }

    #[test]
    fn claude_detects_completion() {
        // completion signal = 다음 user(human) 메시지 = is_user_prompt와 동일
        assert!(ClaudeStrategy.is_completion_signal(USER_LINE));
        assert!(!ClaudeStrategy.is_completion_signal(ASSISTANT_LINE));
    }

    #[test]
    fn claude_extracts_user_text_only_text_blocks() {
        // content[]에서 type:text만 합산, tool_result 제외
        let text = ClaudeStrategy.extract_user_text(USER_LINE).unwrap();
        assert_eq!(text, "리팩토링 해줘");
    }

    #[test]
    fn claude_extracts_request_id_from_assistant() {
        let id = ClaudeStrategy.extract_request_id(ASSISTANT_LINE).unwrap();
        assert_eq!(id, "a1");
    }

    #[test]
    fn claude_extracts_metadata_cwd() {
        let meta = ClaudeStrategy.extract_metadata(USER_LINE).unwrap();
        assert_eq!(meta.context_file, "/proj");
    }

    #[test]
    fn tool_result_line_not_detected_as_user_prompt() {
        assert!(!ClaudeStrategy.is_user_prompt(TOOL_RESULT_LINE));
        assert!(!ClaudeStrategy.is_completion_signal(TOOL_RESULT_LINE));
    }

    #[test]
    fn new_format_user_line_detected() {
        assert!(ClaudeStrategy.is_user_prompt(USER_LINE_NEW_FORMAT));
        let meta = ClaudeStrategy.extract_metadata(USER_LINE_NEW_FORMAT).unwrap();
        assert_eq!(meta.context_file, "/Users/sktds92/project/rust-project/aiprism");
    }
}
