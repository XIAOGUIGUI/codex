/// A privacy-safe description of suspicious text received from a model provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextIntegrityAnalysis {
    pub byte_count: usize,
    pub char_count: usize,
    pub flags: Vec<TextIntegrityFlag>,
}

impl TextIntegrityAnalysis {
    pub fn is_suspicious(&self) -> bool {
        !self.flags.is_empty()
    }
}

/// Deterministic signals that text was corrupted before local execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextIntegrityFlag {
    ReplacementCharacter,
    NullCharacter,
    UnexpectedControlCharacter,
    BidirectionalControlCharacter,
    LikelyMojibake,
    MixedAlphabeticScripts,
}

impl TextIntegrityFlag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplacementCharacter => "replacement_character",
            Self::NullCharacter => "null_character",
            Self::UnexpectedControlCharacter => "unexpected_control_character",
            Self::BidirectionalControlCharacter => "bidirectional_control_character",
            Self::LikelyMojibake => "likely_mojibake",
            Self::MixedAlphabeticScripts => "mixed_alphabetic_scripts",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AlphabeticScript {
    Latin,
    Cyrillic,
    Arabic,
    Devanagari,
}

pub fn analyze_text_integrity(text: &str) -> TextIntegrityAnalysis {
    let mut flags = Vec::new();
    if text.contains('\u{fffd}') {
        flags.push(TextIntegrityFlag::ReplacementCharacter);
    }
    if text.contains('\0') {
        flags.push(TextIntegrityFlag::NullCharacter);
    }
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        flags.push(TextIntegrityFlag::UnexpectedControlCharacter);
    }
    if text.chars().any(is_bidirectional_control) {
        flags.push(TextIntegrityFlag::BidirectionalControlCharacter);
    }
    if [
        "Ã©",
        "Ã¨",
        "Ã¼",
        "Ã¶",
        "Ã¤",
        "Ã±",
        "Ã£",
        "Â ",
        "â€",
        "ðŸ",
        "锟斤拷",
    ]
    .iter()
    .any(|marker| text.contains(marker))
    {
        flags.push(TextIntegrityFlag::LikelyMojibake);
    }
    if contains_mixed_script_word(text) {
        flags.push(TextIntegrityFlag::MixedAlphabeticScripts);
    }

    TextIntegrityAnalysis {
        byte_count: text.len(),
        char_count: text.chars().count(),
        flags,
    }
}

fn is_bidirectional_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn contains_mixed_script_word(text: &str) -> bool {
    let mut word_script = None;
    for character in text.chars() {
        if !character.is_alphabetic() {
            word_script = None;
            continue;
        }
        let Some(script) = alphabetic_script(character) else {
            continue;
        };
        if let Some(previous) = word_script
            && previous != script
        {
            return true;
        }
        word_script = Some(script);
    }
    false
}

fn alphabetic_script(character: char) -> Option<AlphabeticScript> {
    match character {
        'A'..='Z' | 'a'..='z' | '\u{00c0}'..='\u{024f}' => Some(AlphabeticScript::Latin),
        '\u{0400}'..='\u{052f}' => Some(AlphabeticScript::Cyrillic),
        '\u{0600}'..='\u{06ff}' | '\u{0750}'..='\u{077f}' | '\u{08a0}'..='\u{08ff}' => {
            Some(AlphabeticScript::Arabic)
        }
        '\u{0900}'..='\u{097f}' => Some(AlphabeticScript::Devanagari),
        _ => None,
    }
}

#[cfg(test)]
#[path = "text_integrity_tests.rs"]
mod tests;
