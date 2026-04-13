// HTTP sender for API communication
use crate::models::PairPayload;

/// Trait for sending pair payloads to remote server
#[async_trait::async_trait]
pub trait SenderTrait: Send + Sync {
    async fn send(&self, payload: &PairPayload) -> Result<u64, String>;
}

/// HTTP-based sender with retry logic
pub struct Sender {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

impl Sender {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            token,
        }
    }

    /// Send with exponential backoff retry (max 3 attempts)
    async fn send_with_retry(&self, payload: &PairPayload) -> Result<u64, String> {
        let mut attempt = 0;
        let max_attempts = 3;

        loop {
            attempt += 1;

            let result = self.send_once(payload).await;

            match result {
                Ok(id) => {
                    tracing::info!("Send successful on attempt {}", attempt);
                    return Ok(id);
                }
                Err(e) => {
                    tracing::warn!("Send failed on attempt {}: {}", attempt, e);
                    if attempt >= max_attempts {
                        tracing::error!("Failed after {} attempts: {}", max_attempts, e);
                        return Err(format!("Failed after {} attempts: {}", max_attempts, e));
                    }

                    // Check if error is retryable (5xx or network error, not 4xx)
                    if e.parse::<u16>().map_or(false, |c| (400..500).contains(&c)) {
                        // 4xx error - don't retry
                        tracing::error!("Client error, not retrying: {}", e);
                        return Err(e);
                    }

                    // Wait before retry: 2s, 4s, 8s
                    let wait_secs = 2_u64.pow(attempt as u32);
                    tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                }
            }
        }
    }

    /// Send once without retry
    async fn send_once(&self, payload: &PairPayload) -> Result<u64, String> {
        let url = format!("{}/api/prompt-groups/mcp-save", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .json(payload)
            .send()
            .await
            .map_err(|e| {
                tracing::error!("Request failed: {}", e);
                format!("Request failed: {}", e)
            })?;

        let status = response.status();

        if status.is_success() || status.as_u16() == 201 {
            let json: serde_json::Value = response
                .json()
                .await
                .map_err(|e| {
                    tracing::error!("Failed to parse response: {}", e);
                    format!("Failed to parse response: {}", e)
                })?;

            let id = json
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| {
                    tracing::error!("Response missing 'id' field: {:?}", json);
                    "Response missing 'id' field".to_string()
                })?;

            tracing::info!("Successfully saved pair with collection_id: {}", id);
            Ok(id)
        } else if status.is_client_error() {
            let error_body = response.text().await.unwrap_or_default();
            tracing::error!("Client error {}: {}", status.as_u16(), error_body);
            Err(format!("{}", status.as_u16()))
        } else {
            let error_body = response.text().await.unwrap_or_default();
            tracing::error!("Server error {}: {}", status.as_u16(), error_body);
            Err(format!("{}", status.as_u16()))
        }
    }
}

#[async_trait::async_trait]
impl SenderTrait for Sender {
    async fn send(&self, payload: &PairPayload) -> Result<u64, String> {
        self.send_with_retry(payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn make_new_collection_payload() -> PairPayload {
        PairPayload {
            user_query: "test query".to_string(),
            ai_response: "test response".to_string(),
            project_path: "/proj".to_string(),
            collection_id: None,
            tags: Some(vec!["claudecode".to_string()]),
            title: Some("claudecode 2026-04-05T00:00:00+00:00".to_string()),
        }
    }

    fn make_existing_collection_payload() -> PairPayload {
        PairPayload {
            user_query: "test query".to_string(),
            ai_response: "test response".to_string(),
            project_path: "/proj".to_string(),
            collection_id: Some(1239),
            tags: None,
            title: None,
        }
    }

    #[tokio::test]
    async fn send_returns_collection_id_on_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/prompt-groups/mcp-save"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 1239})))
            .mount(&mock_server)
            .await;

        let sender = Sender::new(mock_server.uri(), "test-token".into());
        let payload = make_new_collection_payload();
        let id = sender.send(&payload).await.unwrap();
        assert_eq!(id, 1239);
    }

    #[tokio::test]
    async fn send_includes_bearer_token() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 1})))
            .mount(&mock_server)
            .await;

        let sender = Sender::new(mock_server.uri(), "test-token".into());
        sender.send(&make_new_collection_payload()).await.unwrap();
    }

    #[tokio::test]
    async fn retries_on_server_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(3)
            .mount(&mock_server)
            .await;

        let sender = Sender::new(mock_server.uri(), "token".into());
        let result = sender.send(&make_new_collection_payload()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn no_retry_on_client_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&mock_server)
            .await;

        let sender = Sender::new(mock_server.uri(), "bad-token".into());
        let result = sender.send(&make_new_collection_payload()).await;
        assert!(result.is_err());
    }
}
