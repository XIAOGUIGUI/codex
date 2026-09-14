use pretty_assertions::assert_eq;

use super::TextIntegrityFlag;
use super::analyze_text_integrity;

#[test]
fn accepts_normal_multilingual_text() {
    let analysis = analyze_text_integrity("git commit -m '修复 TUI 中文输出'");

    assert_eq!(analysis.flags, Vec::<TextIntegrityFlag>::new());
    assert_eq!(analysis.byte_count, 39);
    assert_eq!(analysis.char_count, 27);
}

#[test]
fn detects_provider_corruption_signals_without_retaining_content() {
    let analysis = analyze_text_integrity("validationcodes andтобы \u{fffd} Ã©");

    assert_eq!(
        analysis.flags,
        vec![
            TextIntegrityFlag::ReplacementCharacter,
            TextIntegrityFlag::LikelyMojibake,
            TextIntegrityFlag::MixedAlphabeticScripts,
        ]
    );
}

#[test]
fn detects_controls_that_can_change_terminal_rendering() {
    let analysis = analyze_text_integrity("safe\u{202e}txt\u{1b}");

    assert_eq!(
        analysis.flags,
        vec![
            TextIntegrityFlag::UnexpectedControlCharacter,
            TextIntegrityFlag::BidirectionalControlCharacter,
        ]
    );
}
