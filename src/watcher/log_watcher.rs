// Log watcher trait and common types
use crate::models::PairMetadata;
use std::collections::HashMap;
use std::path::PathBuf;
use serde_json::{json, Value};

/// OffsetStore: 각 파일의 마지막 읽은 위치 추적
#[derive(Debug, Clone)]
pub struct OffsetStore {
    offsets: HashMap<PathBuf, u64>,
}

impl OffsetStore {
    pub fn new() -> Self {
        Self {
            offsets: HashMap::new(),
        }
    }

    pub fn load_from_file(path: &PathBuf) -> std::io::Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = std::fs::read_to_string(path)?;
        let json: Value = serde_json::from_str(&content).unwrap_or(json!({}));

        let mut offsets = HashMap::new();
        if let Some(obj) = json.as_object() {
            for (key, val) in obj {
                if let Some(offset) = val.as_u64() {
                    offsets.insert(PathBuf::from(key), offset);
                }
            }
        }
        Ok(Self { offsets })
    }

    pub fn save_to_file(&self, path: &PathBuf) -> std::io::Result<()> {
        let mut json = serde_json::json!({});
        for (file_path, offset) in &self.offsets {
            if let Some(path_str) = file_path.to_str() {
                json[path_str] = Value::Number((*offset).into());
            }
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, json.to_string())?;
        Ok(())
    }

    pub fn get_offset(&self, file_path: &PathBuf) -> u64 {
        self.offsets.get(file_path).copied().unwrap_or(0)
    }

    pub fn set_offset(&mut self, file_path: PathBuf, offset: u64) {
        self.offsets.insert(file_path, offset);
    }
}

/// PairManager trait (실제 구현은 pair 모듈에서)
pub trait PairManagerTrait: Send + Sync {
    fn on_user_prompt(&self, metadata: PairMetadata, text: String);
    fn on_completion(&self, text: String, request_id: String);
    fn on_assistant_line(&self);
}

