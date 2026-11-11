//! Fuzzy matching with a quality check.
//!
//! Plain subsequence matching finds far too much: "fire" matches
//! "LibreOffice Impress" because those four letters appear somewhere in
//! that order. A match is called *strict* when it is something a person
//! would recognise:
//!
//! * the letters sit together ("fox" in "Firefox"), or
//! * every group of letters starts a word ("vsc" in "Visual Studio
//!   Code", "wifi" in "Wi-Fi").
//!
//! The launcher shows strict matches when there are any, and only falls
//! back to loose ones when nothing better exists.

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Matcher, Utf32Str};

/// How well a query matched some text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Higher is better. Only comparable between matches of one query.
    pub score: u32,
    /// The match is one a person would recognise. See the module docs.
    pub strict: bool,
}

/// A parsed query: one fuzzy pattern per whitespace-separated word. All
/// words have to match.
pub struct Query {
    atoms: Vec<Atom>,
    /// The query's words, lowercased, for the plain substring check.
    words: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Query {
        let atoms = text
            .split_whitespace()
            .map(|word| {
                Atom::new(
                    word,
                    CaseMatching::Ignore,
                    Normalization::Smart,
                    AtomKind::Fuzzy,
                    false,
                )
            })
            .collect();
        let words = text.split_whitespace().map(str::to_lowercase).collect();
        Query { atoms, words }
    }

    /// Matches the query against `haystack`. `None` means some word of
    /// the query does not appear at all.
    pub fn score(&self, haystack: &str, matcher: &mut Matcher) -> Option<Match> {
        if self.atoms.is_empty() {
            return None;
        }
        let chars: Vec<char> = haystack.chars().collect();
        let mut buf = Vec::new();
        let text = Utf32Str::new(haystack, &mut buf);
        let mut total = 0u32;
        let mut strict = true;
        let mut indices = Vec::new();
        let lowered = haystack.to_lowercase();
        for (atom, word) in self.atoms.iter().zip(&self.words) {
            indices.clear();
            let score = atom.indices(text, matcher, &mut indices)?;
            total += score as u32;
            indices.sort_unstable();
            indices.dedup();
            // The matcher reports the alignment it scored best, which
            // can be a scattered one even when the word also appears
            // whole ("fox" in "Firefox" aligns to F..ox). So check for
            // the whole word separately.
            strict &= lowered.contains(word.as_str()) || is_recognisable(&chars, &indices);
        }
        Some(Match {
            score: total,
            strict,
        })
    }
}

/// Whether the character at `index` begins a word: the start of the
/// text, after a separator, or a capital following a lowercase letter.
fn starts_word(chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let (Some(&previous), Some(&current)) = (chars.get(index - 1), chars.get(index)) else {
        return false;
    };
    !previous.is_alphanumeric() || (previous.is_lowercase() && current.is_uppercase())
}

/// Applies the rule from the module docs to the matched positions.
fn is_recognisable(chars: &[char], indices: &[u32]) -> bool {
    let mut run_starts = Vec::new();
    let mut previous: Option<u32> = None;
    for &index in indices {
        if previous.map(|p| p + 1) != Some(index) {
            run_starts.push(index as usize);
        }
        previous = Some(index);
    }
    run_starts.len() <= 1 || run_starts.iter().all(|&start| starts_word(chars, start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleo_matcher::Config;

    fn check(query: &str, text: &str) -> Option<Match> {
        let mut matcher = Matcher::new(Config::DEFAULT);
        Query::new(query).score(text, &mut matcher)
    }

    fn strict(query: &str, text: &str) -> bool {
        check(query, text).is_some_and(|m| m.strict)
    }

    fn loose(query: &str, text: &str) -> bool {
        check(query, text).is_some_and(|m| !m.strict)
    }

    #[test]
    fn letters_together_are_strict() {
        assert!(strict("fire", "Firefox"));
        assert!(strict("fox", "Firefox"));
        assert!(strict("blu", "Night light (blue light filter)"));
        assert!(strict("report", "q3-report.pdf"));
    }

    #[test]
    fn word_initials_are_strict() {
        assert!(strict("vsc", "Visual Studio Code"));
        assert!(strict("wifi", "Wi-Fi"));
        assert!(strict("lo", "LibreOffice"));
        assert!(strict("gte", "GNOME Text Editor"));
    }

    #[test]
    fn scattered_letters_are_only_loose() {
        assert!(loose("fire", "LibreOffice Impress"));
        assert!(loose("ff", "Firefox"));
        assert!(loose("frfx", "Firefox"));
    }

    #[test]
    fn missing_letters_do_not_match_at_all() {
        assert_eq!(check("xyz", "Firefox"), None);
        assert_eq!(check("firefoxes", "Firefox"), None);
    }

    #[test]
    fn every_word_of_the_query_must_match() {
        assert!(strict("arch pack", "Arch package search"));
        assert!(strict("search arch", "Arch package search"));
        assert_eq!(check("arch zebra", "Arch package search"), None);
    }

    #[test]
    fn matching_ignores_case_and_accents() {
        assert!(strict("FIRE", "firefox"));
        assert!(strict("cafe", "Café Notes"));
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert_eq!(check("", "Firefox"), None);
        assert_eq!(check("   ", "Firefox"), None);
    }

    #[test]
    fn better_matches_score_higher() {
        let prefix = check("fire", "Firefox").unwrap().score;
        let scattered = check("fire", "LibreOffice Impress").unwrap().score;
        assert!(prefix > scattered);
    }

    #[test]
    fn non_ascii_text_does_not_panic() {
        assert!(check("日本", "日本語入力").is_some());
        assert!(check("e", "🙂 emoji").is_some());
    }
}
