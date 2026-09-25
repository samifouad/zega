//! Folding for the `…Like` text operators (zegadb/zega#98): case- and
//! accent-insensitive matching, as opposed to the byte-exact `…Exact`
//! operators and `=`.
//!
//! The fold is: decompose to NFD, drop every combining mark (accents), then
//! apply Rust's Unicode uppercase mapping. Uppercase, not lowercase, because
//! its special casing expands `ß` to `SS`, which is what makes
//! `"STRASSE".foldLike("straße")` true; lowercasing never introduces `ß` back
//! from `"SS"`. Order matters only a little: `ß` has no NFD decomposition, so
//! stripping marks first and folding case second, or the reverse, agree on
//! every case this module is tested against.
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

fn fold(text: &str) -> String {
    text.nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_uppercase()
}

/// `…Like` takes plain text, never SQL `%`/`_` wildcards: folding does not
/// interpret them, so they match only literally.
pub fn contains(haystack: &str, needle: &str) -> bool {
    fold(haystack).contains(&fold(needle))
}

pub fn starts_with(haystack: &str, needle: &str) -> bool {
    fold(haystack).starts_with(&fold(needle))
}

pub fn ends_with(haystack: &str, needle: &str) -> bool {
    fold(haystack).ends_with(&fold(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_only_ascii() {
        assert!(contains("Webhook fired", "webhook"));
        assert!(starts_with("Webhook", "WEB"));
        assert!(ends_with("Webhook", "HOOK"));
    }

    #[test]
    fn folds_accents_both_directions() {
        assert!(contains("Café Society", "cafe"));
        assert!(contains("cafe society", "Café"));
        assert!(starts_with("São Paulo", "sao"));
        assert!(starts_with("Sao Paulo", "São"));
    }

    #[test]
    fn folds_expanding_special_casing() {
        // ß case-folds to "ss" under full Unicode case folding.
        assert!(contains("STRASSE", "straße"));
        assert!(contains("straße", "STRASSE"));
    }

    #[test]
    fn folds_greek_and_cyrillic() {
        assert!(contains("ΟΔΥΣΣΕΑΣ", "οδυσσεας"));
        assert!(contains("привет", "ПРИВЕТ"));
    }

    #[test]
    fn percent_sign_is_literal_not_a_wildcard() {
        assert!(contains("50% off", "50%"));
        assert!(!contains("50 off", "50%"));
    }

    #[test]
    fn distinct_text_does_not_fold_together() {
        assert!(!contains("Webhook", "widget"));
        assert!(!starts_with("Café", "za"));
        assert!(!ends_with("Café", "za"));
    }
}
