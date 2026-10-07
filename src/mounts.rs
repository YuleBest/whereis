//! Finding which block device backs a given path.
//!
//! For now the tool searches one filesystem at a time, so all we need is the
//! mount table: read `/proc/self/mounts` and pick the mount whose mount point is
//! the longest prefix of the path we care about.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MountEntry {
    pub source: String,
    pub target: PathBuf,
    pub fstype: String,
}

pub fn read_mounts() -> io::Result<Vec<MountEntry>> {
    let text = fs::read_to_string("/proc/self/mounts")?;
    Ok(text.lines().filter_map(parse_line).collect())
}

fn parse_line(line: &str) -> Option<MountEntry> {
    // device mountpoint fstype options dump pass
    let mut fields = line.split(' ');
    let source = fields.next()?;
    let target = fields.next()?;
    let fstype = fields.next()?;
    Some(MountEntry {
        source: unescape(source),
        target: PathBuf::from(unescape(target)),
        fstype: fstype.to_owned(),
    })
}

/// `/proc/self/mounts` escapes space, tab, newline and backslash as `\ooo`.
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &bytes[i + 1..i + 4];
            if oct.iter().all(|c| (b'0'..=b'7').contains(c)) {
                out.push((oct[0] - b'0') * 64 + (oct[1] - b'0') * 8 + (oct[2] - b'0'));
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mount whose mount point is the longest prefix of `path`.
///
/// `Path::starts_with` compares whole components, so `/run` never matches `/ru`.
pub fn mount_for_path<'a>(mounts: &'a [MountEntry], path: &Path) -> Option<&'a MountEntry> {
    mounts
        .iter()
        .filter(|m| path.starts_with(&m.target))
        .max_by_key(|m| m.target.as_os_str().len())
}
