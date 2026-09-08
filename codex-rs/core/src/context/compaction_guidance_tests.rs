use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parse_trims_and_renders_guidance() {
    let guidance = CompactionGuidance::parse(Some("  preserve the build evidence  ".to_string()))
        .expect("guidance should be valid")
        .expect("guidance should be present");

    assert_eq!(
        guidance.render(),
        "<compaction_guidance>\npreserve the build evidence\n</compaction_guidance>"
    );
}

#[test]
fn parse_enforces_utf8_byte_limit() {
    let at_limit = "a".repeat(MAX_COMPACTION_GUIDANCE_BYTES);
    let over_limit = format!("{at_limit}a");

    assert!(CompactionGuidance::parse(Some(at_limit)).is_ok());
    assert_eq!(
        CompactionGuidance::parse(Some(over_limit)),
        Err(format!(
            "compaction guidance exceeds the {MAX_COMPACTION_GUIDANCE_BYTES}-byte limit"
        ))
    );
}
