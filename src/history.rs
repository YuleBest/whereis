//! The last few result sets, so `--base` can search inside an earlier one.
//!
//! The file lives under the XDG state directory. It is replaced atomically and,
//! when the tool runs under sudo, handed to the invoking user so the history is
//! not root-owned in their home.
//!
//! Format: the magic line, then for every record each path followed by a NUL
//! byte and one extra NUL to close the record. A path cannot contain NUL and
//! cannot be empty, so the separators are unambiguous and file names with
//! newlines survive.

use std::env;
use std::ffi::{CStr, CString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::i18n;

const MAGIC: &[u8] = b"wis_history 1";
const MAX_RECORDS: usize = 10;
const FILE: &str = "wis_history";

pub struct History {
    path: Option<PathBuf>,
    /// The invoking user when we are root on their behalf; the file is chowned
    /// to them so later non-sudo runs keep working.
    owner: Option<(u32, u32)>,
    /// Oldest first, newest last; never longer than [`MAX_RECORDS`].
    records: Vec<Vec<Vec<u8>>>,
    error: Option<Error>,
}

impl History {
    /// Open the history. Problems are kept for the caller to report: a search
    /// does not depend on the history unless `--base` is used.
    pub fn open() -> Self {
        let Some((path, owner)) = state_file() else {
            return History {
                path: None,
                owner: None,
                records: Vec::new(),
                error: Some(no_path_error()),
            };
        };
        let (records, error) = match fs::read(&path) {
            Ok(data) => (parse(&data), None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Vec::new(), None),
            Err(e) => (
                Vec::new(),
                Some(Error::io(
                    i18n::t!(io_read_history, path = path.display()),
                    e,
                )),
            ),
        };
        History {
            path: Some(path),
            owner,
            records,
            error,
        }
    }

    pub fn take_error(&mut self) -> Option<Error> {
        self.error.take()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// A record counted back from the most recent one: `1` is the newest.
    pub fn record(&self, back: usize) -> Option<&[Vec<u8>]> {
        let index = self.records.len().checked_sub(back)?;
        self.records.get(index).map(Vec::as_slice)
    }

    /// Append one full result set, drop anything past [`MAX_RECORDS`], and
    /// write the file back.
    pub fn save<'a>(&mut self, paths: impl IntoIterator<Item = &'a [u8]>) -> Result<()> {
        let record: Vec<Vec<u8>> = paths.into_iter().map(<[u8]>::to_vec).collect();
        self.records.push(record);
        if self.records.len() > MAX_RECORDS {
            let excess = self.records.len() - MAX_RECORDS;
            self.records.drain(..excess);
        }
        self.write()
    }

    fn write(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Err(no_path_error());
        };
        if let Some(dir) = path.parent() {
            create_dir_owned(dir, self.owner).map_err(|e| write_error(path, e))?;
        }

        let temp = temp_path(path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)
            .map_err(|e| write_error(path, e))?;
        let written = file
            .write_all(&encode(&self.records))
            .and_then(|()| file.flush());
        drop(file);
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(write_error(path, e));
        }

        if let Some((uid, gid)) = self.owner {
            // Best effort: a root-owned history in the user's home is still
            // usable, just untidy.
            let _ = chown(&temp, uid, gid);
        }
        fs::rename(&temp, path).map_err(|e| {
            let _ = fs::remove_file(&temp);
            write_error(path, e)
        })
    }
}

fn no_path_error() -> Error {
    Error::io(
        i18n::t!(io_history_path),
        io::Error::new(io::ErrorKind::NotFound, "HOME is not set"),
    )
}

fn write_error(path: &Path, source: io::Error) -> Error {
    Error::io(i18n::t!(io_write_history, path = path.display()), source)
}

fn temp_path(path: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    path.with_file_name(format!("{FILE}.tmp.{}.{}", std::process::id(), nanos))
}

/// The state file of the invoking user, plus the ownership to give the file
/// when we are root on their behalf.
fn state_file() -> Option<(PathBuf, Option<(u32, u32)>)> {
    if let Some(uid) = sudo_uid() {
        if let Some((home, gid)) = passwd_entry(uid) {
            let gid = sudo_gid().unwrap_or(gid);
            return Some((home.join(".local/state").join(FILE), Some((uid, gid))));
        }
    }
    let home = env::var_os("HOME").filter(|value| !value.is_empty())?;
    let dir = match env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
        _ => PathBuf::from(home).join(".local/state"),
    };
    Some((dir.join(FILE), None))
}

fn sudo_uid() -> Option<u32> {
    let uid = env::var("SUDO_UID").ok()?.parse().ok()?;
    (uid != unsafe { libc::geteuid() }).then_some(uid)
}

fn sudo_gid() -> Option<u32> {
    env::var("SUDO_GID").ok()?.parse().ok()
}

fn passwd_entry(uid: u32) -> Option<(PathBuf, u32)> {
    let mut buf = [0u8; 4096];
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let found = unsafe {
        libc::getpwuid_r(
            uid,
            &mut passwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        ) == 0
            && !result.is_null()
            && !passwd.pw_dir.is_null()
    };
    if !found {
        return None;
    }
    let home = unsafe { CStr::from_ptr(passwd.pw_dir) }
        .to_string_lossy()
        .into_owned();
    if home.is_empty() {
        None
    } else {
        Some((PathBuf::from(home), passwd.pw_gid))
    }
}

fn create_dir_owned(dir: &Path, owner: Option<(u32, u32)>) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        if !parent.as_os_str().is_empty() {
            create_dir_owned(parent, owner)?;
        }
    }
    match fs::create_dir(dir) {
        Ok(()) => {
            if let Some((uid, gid)) = owner {
                let _ = chown(dir, uid, gid);
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

fn chown(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    };
    if unsafe { libc::chown(path.as_ptr(), uid, gid) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn encode(records: &[Vec<Vec<u8>>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(0);
    for record in records {
        for path in record {
            out.extend_from_slice(path);
            out.push(0);
        }
        out.push(0);
    }
    out
}

/// Read as many complete records as possible. A missing magic, a truncated
/// record or an impossible path drops the damaged part rather than failing the
/// search: history is a convenience, not source of truth.
fn parse(data: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let Some(rest) = data
        .strip_prefix(MAGIC)
        .and_then(|rest| rest.strip_prefix(b"\0"))
    else {
        return Vec::new();
    };

    let mut items: Vec<&[u8]> = rest.split(|&byte| byte == 0).collect();
    // The split leaves an empty item after the final NUL; a missing terminator
    // means the last item is a partial path. Neither belongs to a record.
    items.pop();

    let mut records = Vec::new();
    let mut current = Vec::new();
    for item in items {
        if item.is_empty() {
            records.push(std::mem::take(&mut current));
        } else if item.starts_with(b"/") {
            current.push(item.to_vec());
        }
    }
    // An unterminated record never reached a separator, so it was dropped.
    if records.len() > MAX_RECORDS {
        records.drain(..records.len() - MAX_RECORDS);
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(paths: &[&[u8]]) -> Vec<Vec<u8>> {
        paths.iter().map(|path| path.to_vec()).collect()
    }

    #[test]
    fn round_trips_records_including_empty_and_odd_names() {
        let records = vec![
            record(&[b"/etc/passwd", b"/a/file with\nnewline"]),
            record(&[]),
            record(&[b"/"]),
        ];
        assert_eq!(parse(&encode(&records)), records);
    }

    #[test]
    fn missing_magic_yields_no_history() {
        assert!(parse(b"not our file\0").is_empty());
        assert!(parse(b"").is_empty());
    }

    #[test]
    fn truncated_records_are_dropped() {
        let mut data = encode(&[record(&[b"/keep"]), record(&[b"/lost"])]);
        data.pop(); // chop the final terminator
        assert_eq!(parse(&data), vec![record(&[b"/keep"])]);
    }

    #[test]
    fn only_the_last_ten_records_survive() {
        let records: Vec<Vec<Vec<u8>>> = (0..12)
            .map(|i| record(&[format!("/f{i}").as_bytes()]))
            .collect();
        let parsed = parse(&encode(&records));
        assert_eq!(parsed.len(), MAX_RECORDS);
        assert_eq!(parsed.first().unwrap()[0], b"/f2");
        assert_eq!(parsed.last().unwrap()[0], b"/f11");
    }
}
