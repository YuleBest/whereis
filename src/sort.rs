//! Ordering of results.
//!
//! Two of the keys (`size`, `mtime`) are not known during the scan, because the
//! scan only reads *directory* inodes -- the entries it finds are names, not
//! metadata. Those keys cost one extra inode read per match, done in parallel and
//! only when the key is actually asked for.
//!
//! The rules are: sort by the requested key in the requested direction, and break
//! ties by ascending path so the output is deterministic.

use std::cmp::Ordering;

use crate::ext4::Hit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Path,
    Ext,
    Size,
    Mtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortKey {
    pub const NAMES: &'static str = "name, path, ext, size, mtime";

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "name" => Some(SortKey::Name),
            "path" => Some(SortKey::Path),
            "ext" => Some(SortKey::Ext),
            "size" => Some(SortKey::Size),
            "mtime" | "time" => Some(SortKey::Mtime),
            _ => None,
        }
    }

    /// Whether this key needs each match's inode read.
    pub fn needs_metadata(self) -> bool {
        matches!(self, SortKey::Size | SortKey::Mtime)
    }
}

impl SortDir {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "asc" | "ascending" | "正序" => Some(SortDir::Asc),
            "desc" | "descending" | "倒序" => Some(SortDir::Desc),
            _ => None,
        }
    }
}

/// A precomputed sort key.
///
/// String keys are held as a range into the entry's own path rather than as a
/// copy, so preparing 500,000 keys allocates nothing.
enum Key {
    /// The entry's whole path.
    Path,
    /// A slice of the entry's path, `start` and `len` bytes into it.
    Slice { start: u32, len: u32 },
    /// A number read from the inode.
    Number(u64),
}

pub fn apply(hits: &mut Vec<Hit>, key: SortKey, dir: SortDir) {
    if hits.len() < 2 {
        return;
    }

    // Sorting by path needs no preparation at all, so sort in place.
    if key == SortKey::Path {
        hits.sort_unstable_by(|a, b| match dir {
            SortDir::Asc => a.path.cmp(&b.path),
            SortDir::Desc => b.path.cmp(&a.path),
        });
        return;
    }

    let mut decorated: Vec<(Key, Hit)> = std::mem::take(hits)
        .into_iter()
        .map(|hit| (prepare(key, &hit), hit))
        .collect();

    decorated.sort_unstable_by(|(ka, ha), (kb, hb)| {
        let primary = compare(ka, ha, kb, hb);
        let primary = match dir {
            SortDir::Asc => primary,
            SortDir::Desc => primary.reverse(),
        };
        // Ties always fall back to ascending path order, which makes the
        // comparator a total order -- so an unstable sort is still deterministic.
        primary.then_with(|| ha.path.cmp(&hb.path))
    });

    *hits = decorated.into_iter().map(|(_, hit)| hit).collect();
}

fn prepare(key: SortKey, hit: &Hit) -> Key {
    match key {
        SortKey::Path => Key::Path,
        SortKey::Name => {
            let (start, len) = name_range(&hit.path);
            Key::Slice { start, len }
        }
        SortKey::Ext => {
            let (start, len) = extension_range(&hit.path);
            Key::Slice { start, len }
        }
        SortKey::Size => Key::Number(hit.size),
        SortKey::Mtime => Key::Number(hit.mtime as u64),
    }
}

fn compare(ka: &Key, ha: &Hit, kb: &Key, hb: &Hit) -> Ordering {
    match (ka, kb) {
        (Key::Path, Key::Path) => ha.path.cmp(&hb.path),
        (Key::Slice { start: a, len: n }, Key::Slice { start: b, len: m }) => {
            slice(&ha.path, *a, *n).cmp(slice(&hb.path, *b, *m))
        }
        (Key::Number(a), Key::Number(b)) => a.cmp(b),
        _ => Ordering::Equal,
    }
}

fn slice(path: &[u8], start: u32, len: u32) -> &[u8] {
    &path[start as usize..(start + len) as usize]
}

/// The part of a path after the last `/`.
fn file_name(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&c| c == b'/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// The extension of a path's last component, without the dot. Empty when there is
/// none, and a leading dot does not count -- the same rule as `Path::extension`.
fn extension(path: &[u8]) -> &[u8] {
    let name = file_name(path);
    match name.iter().rposition(|&c| c == b'.') {
        Some(0) | None => b"",
        Some(i) => &name[i + 1..],
    }
}

fn name_range(path: &[u8]) -> (u32, u32) {
    let name = file_name(path);
    ((path.len() - name.len()) as u32, name.len() as u32)
}

fn extension_range(path: &[u8]) -> (u32, u32) {
    let ext = extension(path);
    ((path.len() - ext.len()) as u32, ext.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, size: u64, mtime: u32) -> Hit {
        Hit {
            path: path.as_bytes().to_vec(),
            inode: 1,
            size,
            mtime,
        }
    }

    fn paths(hits: &[Hit]) -> Vec<String> {
        hits.iter()
            .map(|h| String::from_utf8_lossy(&h.path).into_owned())
            .collect()
    }

    #[test]
    fn name_key_uses_the_last_component() {
        assert_eq!(file_name(b"/etc/ssh/sshd_config"), b"sshd_config");
        assert_eq!(file_name(b"/etc"), b"etc");
        assert_eq!(file_name(b"plain"), b"plain");
    }

    #[test]
    fn extension_ignores_a_leading_dot() {
        assert_eq!(extension(b"/a/b.tar.gz"), b"gz");
        assert_eq!(extension(b"/a/.bashrc"), b"");
        assert_eq!(extension(b"/a/plain"), b"");
    }

    #[test]
    fn ranges_locate_the_same_slices_as_the_helpers() {
        for path in [
            &b"/etc/ssh/sshd_config"[..],
            b"/a/b.tar.gz",
            b"/a/.bashrc",
            b"plain",
            b"/",
            b"/trailing/",
        ] {
            let (start, len) = name_range(path);
            assert_eq!(slice(path, start, len), file_name(path), "name of {path:?}");
            let (start, len) = extension_range(path);
            assert_eq!(slice(path, start, len), extension(path), "ext of {path:?}");
        }
    }

    #[test]
    fn sorts_by_path_in_both_directions() {
        let mut hits = vec![hit("/b", 0, 0), hit("/a", 0, 0)];
        apply(&mut hits, SortKey::Path, SortDir::Asc);
        assert_eq!(paths(&hits), ["/a", "/b"]);
        apply(&mut hits, SortKey::Path, SortDir::Desc);
        assert_eq!(paths(&hits), ["/b", "/a"]);
    }

    #[test]
    fn descending_reverses_the_key_but_not_the_tie_break() {
        let mut hits = vec![
            hit("/z/small", 1, 0),
            hit("/a/big", 9, 0),
            hit("/b/big", 9, 0),
        ];
        apply(&mut hits, SortKey::Size, SortDir::Desc);
        // Both 9-byte entries come first, in ascending path order among themselves.
        assert_eq!(paths(&hits), ["/a/big", "/b/big", "/z/small"]);
    }

    #[test]
    fn sorts_by_name_and_extension() {
        let mut hits = vec![hit("/x/b.tar.gz", 0, 0), hit("/x/a.txt", 0, 0)];
        apply(&mut hits, SortKey::Name, SortDir::Asc);
        assert_eq!(paths(&hits), ["/x/a.txt", "/x/b.tar.gz"]);

        let mut hits = vec![hit("/x/b.tar.gz", 0, 0), hit("/x/a.txt", 0, 0)];
        apply(&mut hits, SortKey::Ext, SortDir::Asc);
        assert_eq!(paths(&hits), ["/x/b.tar.gz", "/x/a.txt"]); // gz < txt
    }

    #[test]
    fn a_single_hit_is_left_alone() {
        let mut hits = vec![hit("/a", 0, 0)];
        apply(&mut hits, SortKey::Size, SortDir::Asc);
        assert_eq!(paths(&hits), ["/a"]);
    }
}
