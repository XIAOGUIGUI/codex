pub(super) struct ClassifiedError {
    pub(super) code: &'static str,
    pub(super) summary: &'static str,
}

pub(super) fn classify(phase: &str, tool: Option<&str>, error: Option<&str>) -> ClassifiedError {
    let Some(error) = error else {
        return classified("operation.completed", "operation completed");
    };
    let error = error.to_ascii_lowercase();
    let contains = |needle: &str| error.contains(needle);
    if phase.starts_with("model.text_integrity") {
        classified(
            "provider.output.text_integrity",
            "Model output contained a text integrity signal",
        )
    } else if phase.starts_with("model.tool_loop") {
        classified(
            "provider.output.tool_loop",
            "Model repeated an identical tool call",
        )
    } else if contains("split writable root") {
        classified(
            "windows.sandbox.split_roots",
            "Windows sandbox rejected split writable roots",
        )
    } else if contains("setup refresh") || contains("prepare fs sandbox") {
        classified("windows.sandbox.setup", "Windows sandbox setup failed")
    } else if contains("createprocesswithlogonw") || contains("failed: 267") {
        classified(
            "windows.sandbox.process_start",
            "Windows sandbox process startup failed",
        )
    } else if contains("access is denied") || contains("access denied") || contains("eperm") {
        classified(
            "windows.sandbox.access_denied",
            "Windows sandbox denied filesystem access",
        )
    } else if contains("custom/freeform") || contains("custom tool") && contains("not support") {
        classified(
            "provider.tool_schema.custom_unsupported",
            "Provider rejected a custom/freeform tool",
        )
    } else if contains("failed to parse function arguments")
        || contains("invalid function arguments")
    {
        classified(
            "provider.function_arguments.invalid",
            "Provider returned invalid function arguments",
        )
    } else if contains("first line of the patch") || contains("*** begin patch") {
        classified(
            "file.apply_patch.missing_begin",
            "apply_patch input was missing its Begin Patch marker",
        )
    } else if contains("unresolved git merge conflict")
        || contains("<<<<<<<")
        || contains(">>>>>>>")
    {
        classified(
            "file.merge_conflict",
            "A file mutation encountered unresolved merge conflict markers",
        )
    } else if contains("unexpected line found in update hunk")
        || contains("every line should start with")
        || contains("expected update hunk to start")
        || contains("not a valid hunk header")
    {
        classified(
            "file.apply_patch.hunk_format",
            "apply_patch received an invalid hunk body",
        )
    } else if contains("invalid patch") || contains("invalid hunk") {
        classified(
            "file.apply_patch.format",
            "apply_patch received an invalid patch format",
        )
    } else if contains("failed to find expected lines") {
        classified(
            "file.apply_patch.context_mismatch",
            "apply_patch context did not match the file",
        )
    } else if contains("utf-16") || contains("invalid utf-8") {
        classified(
            "file.encoding.unsupported",
            "File encoding was not supported",
        )
    } else if contains("no-op") || contains("does not change") {
        classified(
            "file.mutation.no_op",
            "File mutation made no byte-level change",
        )
    } else if contains("parsererror") || contains("unexpected token") && phase.contains("exec") {
        classified(
            "windows.powershell.parser",
            "PowerShell rejected the generated command",
        )
    } else if contains("command line is too long") || contains("filename or extension is too long")
    {
        classified(
            "windows.command_line.too_long",
            "Windows command-line length limit was exceeded",
        )
    } else if contains("timed out") || contains("timeout") {
        classified("operation.timeout", "Operation timed out")
    } else if tool == Some("apply_patch") {
        classified("file.apply_patch.failed", "apply_patch failed")
    } else {
        classified("unknown.failure", "Unclassified compatibility failure")
    }
}

fn classified(code: &'static str, summary: &'static str) -> ClassifiedError {
    ClassifiedError { code, summary }
}
