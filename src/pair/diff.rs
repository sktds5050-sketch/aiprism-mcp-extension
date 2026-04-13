// Diff generation for code changes
use std::collections::HashMap;
use std::path::PathBuf;
use similar::TextDiff;

/// Generate diff for dirty files only.
/// current_content must include ALL dirty files — deleted files should be passed with empty string.
pub fn generate_diff_from_content(
    snapshots: &HashMap<PathBuf, String>,
    current_content: &[(PathBuf, String)],
) -> String {
    let mut result = String::new();

    for (path, current) in current_content.iter() {
        let original = snapshots.get(path).map(|s| s.as_str()).unwrap_or("");
        if original == current.as_str() {
            continue;
        }

        result.push_str(&format!("### {}\n\n", path.display()));
        result.push_str("```diff\n");
        let diff = TextDiff::from_lines(original, current.as_str());
        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Delete => result.push_str(&format!("- {}", change.value())),
                similar::ChangeTag::Insert => result.push_str(&format!("+ {}", change.value())),
                similar::ChangeTag::Equal => {}
            }
        }
        result.push_str("```\n\n");
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_generates_markdown_for_changed_file() {
        let mut snapshots = HashMap::new();
        snapshots.insert(
            PathBuf::from("src/main.rs"),
            "fn main() {}".to_string(),
        );

        let current = vec![(
            PathBuf::from("src/main.rs"),
            "fn main() { println!(\"hi\"); }".to_string(),
        )];

        let result = generate_diff_from_content(&snapshots, &current);
        assert!(result.contains("### src/main.rs"));
        assert!(result.contains("```diff"));
    }

    #[test]
    fn diff_skips_unchanged_files() {
        let content = "fn main() {}";
        let mut snapshots = HashMap::new();
        snapshots.insert(PathBuf::from("src/main.rs"), content.to_string());

        let current = vec![(PathBuf::from("src/main.rs"), content.to_string())];

        let result = generate_diff_from_content(&snapshots, &current);
        assert!(result.is_empty());
    }

    #[test]
    fn diff_handles_multiple_files() {
        let mut snapshots = HashMap::new();
        snapshots.insert(PathBuf::from("src/main.rs"), "old main".to_string());
        snapshots.insert(PathBuf::from("src/lib.rs"), "old lib".to_string());

        let current = vec![
            (PathBuf::from("src/main.rs"), "new main".to_string()),
            (PathBuf::from("src/lib.rs"), "old lib".to_string()), // unchanged
        ];

        let result = generate_diff_from_content(&snapshots, &current);
        assert!(result.contains("### src/main.rs"));
        assert!(!result.contains("### src/lib.rs"));
    }

    #[test]
    fn diff_handles_deleted_file() {
        let mut snapshots = HashMap::new();
        snapshots.insert(PathBuf::from("src/old.rs"), "old content\n".to_string());

        // Deleted file passed as empty string
        let current = vec![(PathBuf::from("src/old.rs"), "".to_string())];

        let result = generate_diff_from_content(&snapshots, &current);
        assert!(result.contains("### src/old.rs"));
        assert!(result.contains("- old content"));
    }

    #[test]
    fn diff_handles_new_file() {
        let snapshots = HashMap::new();

        let current = vec![(PathBuf::from("src/new.rs"), "new content\n".to_string())];

        let result = generate_diff_from_content(&snapshots, &current);
        assert!(result.contains("### src/new.rs"));
        assert!(result.contains("+ new content"));
    }
}
