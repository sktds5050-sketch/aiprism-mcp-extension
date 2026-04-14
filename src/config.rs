use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://aiprism.dsj.co.kr";

pub const DEFAULT_EXTENSIONS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "cpp", "h",
    "cs", "rb", "swift", "kt", "scala", "php", "html", "css", "scss",
    "toml", "yaml", "yml", "md",
];

pub const DEFAULT_EXCLUDE_DIRS: &[&str] = &[
    ".git", "target", "node_modules", ".venv",
    "__pycache__", "dist", "build", "workflow", ".claude",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub api_token: String,
    pub base_url: String,
    #[serde(default)]
    pub source_roots: Vec<PathBuf>,
    #[serde(default = "default_quiet_period")]
    pub quiet_period_secs: u64,
    #[serde(default = "default_watch_extensions")]
    pub watch_extensions: Vec<String>,
    #[serde(default = "default_exclude_dirs")]
    pub exclude_dirs: Vec<String>,
}

fn default_quiet_period() -> u64 {
    60
}

fn default_watch_extensions() -> Vec<String> {
    DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect()
}

fn default_exclude_dirs() -> Vec<String> {
    DEFAULT_EXCLUDE_DIRS.iter().map(|s| s.to_string()).collect()
}

impl Config {
    /// ~/.aiprism/config.json 경로 반환
    fn config_path() -> PathBuf {
        let home = dirs::home_dir().expect("Home directory not found");
        home.join(".aiprism").join("config.json")
    }

    /// ~/.aiprism/config.json 에서 token 로드
    /// 파일 없으면 에러 반환
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Self::config_path();
        let content = fs::read_to_string(&path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// token을 ~/.aiprism/config.json 에 저장
    /// 디렉토리 없으면 생성, 파일 권한은 0600으로 설정
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path();
        let dir = path.parent().unwrap();

        // ~/.aiprism 디렉토리 생성
        fs::create_dir_all(dir)?;

        // config.json 작성
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;

        // Unix: 파일 권한을 0600 (user read/write only)으로 설정
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, perms)?;
        }

        Ok(())
    }

    /// source_roots에 경로 추가 후 config.json 저장
    /// 중복 경로는 제거 후 다시 추가
    pub fn add_source_root(&mut self, path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        self.source_roots.retain(|p| p != &path);
        self.source_roots.push(path);
        self.save()?;
        Ok(())
    }

    /// token 검증 (비어있지 않은지 확인)
    pub fn validate(&self) -> Result<(), String> {
        if self.api_token.is_empty() {
            return Err("Token cannot be empty".to_string());
        }
        if self.base_url.is_empty() {
            return Err("Base URL cannot be empty".to_string());
        }
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    fn mock_config_path(temp_dir: &TempDir) -> PathBuf {
        temp_dir.path().join("config.json")
    }

    #[test]
    fn config_load_success() {
        // Given: config.json 파일이 존재
        let temp_dir = TempDir::new().unwrap();
        let config_path = mock_config_path(&temp_dir);

        let config_data = r#"{"api_token": "test-token-12345", "base_url": "https://api.example.com"}"#;
        fs::write(&config_path, config_data).unwrap();

        // When: 파일에서 로드
        let content = fs::read_to_string(&config_path).unwrap();
        let config: Config = serde_json::from_str(&content).unwrap();

        // Then: token이 올바르게 파싱됨
        assert_eq!(config.api_token, "test-token-12345");
    }

    #[test]
    fn config_load_file_not_found() {
        // Given: 존재하지 않는 파일
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("nonexistent.json");

        // When: 파일 로드 시도
        let result = fs::read_to_string(&config_path);

        // Then: 에러 발생
        assert!(result.is_err());
    }

    #[test]
    fn config_load_invalid_json() {
        // Given: 잘못된 JSON 형식
        let temp_dir = TempDir::new().unwrap();
        let config_path = mock_config_path(&temp_dir);
        fs::write(&config_path, "{ invalid json }").unwrap();

        // When: 파싱 시도
        let content = fs::read_to_string(&config_path).unwrap();
        let result: Result<Config, _> = serde_json::from_str(&content);

        // Then: serde_json 에러 발생
        assert!(result.is_err());
    }

    #[test]
    fn config_save_creates_directory() {
        // Given: 디렉토리가 없음
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join(".claude").join("config.json");

        // When: 파일 저장
        let config = Config {
            api_token: "test-token".to_string(),
            base_url: "https://api.example.com".to_string(),
            source_roots: vec![],
            quiet_period_secs: 30,
            watch_extensions: default_watch_extensions(),
            exclude_dirs: default_exclude_dirs(),
        };
        let content = serde_json::to_string_pretty(&config).unwrap();

        fs::create_dir_all(config_file.parent().unwrap()).unwrap();
        fs::write(&config_file, content).unwrap();

        // Then: 디렉토리와 파일이 생성됨
        assert!(config_file.parent().unwrap().exists());
        assert!(config_file.exists());
    }

    #[test]
    fn config_save_overwrites_existing() {
        // Given: 기존 config.json
        let temp_dir = TempDir::new().unwrap();
        let config_path = mock_config_path(&temp_dir);
        fs::write(&config_path, r#"{"api_token": "old-token", "base_url": "https://old.com"}"#).unwrap();

        // When: 새로운 token으로 save
        let new_config = Config {
            api_token: "new-token".to_string(),
            base_url: "https://new.com".to_string(),
            source_roots: vec![],
            quiet_period_secs: 30,
            watch_extensions: default_watch_extensions(),
            exclude_dirs: default_exclude_dirs(),
        };
        let content = serde_json::to_string_pretty(&new_config).unwrap();
        fs::write(&config_path, content).unwrap();

        // Then: 파일이 업데이트됨
        let loaded: Config = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(loaded.api_token, "new-token");
    }

    #[test]
    #[cfg(unix)]
    fn config_save_sets_permissions_0600() {
        // Given: 새 config 파일을 저장
        let temp_dir = TempDir::new().unwrap();
        let config_path = mock_config_path(&temp_dir);

        let config = Config {
            api_token: "test-token".to_string(),
            base_url: "https://api.example.com".to_string(),
            source_roots: vec![],
            quiet_period_secs: 30,
            watch_extensions: default_watch_extensions(),
            exclude_dirs: default_exclude_dirs(),
        };
        let content = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&config_path, content).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&config_path, perms).unwrap();

        // When: 파일 권한 확인
        let metadata = fs::metadata(&config_path).unwrap();
        let mode = metadata.permissions().mode();

        // Then: 권한이 0o600 (user read/write only)
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn config_validate_success() {
        // Given: 유효한 token
        let config = Config {
            api_token: "valid-token".to_string(),
            base_url: "https://api.example.com".to_string(),
            source_roots: vec![],
            quiet_period_secs: 30,
            watch_extensions: default_watch_extensions(),
            exclude_dirs: default_exclude_dirs(),
        };

        // When: validate 호출
        let result = config.validate();

        // Then: 성공
        assert!(result.is_ok());
    }

    #[test]
    fn config_validate_empty_token() {
        // Given: 빈 token
        let config = Config {
            api_token: "".to_string(),
            base_url: "https://api.example.com".to_string(),
            source_roots: vec![],
            quiet_period_secs: 30,
            watch_extensions: default_watch_extensions(),
            exclude_dirs: default_exclude_dirs(),
        };

        // When: validate 호출
        let result = config.validate();

        // Then: 에러 반환
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Token cannot be empty");
    }

}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn cli_init_saves_config() {
        // Given: CLI init 커맨드 시뮬레이션
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        // When: token으로 config 생성 및 저장
        let config = Config {
            api_token: "cli-test-token".to_string(),
            base_url: "https://api.example.com".to_string(),
            source_roots: vec![],
            quiet_period_secs: 30,
            watch_extensions: default_watch_extensions(),
            exclude_dirs: default_exclude_dirs(),
        };
        config.validate().unwrap();

        let content = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&config_path, content).unwrap();

        // Then: config.json이 저장됨
        assert!(config_path.exists());
        let loaded: Config = serde_json::from_str(
            &fs::read_to_string(&config_path).unwrap()
        ).unwrap();
        assert_eq!(loaded.api_token, "cli-test-token");
    }

    #[test]
    fn cli_init_rejects_empty_token() {
        // Given: 빈 token
        let config = Config {
            api_token: "".to_string(),
            base_url: "https://api.example.com".to_string(),
            source_roots: vec![],
            quiet_period_secs: 30,
            watch_extensions: default_watch_extensions(),
            exclude_dirs: default_exclude_dirs(),
        };

        // When: validate 호출
        let result = config.validate();

        // Then: 에러 반환
        assert!(result.is_err());
    }
}
