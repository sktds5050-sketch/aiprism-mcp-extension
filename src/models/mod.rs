use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Pair {
    pub source: String,         // "claudecode" | "GitHub Copilot"
    pub request_id: String,
    pub timestamp: u64,
    pub model_id: String,
    pub user_query: String,
    pub ai_response: String,
    pub context_file: String,   // project_path
    pub code_changes: String,   // Diff Markdown
    pub log_file_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ActivePair {
    pub request_id: String,
    pub user_query: String,
    pub ai_response: String,
    pub context_file: String,
    pub log_file_path: PathBuf,
    pub source: String,          // "claudecode" | "GitHub Copilot"
    pub timestamp: u64,
    pub model_id: String,
    pub snapshots: HashMap<PathBuf, String>,  // Lazy Snapshot
    pub dirty_files: HashSet<PathBuf>,
    pub last_activity: Instant,
    pub quiet_timer_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionState {
    pub collection_id: Option<u64>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairPayload {
    pub user_query: String,
    pub ai_response: String,
    pub project_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,   // 신규 Collection만
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,       // 신규 Collection만
}

#[derive(Debug, Clone)]
pub struct PairMetadata {
    pub timestamp: u64,
    pub model_id: String,
    pub context_file: String,
    pub log_file_path: PathBuf,
    pub source: String,  // "claudecode" | "GitHub Copilot"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_new_collection_includes_tags_and_title() {
        let payload = PairPayload {
            user_query: "q".into(),
            ai_response: "a".into(),
            project_path: "/proj".into(),
            collection_id: None,
            tags: Some(vec!["claudecode".into()]),
            title: Some("claudecode 2026-04-05T14:00:00+09:00".into()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json.get("tags").is_some());
        assert!(json.get("title").is_some());
        assert!(json.get("collection_id").is_none()); // skip_serializing_if
    }

    #[test]
    fn payload_existing_collection_omits_tags_and_title() {
        let payload = PairPayload {
            user_query: "q".into(),
            ai_response: "a".into(),
            project_path: "/proj".into(),
            collection_id: Some(1239),
            tags: None,
            title: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json.get("tags").is_none());
        assert!(json.get("title").is_none());
        assert_eq!(json["collection_id"], 1239);
    }
}
