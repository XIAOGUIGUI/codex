use pretty_assertions::assert_eq;

use super::*;

#[test]
fn presentations_map_native_tools_to_exploration_commands() {
    assert_eq!(
        file_tool_presentation(FileToolKind::Read, r#"{"file_path":"src/main.rs"}"#).parsed_cmd,
        vec![ParsedCommand::Read {
            cmd: "read_file src/main.rs".to_string(),
            name: "main.rs".to_string(),
            path: "src/main.rs".into(),
        }]
    );
    assert_eq!(
        file_tool_presentation(FileToolKind::Grep, r#"{"pattern":"needle","path":"src"}"#)
            .parsed_cmd,
        vec![ParsedCommand::Search {
            cmd: "grep_files src".to_string(),
            query: Some("needle".to_string()),
            path: Some("src".to_string()),
        }]
    );
    assert_eq!(
        file_tool_presentation(FileToolKind::Glob, r#"{"pattern":"*.rs"}"#).parsed_cmd,
        vec![ParsedCommand::ListFiles {
            cmd: "glob_files".to_string(),
            path: Some("*.rs".to_string()),
        }]
    );
}
