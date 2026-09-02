use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;

use super::FileToolKind;

pub(crate) fn create_file_tool(kind: FileToolKind) -> ToolSpec {
    let (name, description, properties, required) = match kind {
        FileToolKind::Read => (
            "read_file",
            "Read a text file without invoking PowerShell. Results are line-numbered and bounded; use offset and limit to continue when next_offset is returned.",
            BTreeMap::from([
                (
                    "file_path".to_string(),
                    JsonSchema::string(Some(
                        "Absolute or workspace-relative Windows file path.".to_string(),
                    )),
                ),
                (
                    "offset".to_string(),
                    JsonSchema::number(Some(
                        "One-based line number to start reading. Defaults to 1.".to_string(),
                    )),
                ),
                (
                    "limit".to_string(),
                    JsonSchema::number(Some(
                        "Maximum lines to return. Defaults to 400 and cannot exceed 1000."
                            .to_string(),
                    )),
                ),
            ]),
            vec!["file_path".to_string()],
        ),
        FileToolKind::Grep => (
            "grep_files",
            "Search file contents with Codex's bundled ripgrep executable, invoked directly without PowerShell. Results are bounded and respect ignore files by default.",
            BTreeMap::from([
                (
                    "pattern".to_string(),
                    JsonSchema::string(Some("Regular expression to search for.".to_string())),
                ),
                (
                    "path".to_string(),
                    JsonSchema::string(Some(
                        "Directory or file to search. Defaults to the workspace directory."
                            .to_string(),
                    )),
                ),
                (
                    "glob".to_string(),
                    JsonSchema::string(Some(
                        "Optional ripgrep glob restricting searched files.".to_string(),
                    )),
                ),
                (
                    "output_mode".to_string(),
                    JsonSchema::string_enum(
                        vec![
                            serde_json::json!("files_with_matches"),
                            serde_json::json!("content"),
                            serde_json::json!("count"),
                        ],
                        Some(
                            "Result form. Defaults to files_with_matches; use content only when matching lines are needed."
                                .to_string(),
                        ),
                    ),
                ),
                (
                    "context".to_string(),
                    JsonSchema::number(Some(
                        "Context lines around content matches. Ignored in other modes; maximum 20."
                            .to_string(),
                    )),
                ),
                (
                    "head_limit".to_string(),
                    JsonSchema::number(Some(
                        "Maximum result lines. Defaults to 100 and cannot exceed 1000."
                            .to_string(),
                    )),
                ),
            ]),
            vec!["pattern".to_string()],
        ),
        FileToolKind::Glob => (
            "glob_files",
            "List files matching a glob with Codex's bundled ripgrep executable, invoked directly without PowerShell. Results are sorted, bounded, and respect ignore files by default.",
            BTreeMap::from([
                (
                    "pattern".to_string(),
                    JsonSchema::string(Some("Ripgrep glob pattern to match.".to_string())),
                ),
                (
                    "path".to_string(),
                    JsonSchema::string(Some(
                        "Directory to search. Defaults to the workspace directory.".to_string(),
                    )),
                ),
                (
                    "head_limit".to_string(),
                    JsonSchema::number(Some(
                        "Maximum paths to return. Defaults to 100 and cannot exceed 1000."
                            .to_string(),
                    )),
                ),
            ]),
            vec!["pattern".to_string()],
        ),
    };

    ToolSpec::Function(ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(required), Some(false.into())),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "file_tools_spec_tests.rs"]
mod tests;
