// Copilot log parsing strategy
use crate::models::PairMetadata;
use crate::strategy::LogParsingStrategy;
use std::path::PathBuf;

pub struct CopilotStrategy;

// Copilot JSONL 형식:
// User:  kind==1, k==["requests", N, "result"] → v.metadata.renderedUserMessage 포함
// AI:    kind==2, k==["requests", N, "response"] → v 배열에 value 항목 포함
// N이 같으면 같은 턴 → extract_request_id로 k[1] 추출해서 페어링

#[async_trait::async_trait]
impl LogParsingStrategy for CopilotStrategy {
    fn agent_name(&self) -> &str {
        "GitHub Copilot"
    }

    fn target_extension(&self) -> &str {
        ".jsonl"
    }

    fn is_user_prompt(&self, line: &str) -> bool {
        // kind==1, k==["requests", N, "result"], v.metadata.renderedUserMessage 존재
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return false; };
        let is_kind1 = v.get("kind").and_then(|k| k.as_u64()) == Some(1);
        let k_arr = v.get("k").and_then(|k| k.as_array());
        let is_result = k_arr.map(|k| {
            k.len() == 3
                && k[0].as_str() == Some("requests")
                && k[2].as_str() == Some("result")
        }).unwrap_or(false);
        let has_rendered = v
            .pointer("/v/metadata/renderedUserMessage")
            .and_then(|r| r.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        is_kind1 && is_result && has_rendered
    }

    fn is_completion_signal(&self, line: &str) -> bool {
        // kind==2, k==["requests", N, "response"], v 배열에 value 항목 존재
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return false; };
        let is_kind2 = v.get("kind").and_then(|k| k.as_u64()) == Some(2);
        let k_arr = v.get("k").and_then(|k| k.as_array());
        let is_response = k_arr.map(|k| {
            k.len() == 3
                && k[0].as_str() == Some("requests")
                && k[2].as_str() == Some("response")
        }).unwrap_or(false);
        let has_value = v
            .get("v")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().any(|item| {
                item.get("kind").and_then(|k| k.as_str()) != Some("thinking")
                    && item.get("value").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
            }))
            .unwrap_or(false);
        is_kind2 && is_response && has_value
    }

    fn extract_request_id(&self, line: &str) -> Option<String> {
        // k==["requests", N, "response"] → k[1]이 N (페어링 키)
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let k = v.get("k")?.as_array()?;
        if k.len() >= 2 {
            if let Some(idx) = k[1].as_u64() {
                return Some(idx.to_string());
            }
        }
        None
    }

    fn extract_metadata(&self, line: &str) -> Option<PairMetadata> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Some(PairMetadata {
            timestamp,
            model_id: "copilot-unknown".to_string(),
            context_file: String::new(),
            log_file_path: PathBuf::new(),
            source: self.agent_name().to_string(),
        })
    }

    fn extract_user_text(&self, line: &str) -> Option<String> {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(val) => val,
            Err(e) => {
                tracing::warn!(error = ?e, "JSON parse failed in Copilot extract_user_text");
                return None;
            }
        };

        // renderedUserMessage 배열에서 <userRequest>...</userRequest> 사이 텍스트 추출
        let rendered = v
            .get("v")?
            .get("metadata")?
            .get("renderedUserMessage")?
            .as_array()?;

        for item in rendered {
            if item.get("type").and_then(|t| t.as_u64()) == Some(1) {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    if let Some(start) = text.find("<userRequest>") {
                        let after = &text[start + "<userRequest>".len()..];
                        if let Some(end) = after.find("</userRequest>") {
                            let user_text = after[..end].trim().to_string();
                            if !user_text.is_empty() {
                                tracing::trace!("User text extracted from renderedUserMessage");
                                return Some(user_text);
                            }
                        }
                    }
                }
            }
        }

        tracing::trace!("No userRequest found in renderedUserMessage");
        None
    }

    fn extract_ai_text(&self, line: &str) -> Option<String> {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(val) => val,
            Err(e) => {
                tracing::warn!(error = ?e, "JSON parse failed in Copilot extract_ai_text");
                return None;
            }
        };

        // v 배열에서 kind != "thinking"이고 non-empty value 문자열인 항목들 join
        let items = v.get("v")?.as_array()?;
        let parts: Vec<&str> = items
            .iter()
            .filter(|item| item.get("kind").and_then(|k| k.as_str()) != Some("thinking"))
            .filter_map(|item| item.get("value")?.as_str())
            .filter(|s| !s.is_empty())
            .collect();

        if parts.is_empty() {
            tracing::trace!("No value found in response items");
            None
        } else {
            tracing::trace!("AI text extracted from response values");
            Some(parts.join(""))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // kind==1, k==["requests",0,"result"], renderedUserMessage에 <userRequest> 포함
    const USER_LINE: &str = r#"{"kind":1,"k":["requests",0,"result"],"v":{"timings":{"firstProgress":100,"totalElapsed":200},"metadata":{"codeBlocks":[],"renderedUserMessage":[{"type":1,"text":"<context>\n날짜 정보\n</context>\n<userRequest>\n버그 찾아줘\n</userRequest>\n"}]}}}"#;
    // kind==2, k==["requests",0,"response"], v 배열에 value 포함
    const RESPONSE_LINE: &str = r#"{"kind":2,"k":["requests",0,"response"],"v":[{"kind":"thinking","value":"생각중...","id":"thinking_0"},{"kind":"thinking","value":"","id":"","metadata":{"vscodeReasoningDone":true}},{"value":"버그 찾았습니다: 타입 오류","supportThemeIcons":false}]}"#;
    const OTHER_LINE: &str = r#"{"kind":1,"k":["customTitle"],"v":"테스트 세션"}"#;

    #[test]
    fn copilot_detects_user_prompt() {
        assert!(CopilotStrategy.is_user_prompt(USER_LINE));
        assert!(!CopilotStrategy.is_user_prompt(RESPONSE_LINE));
        assert!(!CopilotStrategy.is_user_prompt(OTHER_LINE));
    }

    #[test]
    fn copilot_detects_completion() {
        assert!(CopilotStrategy.is_completion_signal(RESPONSE_LINE));
        assert!(!CopilotStrategy.is_completion_signal(USER_LINE));
        assert!(!CopilotStrategy.is_completion_signal(OTHER_LINE));
    }

    #[test]
    fn copilot_extracts_request_id() {
        let id = CopilotStrategy.extract_request_id(RESPONSE_LINE).unwrap();
        assert_eq!(id, "0");
    }

    #[test]
    fn copilot_extracts_user_text() {
        let text = CopilotStrategy.extract_user_text(USER_LINE).unwrap();
        assert_eq!(text, "버그 찾아줘");
    }

    #[test]
    fn copilot_extracts_ai_text() {
        let text = CopilotStrategy.extract_ai_text(RESPONSE_LINE).unwrap();
        assert_eq!(text, "버그 찾았습니다: 타입 오류");
    }
}
