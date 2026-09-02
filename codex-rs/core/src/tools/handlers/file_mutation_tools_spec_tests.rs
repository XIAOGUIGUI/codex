use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn mutation_tool_specs_use_standard_function_tools() {
    let names = [FileMutationToolKind::Edit, FileMutationToolKind::Write]
        .into_iter()
        .map(create_file_mutation_tool)
        .map(|tool| match tool {
            ToolSpec::Function(tool) => tool.name,
            _ => panic!("expected function tool"),
        })
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["edit_file", "write_file"]);
}
