//! Deciding which file names match.
//!
//! Matching is always against the file *name*, never the full path, and always
//! on raw bytes, since ext4 does not require names to be UTF-8.

use std::ops::Range;

use regex::bytes::{Regex, RegexBuilder};

use crate::error::{Error, Result};
use crate::i18n;

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

    /// Append the ranges of every non-overlapping match in `name`, for
    /// highlighting. Zero-width matches are ignored: there is nothing to show.
    pub fn find_ranges(&self, name: &[u8], out: &mut Vec<Range<usize>>) {
        match self {
            Matcher::Substring(needle) => substring_ranges(name, needle, out),
            Matcher::Regex(re) => {
                for found in re.find_iter(name) {
                    if found.start() < found.end() {
                        out.push(found.start()..found.end());
                    }
                }
            }
        }
    }
}

/// Restricting a scan to part of the directory tree.
///
/// Without `--regex` the path is a literal absolute directory and only entries
/// strictly below it are searched. With `--regex` it is a regex matched against
/// directory paths, and every directory it matches is searched recursively.
/// Paths are interpreted on the on-disk tree: symlinks are never followed.
pub enum PathFilter {
    Literal(Vec<u8>),
    Regex(Matcher),
}

impl PathFilter {
    pub fn literal(path: &str) -> Result<Self> {
        if !path.starts_with('/') {
            return Err(Error::usage(i18n::t!(err_bad_path, value = path)));
        }
        let mut trimmed = path;
        while trimmed.len() > 1 && trimmed.ends_with('/') {
            trimmed = &trimmed[..trimmed.len() - 1];
        }
        Ok(PathFilter::Literal(trimmed.as_bytes().to_vec()))
    }

    pub fn regex(pattern: &str) -> Result<Self> {
        Ok(PathFilter::Regex(Matcher::regex(pattern)?))
    }

    /// Whether this directory's own entries are inside the scope.
    pub fn is_scope(&self, dir: &[u8]) -> bool {
        match self {
            PathFilter::Literal(root) => dir == root.as_slice(),
            PathFilter::Regex(matcher) => matcher.is_match(dir),
        }
    }

    /// Classify a child directory of a directory whose scope is `parent_scoped`:
    /// whether the child and its entries are in scope, and whether the walk has
    /// to enter it at all. A literal path prunes whole subtrees for free; a regex
    /// cannot, because it may match anywhere.
    pub fn enter(&self, parent_scoped: bool, dir: &[u8]) -> (bool, bool) {
        if parent_scoped {
            return (true, true);
        }
        match self {
            PathFilter::Literal(root) => {
                let scoped = dir == root.as_slice();
                (scoped, scoped || is_ancestor(dir, root))
            }
            PathFilter::Regex(matcher) => (matcher.is_match(dir), true),
        }
    }
}

/// Whether `dir` is a proper ancestor of `target`, comparing whole components so
/// `/etc` is never an ancestor of `/etcother`.
fn is_ancestor(dir: &[u8], target: &[u8]) -> bool {
    if dir == b"/" {
        return target != b"/";
    }
    target.len() > dir.len() && target.starts_with(dir) && target[dir.len()] == b'/'
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
    (0..=last_start).any(|start| matches_at(haystack, needle, start))
}

fn matches_at(haystack: &[u8], needle: &[u8], start: usize) -> bool {
    haystack[start..start + needle.len()]
        .iter()
        .zip(needle)
        .all(|(h, n)| h.to_ascii_lowercase() == *n)
}

fn substring_ranges(haystack: &[u8], needle: &[u8], out: &mut Vec<Range<usize>>) {
    if needle.is_empty() || needle.len() > haystack.len() {
        return;
    }
    let last_start = haystack.len() - needle.len();
    let mut start = 0;
    while start <= last_start {
        if matches_at(haystack, needle, start) {
            out.push(start..start + needle.len());
            start += needle.len();
        } else {
            start += 1;
        }
    }
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

    #[test]
    fn substring_ranges_are_case_insensitive_and_non_overlapping() {
        let mut ranges = Vec::new();
        Matcher::substring("ab").find_ranges(b"xABab", &mut ranges);
        assert_eq!(ranges, [1..3, 3..5]);

        // The empty needle matches everything, so there is nothing to show.
        let mut ranges = Vec::new();
        Matcher::substring("").find_ranges(b"abc", &mut ranges);
        assert!(ranges.is_empty());
    }

    #[test]
    fn regex_ranges_skip_zero_width_matches() {
        let mut ranges = Vec::new();
        Matcher::regex(r"[0-9]+")
            .unwrap()
            .find_ranges(b"a12b345", &mut ranges);
        assert_eq!(ranges, [1..3, 4..7]);

        let mut ranges = Vec::new();
        Matcher::regex(r"\d*")
            .unwrap()
            .find_ranges(b"abc", &mut ranges);
        assert!(ranges.is_empty());
    }

    #[test]
    fn literal_path_filter_scopes_and_prunes() {
        let f = PathFilter::literal("/etc").unwrap();
        assert!(!f.is_scope(b"/"));
        assert!(f.is_scope(b"/etc"));
        assert!(!f.is_scope(b"/etc/nginx"));
        assert_eq!(f.enter(false, b"/"), (false, true));
        assert_eq!(f.enter(false, b"/etc"), (true, true));
        assert_eq!(f.enter(false, b"/etc/nginx"), (false, false));
        assert_eq!(f.enter(true, b"/etc/nginx"), (true, true));
        assert_eq!(f.enter(false, b"/etcother"), (false, false));
        assert_eq!(f.enter(false, b"/lost+found"), (false, false));
        assert_eq!(f.enter(true, b"/anything"), (true, true));
    }

    #[test]
    fn literal_path_filter_normalizes_and_requires_absolute() {
        let f = PathFilter::literal("/var/log///").unwrap();
        assert!(f.is_scope(b"/var/log"));
        assert!(!f.is_scope(b"/"));

        let root = PathFilter::literal("/").unwrap();
        assert!(root.is_scope(b"/"));

        assert!(PathFilter::literal("var/log").is_err());
        assert!(PathFilter::literal("").is_err());
    }

    #[test]
    fn regex_path_filter_matches_directories_but_keeps_walking() {
        let f = PathFilter::regex(r"^/var/(log|tmp)$").unwrap();
        assert_eq!(f.enter(false, b"/var"), (false, true));
        assert_eq!(f.enter(false, b"/var/log"), (true, true));
        assert_eq!(f.enter(false, b"/usr"), (false, true));
        // Once a directory matched, its descendants stay in scope.
        assert_eq!(f.enter(true, b"/var/log/nginx"), (true, true));
        assert!(f.is_scope(b"/var/tmp"));
    }
}
