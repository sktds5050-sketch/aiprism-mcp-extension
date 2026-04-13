// MCP config file writer — merge-safe (preserves existing MCP server entries)
use std::path::PathBuf;
use serde_json::{json, Value};

fn merge_and_write(path: &PathBuf, top_key: &str, entry: Value) -> std::io::Result<()> {
    let mut root: Value = if path.exists() {
        let existing = std::fs::read_to_string(path)?;
        serde_json::from_str(&existing).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    root[top_key]["aiprism"] = entry;

    std::fs::write(path, serde_json::to_string_pretty(&root).unwrap())
}

/// 프로젝트 루트에 Claude Code용 .mcp.json 생성/업데이트
pub fn write_claude_mcp(project_root: &PathBuf, base_url: &str, token: &str) -> std::io::Result<()> {
    let path = project_root.join(".mcp.json");
    let entry = json!({
        "type": "sse",
        "url": format!("{}/mcp/sse", base_url),
        "headers": { "Authorization": format!("Bearer {}", token) },
        "tools": [
            {
                "name": "search_groups",
                "description": "Search public prompt groups by keyword"
            },
            {
                "name": "search_my_groups",
                "description": "Search my prompt groups (including private)"
            },
            {
                "name": "get_group_detail",
                "description": "Get full details of a prompt group by ID"
            }
        ]
    });
    merge_and_write(&path, "mcpServers", entry)
}

/// 프로젝트 루트에 GitHub Copilot용 .vscode/mcp.json 생성/업데이트
pub fn write_copilot_mcp(project_root: &PathBuf, base_url: &str, token: &str) -> std::io::Result<()> {
    let vscode_dir = project_root.join(".vscode");
    std::fs::create_dir_all(&vscode_dir)?;
    let path = vscode_dir.join("mcp.json");
    let entry = json!({
        "type": "sse",
        "url": format!("{}/mcp/sse", base_url),
        "headers": { "Authorization": format!("Bearer {}", token) }
    });
    merge_and_write(&path, "servers", entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn write_claude_mcp_creates_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();

        write_claude_mcp(&root, "https://aiprism.dsj.co.kr", "tok123").unwrap();

        let content = fs::read_to_string(root.join(".mcp.json")).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["mcpServers"]["aiprism"]["type"], "sse");
        assert!(v["mcpServers"]["aiprism"]["headers"]["Authorization"]
            .as_str().unwrap().contains("tok123"));
    }

    #[test]
    fn write_claude_mcp_includes_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();

        write_claude_mcp(&root, "https://aiprism.dsj.co.kr", "tok123").unwrap();

        let content = fs::read_to_string(root.join(".mcp.json")).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        let tools = &v["mcpServers"]["aiprism"]["tools"];
        assert!(tools.is_array());
        assert_eq!(tools.as_array().unwrap().len(), 3);
        assert_eq!(tools[0]["name"], "search_groups");
        assert_eq!(tools[1]["name"], "search_my_groups");
        assert_eq!(tools[2]["name"], "get_group_detail");
    }

    #[test]
    fn write_claude_mcp_merges_existing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let path = root.join(".mcp.json");

        // 기존 파일에 다른 서버 설정 존재
        fs::write(&path, r#"{"mcpServers":{"other-server":{"type":"stdio","command":"other"}}}"#).unwrap();

        write_claude_mcp(&root, "https://aiprism.dsj.co.kr", "tok123").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        // 기존 서버 보존
        assert_eq!(v["mcpServers"]["other-server"]["type"], "stdio");
        // aiprism 추가
        assert_eq!(v["mcpServers"]["aiprism"]["type"], "sse");
    }

    #[test]
    fn write_claude_mcp_preserves_tools_on_merge() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let path = root.join(".mcp.json");

        // 기존 파일에 aiprism 서버 이미 존재 (tools 없음)
        fs::write(&path, r#"{"mcpServers":{"aiprism":{"type":"sse","url":"old"}}}"#).unwrap();

        write_claude_mcp(&root, "https://aiprism.dsj.co.kr", "tok123").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        // tools 추가됨
        assert!(v["mcpServers"]["aiprism"]["tools"].is_array());
        assert_eq!(v["mcpServers"]["aiprism"]["tools"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn write_copilot_mcp_creates_vscode_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();

        write_copilot_mcp(&root, "https://aiprism.dsj.co.kr", "tok456").unwrap();

        assert!(root.join(".vscode").exists());
        let content = fs::read_to_string(root.join(".vscode/mcp.json")).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["servers"]["aiprism"]["type"], "sse");
    }

    #[test]
    fn write_copilot_mcp_merges_existing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let vscode_dir = root.join(".vscode");
        fs::create_dir_all(&vscode_dir).unwrap();
        let path = vscode_dir.join("mcp.json");

        fs::write(&path, r#"{"servers":{"github":{"type":"stdio","command":"gh"}}}"#).unwrap();

        write_copilot_mcp(&root, "https://aiprism.dsj.co.kr", "tok456").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["servers"]["github"]["type"], "stdio");
        assert_eq!(v["servers"]["aiprism"]["type"], "sse");
    }

    #[test]
    fn mcp_files_contain_correct_token() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();

        write_claude_mcp(&root, "https://aiprism.dsj.co.kr", "secret_token").unwrap();
        write_copilot_mcp(&root, "https://aiprism.dsj.co.kr", "secret_token").unwrap();

        let claude = fs::read_to_string(root.join(".mcp.json")).unwrap();
        let copilot = fs::read_to_string(root.join(".vscode/mcp.json")).unwrap();

        assert!(claude.contains("Bearer secret_token"));
        assert!(copilot.contains("Bearer secret_token"));
    }
}
