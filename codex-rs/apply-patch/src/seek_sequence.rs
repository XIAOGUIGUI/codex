/// Attempt to find the sequence of `pattern` lines within `lines` beginning at or after `start`.
/// Returns the starting index of the match or `None` if not found. Matches are attempted with
/// decreasing strictness: exact match, then ignoring trailing whitespace, then normalising common
/// Unicode punctuation while still preserving leading whitespace. When `eof` is true, we first
/// try starting at the end-of-file (so that patterns intended to match file endings are applied at
/// the end), and fall back to searching from `start` if needed.
///
/// Special cases handled defensively:
///  • Empty `pattern` → returns `Some(start)` (no-op match)
///  • `pattern.len() > lines.len()` → returns `None` (cannot match, avoids
///    out‑of‑bounds panic that occurred pre‑2025‑04‑12)
pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
    update_file_mode: crate::ApplyPatchFileUpdateMode,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }

    // When the pattern is longer than the available input there is no possible
    // match. Early‑return to avoid the out‑of‑bounds slice that would occur in
    // the search loops below (previously caused a panic when
    // `pattern.len() > lines.len()`).
    if pattern.len() > lines.len() {
        return None;
    }

    let final_start = lines.len() - pattern.len();
    let can_match_at_eof = match update_file_mode {
        crate::ApplyPatchFileUpdateMode::NormalizeToLf => true,
        crate::ApplyPatchFileUpdateMode::PreserveLineEndings => final_start >= start,
    };
    if eof
        && can_match_at_eof
        && sequence_matches_in_range(lines, pattern, final_start, final_start).is_some()
    {
        return Some(final_start);
    }

    sequence_matches_in_range(lines, pattern, start, final_start)
}

fn sequence_matches_in_range(
    lines: &[String],
    pattern: &[String],
    search_start: usize,
    search_end: usize,
) -> Option<usize> {
    if search_start > search_end {
        return None;
    }

    // Exact match first.
    for i in search_start..=search_end {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    // Then rstrip match.
    for i in search_start..=search_end {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim_end() != pat.trim_end() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }
    // ------------------------------------------------------------------
    // Final, most permissive pass – attempt to match after *normalising*
    // common Unicode punctuation to their ASCII equivalents so that diffs
    // authored with plain ASCII characters can still be applied to source
    // files that contain typographic dashes / quotes, etc.  This mirrors the
    // fuzzy behaviour of `git apply` which ignores minor byte-level
    // differences when locating context lines.
    // ------------------------------------------------------------------

    fn normalise(s: &str) -> String {
        s.trim_end()
            .chars()
            .map(|c| match c {
                // Various dash / hyphen code-points → ASCII '-'
                '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
                | '\u{2212}' => '-',
                // Fancy single quotes → '\''
                '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
                // Fancy double quotes → '"'
                '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
                // Non-breaking space and other odd spaces → normal space
                '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
                | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
                | '\u{3000}' => ' ',
                other => other,
            })
            .collect::<String>()
    }

    for i in search_start..=search_end {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if normalise(&lines[i + p_idx]) != normalise(pat) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::seek_sequence;
    use crate::ApplyPatchFileUpdateMode;
    use std::string::ToString;

    fn to_vec(strings: &[&str]) -> Vec<String> {
        strings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn test_exact_match_finds_sequence() {
        let lines = to_vec(&["foo", "bar", "baz"]);
        let pattern = to_vec(&["bar", "baz"]);
        assert_eq!(
            seek_sequence(
                &lines,
                &pattern,
                /*start*/ 0,
                /*eof*/ false,
                ApplyPatchFileUpdateMode::NormalizeToLf,
            ),
            Some(1)
        );
    }

    #[test]
    fn test_rstrip_match_ignores_trailing_whitespace() {
        let lines = to_vec(&["foo   ", "bar\t\t"]);
        // Pattern omits trailing whitespace.
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(
            seek_sequence(
                &lines,
                &pattern,
                /*start*/ 0,
                /*eof*/ false,
                ApplyPatchFileUpdateMode::NormalizeToLf,
            ),
            Some(0)
        );
    }

    #[test]
    fn test_leading_whitespace_mismatch_is_rejected() {
        let lines = to_vec(&["    foo   ", "   bar\t"]);
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(
            seek_sequence(
                &lines,
                &pattern,
                /*start*/ 0,
                /*eof*/ false,
                ApplyPatchFileUpdateMode::NormalizeToLf,
            ),
            None
        );
    }

    #[test]
    fn test_unicode_match_preserves_leading_whitespace() {
        let lines = to_vec(&["    ‘quoted’ — value   "]);
        let matching_pattern = to_vec(&["    'quoted' - value"]);
        let mismatched_pattern = to_vec(&["   'quoted' - value"]);

        assert_eq!(
            seek_sequence(
                &lines,
                &matching_pattern,
                /*start*/ 0,
                /*eof*/ false,
                ApplyPatchFileUpdateMode::NormalizeToLf,
            ),
            Some(0)
        );
        assert_eq!(
            seek_sequence(
                &lines,
                &mismatched_pattern,
                /*start*/ 0,
                /*eof*/ false,
                ApplyPatchFileUpdateMode::NormalizeToLf,
            ),
            None
        );
    }

    #[test]
    fn test_pattern_longer_than_input_returns_none() {
        let lines = to_vec(&["just one line"]);
        let pattern = to_vec(&["too", "many", "lines"]);
        // Should not panic – must return None when pattern cannot possibly fit.
        assert_eq!(
            seek_sequence(
                &lines,
                &pattern,
                /*start*/ 0,
                /*eof*/ false,
                ApplyPatchFileUpdateMode::NormalizeToLf,
            ),
            None
        );
    }

    #[test]
    fn test_eof_match_prefers_file_end() {
        let lines = to_vec(&["target", "middle", "target   "]);
        let pattern = to_vec(&["target"]);

        assert_eq!(
            seek_sequence(
                &lines,
                &pattern,
                /*start*/ 0,
                /*eof*/ true,
                ApplyPatchFileUpdateMode::PreserveLineEndings,
            ),
            Some(2)
        );
    }

    #[test]
    fn test_eof_match_falls_back_to_search_start() {
        let lines = to_vec(&["before", "target", "not the target"]);
        let pattern = to_vec(&["target"]);

        assert_eq!(
            seek_sequence(
                &lines,
                &pattern,
                /*start*/ 1,
                /*eof*/ true,
                ApplyPatchFileUpdateMode::PreserveLineEndings,
            ),
            Some(1)
        );
    }

    #[test]
    fn test_eof_match_does_not_move_before_search_start_in_preserve_mode() {
        let lines = to_vec(&["target"]);
        let pattern = to_vec(&["target"]);

        assert_eq!(
            seek_sequence(
                &lines,
                &pattern,
                /*start*/ 1,
                /*eof*/ true,
                ApplyPatchFileUpdateMode::PreserveLineEndings,
            ),
            None
        );
    }
}
