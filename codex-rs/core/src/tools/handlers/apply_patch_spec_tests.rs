use super::*;
use pretty_assertions::assert_eq;

#[test]
fn create_apply_patch_freeform_tool_matches_expected_spec() {
    assert_eq!(
        create_apply_patch_freeform_tool(/*include_environment_id*/ false),
        ToolSpec::Freeform(FreeformTool {
            name: "apply_patch".to_string(),
            description:
                "The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON."
                    .to_string(),
            defer_loading: None,
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: APPLY_PATCH_LARK_GRAMMAR.to_string(),
            },
        })
    );
}

#[test]
fn create_apply_patch_freeform_tool_includes_environment_id_when_requested() {
    let ToolSpec::Freeform(tool) =
        create_apply_patch_freeform_tool(/*include_environment_id*/ true)
    else {
        panic!("expected freeform tool");
    };

    assert!(tool.format.definition.contains("environment_id?"));
    assert!(
        tool.format
            .definition
            .contains("\"*** Environment ID: \" filename LF")
    );
}

#[test]
fn create_apply_patch_function_tool_matches_expected_spec() {
    assert_eq!(
        create_apply_patch_function_tool(/*include_environment_id*/ false),
        ToolSpec::Function(ResponsesApiTool {
            name: "apply_patch".to_string(),
            description: "Apply a context-checked patch to files. When edit_file or write_file is available, prefer those structured tools for single-file changes and use apply_patch for complex or multi-file edits. This is a standard JSON function tool; pass the patch text in the `patch` field. The Codex host supplies omitted outer Begin/End markers."
                .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "patch".to_string(),
                    JsonSchema::string(Some(
                        "Patch text to apply. You may provide a complete patch from `*** Begin Patch` through `*** End Patch`, or omit both outer markers and start directly with `*** Add File:`, `*** Update File:`, or `*** Delete File:`. Update hunks must start with `@@`, and every hunk body line must start with a space (context), `+` (added), or `-` (removed). Do not wrap the patch in Markdown or a command array."
                            .to_string(),
                    )),
                )]),
                Some(vec!["patch".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    );
}

#[test]
fn create_apply_patch_function_tool_includes_environment_id_when_requested() {
    let ToolSpec::Function(tool) =
        create_apply_patch_function_tool(/*include_environment_id*/ true)
    else {
        panic!("expected function tool");
    };

    assert_eq!(
        serde_json::to_value(tool.parameters)
            .expect("schema should serialize")
            .pointer("/properties/environment_id")
            .is_some(),
        true
    );
}
