//! Deciding which file names match.
//!
//! Matching is always against the file *name*, never the full path, and always
//! on raw bytes, since ext4 does not require names to be UTF-8.

use regex::bytes::{Regex, RegexBuilder};

use crate::error::{Error, Result};

pub enum Matcher {
    /// Case-insensitive substring. The needle is stored pre-lowercased.
    Substring(Vec<u8>),
    /// Regular expression, searched (not anchored) within the name.
    Regex(Box<Regex>),
}

impl Matcher {
    pub fn substring(needle: &str) -> Self {
        Matcher::Substring(needle.as_bytes().to_ascii_lowercase())
    }

    /// `--regex` changes the pattern language and nothing else, so matching stays
    /// case-insensitive by default the way substring mode is. A pattern can opt
    /// back out with `(?-i)`.
    pub fn regex(pattern: &str) -> Result<Self> {
        let re = RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map_err(|e| Error::bad_pattern(tidy(&e)))?;
        Ok(Matcher::Regex(Box::new(re)))
    }

    pub fn is_match(&self, name: &[u8]) -> bool {
        match self {
            Matcher::Substring(needle) => contains_ignore_ascii_case(name, needle),
            Matcher::Regex(re) => re.is_match(name),
        }
    }
}

/// Case-insensitive substring search. `needle` must already be lowercased.
fn contains_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    let last_start = haystack.len() - needle.len();
    (0..=last_start).any(|start| {
        haystack[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(h, n)| h.to_ascii_lowercase() == *n)
    })
}

/// `regex::Error` prints a multi-line diagram; squash it onto one line so it fits
/// in a normal error message.
fn tidy(error: &regex::Error) -> String {
    error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_matches_case_insensitively() {
        let m = Matcher::substring("make");
        assert!(m.is_match(b"Makefile"));
        assert!(m.is_match(b"GNUmakefile"));
        assert!(!m.is_match(b"other"));
    }

    #[test]
    fn empty_substring_matches_everything() {
        let m = Matcher::substring("");
        assert!(m.is_match(b"anything"));
        assert!(m.is_match(b""));
    }

    #[test]
    fn regex_searches_rather_than_anchors() {
        let m = Matcher::regex(r"^\d+\.md$").unwrap();
        assert!(m.is_match(b"2024.md"));
        assert!(!m.is_match(b"x2024.md"));
        assert!(!m.is_match(b"2024.txt"));
    }

    #[test]
    fn regex_is_case_insensitive_unless_told_otherwise() {
        assert!(Matcher::regex("readme").unwrap().is_match(b"README"));
        assert!(!Matcher::regex("(?-i)readme").unwrap().is_match(b"README"));
        assert!(Matcher::regex("(?-i)README").unwrap().is_match(b"README"));
    }

    #[test]
    fn regex_matches_non_utf8_names_bytewise() {
        let m = Matcher::regex(r"\.crt$").unwrap();
        assert!(m.is_match(b"NetLock_Arany_F\xc5\x91tan\xc3\xbas\xc3\xadtv\xc3\xa1ny.crt"));
    }

    #[test]
    fn bad_regex_is_reported() {
        assert!(Matcher::regex("a(b").is_err());
    }
}
