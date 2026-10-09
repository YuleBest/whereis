//! Long-listing columns printed before each path.
//!
//! `--line` picks which columns appear and in what order. Their widths are
//! measured over the whole result set, so file names start at the same column;
//! the path is always last.

use std::collections::HashMap;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::ops::Range;

use crate::error::{Error, Result};
use crate::ext4::{EntryKind, Hit};
use crate::i18n;
use crate::logical::Query;

/// A column that can appear before the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Type,
    Mode,
    Mtime(MtimeFormat),
    Size(SizeUnit),
    Spans,
    User,
    Group,
    Nlink,
}

/// How the mtime column is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtimeFormat {
    /// Local time like `ls -l`, the default.
    Local,
    /// Seconds since the Unix epoch.
    Timestamp,
}

/// How the size column is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeUnit {
    /// The largest unit that keeps the number readable, the default.
    Auto,
    /// Bytes with a trailing `B`.
    Bytes,
    /// Raw bytes, no unit at all.
    Raw,
    K,
    M,
    G,
    T,
}

/// The name of a column, used as the key of structured output.
pub fn field_name(field: &Field) -> &'static str {
    match field {
        Field::Type => "type",
        Field::Mode => "mode",
        Field::Mtime(_) => "mtime",
        Field::Size(_) => "size",
        Field::Spans => "spans",
        Field::User => "user",
        Field::Group => "group",
        Field::Nlink => "nlink",
    }
}

/// The byte range of the file name inside a path.
pub fn name_range(path: &[u8]) -> Range<usize> {
    let start = path
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);
    start..path.len()
}

/// The parsed `--line` value.
#[derive(Debug, Clone)]
pub struct LineSpec {
    fields: Vec<Field>,
}

impl Default for LineSpec {
    fn default() -> Self {
        LineSpec {
            fields: vec![
                Field::Mode,
                Field::Mtime(MtimeFormat::Local),
                Field::Size(SizeUnit::Auto),
            ],
        }
    }
}

impl LineSpec {
    pub const FIELD_NAMES: &'static str = "type, mode, mtime, size, spans, user, group, nlink";
    pub const UNIT_NAMES: &'static str = "auto, raw, b, k, m, g, t";

    pub fn parse(value: &str) -> Result<Self> {
        let mut fields = Vec::new();
        for token in value.split(',') {
            let token = token.trim();
            let (name, unit) = match token.split_once('=') {
                Some((name, unit)) => (name.trim(), Some(unit.trim())),
                None => (token, None),
            };
            let field = match (name, unit) {
                ("type", None) => Field::Type,
                ("mode", None) => Field::Mode,
                ("mtime", None) => Field::Mtime(MtimeFormat::Local),
                ("mtime", Some(unit)) if unit.eq_ignore_ascii_case("timestamp") => {
                    Field::Mtime(MtimeFormat::Timestamp)
                }
                ("size", None) => Field::Size(SizeUnit::Auto),
                ("size", Some(unit)) => Field::Size(SizeUnit::parse(unit)?),
                ("spans", None) => Field::Spans,
                ("user", None) => Field::User,
                ("group", None) => Field::Group,
                ("nlink", None) => Field::Nlink,
                _ => {
                    return Err(Error::usage(i18n::t!(
                        err_unknown_line_field,
                        field = token,
                        fields = Self::FIELD_NAMES
                    )))
                }
            };
            fields.push(field);
        }
        Ok(LineSpec { fields })
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }
}

impl SizeUnit {
    fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(SizeUnit::Auto),
            "b" => Ok(SizeUnit::Bytes),
            "raw" => Ok(SizeUnit::Raw),
            "k" => Ok(SizeUnit::K),
            "m" => Ok(SizeUnit::M),
            "g" => Ok(SizeUnit::G),
            "t" => Ok(SizeUnit::T),
            _ => Err(Error::usage(i18n::t!(
                err_unknown_size_unit,
                unit = value,
                units = LineSpec::UNIT_NAMES
            ))),
        }
    }
}

/// Cached uid/gid lookups, filled on demand.
#[derive(Default)]
pub struct Names {
    users: HashMap<u32, String>,
    groups: HashMap<u32, String>,
}

impl Names {
    fn user(&mut self, uid: u32) -> &str {
        self.users
            .entry(uid)
            .or_insert_with(|| lookup_user(uid))
            .as_str()
    }

    fn group(&mut self, gid: u32) -> &str {
        self.groups
            .entry(gid)
            .or_insert_with(|| lookup_group(gid))
            .as_str()
    }
}

fn lookup_user(uid: u32) -> String {
    let mut buf = [0u8; 1024];
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
            && !passwd.pw_name.is_null()
    };
    if found {
        unsafe { CStr::from_ptr(passwd.pw_name) }
            .to_string_lossy()
            .into_owned()
    } else {
        uid.to_string()
    }
}

fn lookup_group(gid: u32) -> String {
    let mut buf = [0u8; 1024];
    let mut group: libc::group = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let found = unsafe {
        libc::getgrgid_r(
            gid,
            &mut group,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        ) == 0
            && !result.is_null()
            && !group.gr_name.is_null()
    };
    if found {
        unsafe { CStr::from_ptr(group.gr_name) }
            .to_string_lossy()
            .into_owned()
    } else {
        gid.to_string()
    }
}

/// Column widths, one per requested field.
pub struct Widths {
    widths: Vec<usize>,
}

/// Measure every requested column over the result set.
pub fn measure(spec: &LineSpec, hits: &[Hit], names: &mut Names, query: &Query) -> Widths {
    let mut widths = vec![0; spec.fields.len()];
    let mut cell = String::new();
    for hit in hits {
        for (i, field) in spec.fields().iter().enumerate() {
            // The local-time format is fixed width, so measuring it would only
            // cost a localtime_r call per hit before anything is printed.
            let width = if *field == Field::Mtime(MtimeFormat::Local) {
                19
            } else {
                render(field, hit, names, query, &mut cell);
                cell.chars().count()
            };
            widths[i] = widths[i].max(width);
        }
    }
    Widths { widths }
}

/// Write one row of columns, each padded to its measured width and followed by
/// a space. The caller appends the path.
pub fn write_row(
    out: &mut impl Write,
    spec: &LineSpec,
    widths: &Widths,
    hit: &Hit,
    names: &mut Names,
    query: &Query,
) -> io::Result<()> {
    let mut cell = String::new();
    for (i, field) in spec.fields().iter().enumerate() {
        render(field, hit, names, query, &mut cell);
        let pad = widths.widths[i].saturating_sub(cell.chars().count());
        if right_aligned(field) {
            write_spaces(out, pad)?;
            out.write_all(cell.as_bytes())?;
        } else {
            out.write_all(cell.as_bytes())?;
            write_spaces(out, pad)?;
        }
        out.write_all(b" ")?;
    }
    Ok(())
}

fn right_aligned(field: &Field) -> bool {
    matches!(
        field,
        Field::Mode | Field::Mtime(_) | Field::Size(_) | Field::Nlink
    )
}

fn write_spaces(out: &mut impl Write, mut count: usize) -> io::Result<()> {
    const SPACES: &[u8] = b"                                ";
    while count > 0 {
        let take = count.min(SPACES.len());
        out.write_all(&SPACES[..take])?;
        count -= take;
    }
    Ok(())
}

/// Render one column into `out`, without padding or colours.
pub fn render(field: &Field, hit: &Hit, names: &mut Names, query: &Query, out: &mut String) {
    out.clear();
    match field {
        Field::Type => out.push_str(kind_name(hit.kind)),
        Field::Mode => {
            let _ = write!(out, "{:04o}", hit.mode & 0o7777);
        }
        Field::Mtime(MtimeFormat::Local) => out.push_str(&format_mtime(hit.mtime)),
        Field::Mtime(MtimeFormat::Timestamp) => {
            let _ = write!(out, "{}", hit.mtime);
        }
        Field::Size(unit) => out.push_str(&format_size(hit.size, *unit)),
        Field::Spans => {
            let name = &hit.path[name_range(&hit.path)];
            for (i, span) in query.spans(name).iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{}-{}", span.start, span.end);
            }
        }
        Field::User => out.push_str(names.user(hit.uid)),
        Field::Group => out.push_str(names.group(hit.gid)),
        Field::Nlink => {
            let _ = write!(out, "{}", hit.nlink);
        }
    }
}

fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "directory",
        EntryKind::Other => "other",
    }
}

fn format_size(bytes: u64, unit: SizeUnit) -> String {
    match unit {
        SizeUnit::Auto => human_size(bytes),
        SizeUnit::Bytes => format!("{bytes}B"),
        SizeUnit::Raw => bytes.to_string(),
        SizeUnit::K => scaled_size(bytes, 1 << 10, "K"),
        SizeUnit::M => scaled_size(bytes, 1 << 20, "M"),
        SizeUnit::G => scaled_size(bytes, 1 << 30, "G"),
        SizeUnit::T => scaled_size(bytes, 1 << 40, "T"),
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1 << 40, "T"),
        (1 << 30, "G"),
        (1 << 20, "M"),
        (1 << 10, "K"),
    ];
    for (factor, suffix) in UNITS {
        if bytes >= factor {
            return scaled_size(bytes, factor, suffix);
        }
    }
    format!("{bytes}B")
}

fn scaled_size(bytes: u64, factor: u64, suffix: &str) -> String {
    format!("{:.1}{suffix}", bytes as f64 / factor as f64)
}

/// `YYYY-MM-DD HH:MM:SS` in the local timezone, like `ls -l`.
fn format_mtime(secs: i64) -> String {
    let time = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return secs.to_string();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_spec_parses_fields_in_order() {
        let spec = LineSpec::parse("type,mtime=timestamp,size=raw,spans,nlink").unwrap();
        assert_eq!(
            spec.fields(),
            &[
                Field::Type,
                Field::Mtime(MtimeFormat::Timestamp),
                Field::Size(SizeUnit::Raw),
                Field::Spans,
                Field::Nlink
            ]
        );
        assert_eq!(
            LineSpec::parse("size").unwrap().fields(),
            &[Field::Size(SizeUnit::Auto)]
        );
        assert_eq!(
            LineSpec::parse("size=K").unwrap().fields(),
            &[Field::Size(SizeUnit::K)]
        );
        assert_eq!(
            LineSpec::parse("mtime").unwrap().fields(),
            &[Field::Mtime(MtimeFormat::Local)]
        );
    }

    #[test]
    fn line_spec_rejects_unknown_fields_and_units() {
        assert!(LineSpec::parse("owner").is_err());
        assert!(LineSpec::parse("mtime=utc").is_err());
        assert!(LineSpec::parse("size=x").is_err());
        assert!(LineSpec::parse("").is_err());
    }

    #[test]
    fn sizes_use_the_requested_unit() {
        assert_eq!(format_size(0, SizeUnit::Bytes), "0B");
        assert_eq!(format_size(1536, SizeUnit::Raw), "1536");
        assert_eq!(format_size(4096, SizeUnit::K), "4.0K");
        assert_eq!(format_size(1536, SizeUnit::Auto), "1.5K");
        assert_eq!(format_size(1 << 20, SizeUnit::Auto), "1.0M");
        assert_eq!(format_size(999, SizeUnit::Auto), "999B");
        assert_eq!(format_size(4096, SizeUnit::M), "0.0M");
    }

    #[test]
    fn mtime_is_formatted_like_ls() {
        let text = format_mtime(0);
        assert_eq!(text.len(), 19, "{text}");
        assert_eq!(&text[4..5], "-");
        assert_eq!(&text[7..8], "-");
        assert_eq!(&text[10..11], " ");
        assert_eq!(&text[13..14], ":");
        assert_eq!(&text[16..17], ":");
    }
}
