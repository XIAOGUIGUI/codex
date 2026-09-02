use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn file_tool_specs_use_standard_function_tools() {
    let names = [FileToolKind::Read, FileToolKind::Grep, FileToolKind::Glob]
        .into_iter()
        .map(create_file_tool)
        .map(|tool| match tool {
            ToolSpec::Function(tool) => tool.name,
            _ => panic!("expected function tool"),
        })
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["read_file", "grep_files", "glob_files"]);
}
