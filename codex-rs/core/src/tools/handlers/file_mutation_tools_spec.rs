use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;

use super::FileMutationToolKind;

pub(crate) fn create_file_mutation_tool(kind: FileMutationToolKind) -> ToolSpec {
    let (name, description, properties, required) = match kind {
        FileMutationToolKind::Edit => (
            "edit_file",
            "Prefer this tool for a small, precise change to one existing text file. It performs an exact old_string replacement without apply_patch syntax, preserves UTF-8 BOM and existing line endings, and rejects ambiguous matches unless replace_all is true. The complete JSON arguments must stay under 8192 bytes; use apply_patch for larger changes.",
            BTreeMap::from([
                (
                    "file_path".to_string(),
                    JsonSchema::string(Some(
                        "Absolute or workspace-relative Windows file path.".to_string(),
                    )),
                ),
                (
                    "old_string".to_string(),
                    JsonSchema::string(Some(
                        "Exact existing text to replace, including indentation. Use more surrounding context when the text occurs more than once."
                            .to_string(),
                    )),
                ),
                (
                    "new_string".to_string(),
                    JsonSchema::string(Some("Replacement text.".to_string())),
                ),
                (
                    "replace_all".to_string(),
                    JsonSchema::boolean(Some(
                        "Replace every exact occurrence. Defaults to false.".to_string(),
                    )),
                ),
            ]),
            vec![
                "file_path".to_string(),
                "old_string".to_string(),
                "new_string".to_string(),
            ],
        ),
        FileMutationToolKind::Write => (
            "write_file",
            "Create one small UTF-8 text file without invoking PowerShell. This tool atomically refuses to overwrite an existing path. The complete JSON arguments must stay under 8192 bytes; use edit_file for existing files and apply_patch for larger content.",
            BTreeMap::from([
                (
                    "file_path".to_string(),
                    JsonSchema::string(Some(
                        "Absolute or workspace-relative Windows file path.".to_string(),
                    )),
                ),
                (
                    "content".to_string(),
                    JsonSchema::string(Some(
                        "Complete UTF-8 contents for the new file.".to_string(),
                    )),
                ),
            ]),
            vec!["file_path".to_string(), "content".to_string()],
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
#[path = "file_mutation_tools_spec_tests.rs"]
mod tests;
