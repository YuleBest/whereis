//! Structured output: JSON, JSONL, CSV and TSV.
//!
//! Every format writes the columns chosen by `--line` in order, then the path.
//! Values are rendered exactly as in the text format; only the separators and
//! escaping differ. Text itself lives in `listing`, because it measures and
//! colours the columns.

use std::io::{self, Write};

use crate::error::{Error, Result};
use crate::ext4::Hit;
use crate::i18n;
use crate::listing::{self, LineSpec, Names};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    Json,
    Jsonl,
    Tsv,
    Csv,
    #[default]
    Text,
}

impl Format {
    pub const NAMES: &'static str = "json, jsonl, tsv, csv, text";

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Format::Json),
            "jsonl" => Ok(Format::Jsonl),
            "tsv" => Ok(Format::Tsv),
            "csv" => Ok(Format::Csv),
            "text" => Ok(Format::Text),
            _ => Err(Error::usage(i18n::t!(
                err_unknown_format,
                format = value,
                formats = Self::NAMES
            ))),
        }
    }
}

/// One JSON object: the `--line` fields in order, then the path. Every value is
/// a string, rendered the same way as the text columns.
pub fn write_json_object(
    out: &mut impl Write,
    spec: Option<&LineSpec>,
    hit: &Hit,
    names: &mut Names,
) -> io::Result<()> {
    out.write_all(b"{")?;
    let mut first = true;
    let mut value = String::new();
    if let Some(spec) = spec {
        for field in spec.fields() {
            if !first {
                out.write_all(b",")?;
            }
            first = false;
            write_json_key(out, listing::field_name(field))?;
            listing::render(field, hit, names, &mut value);
            write_json_string(out, value.as_bytes())?;
        }
    }
    if !first {
        out.write_all(b",")?;
    }
    write_json_key(out, "path")?;
    write_json_string(out, &hit.path)?;
    out.write_all(b"}")
}

/// One CSV or TSV record: the `--line` fields in order, then the path.
pub fn write_delimited(
    out: &mut impl Write,
    format: Format,
    spec: Option<&LineSpec>,
    hit: &Hit,
    names: &mut Names,
) -> io::Result<()> {
    let csv = match format {
        Format::Csv => true,
        Format::Tsv => false,
        _ => unreachable!("delimited output is CSV or TSV only"),
    };
    let separator: &[u8] = if csv { b"," } else { b"\t" };
    let mut first = true;
    let mut value = String::new();
    if let Some(spec) = spec {
        for field in spec.fields() {
            if !first {
                out.write_all(separator)?;
            }
            first = false;
            listing::render(field, hit, names, &mut value);
            write_delimited_field(out, value.as_bytes(), csv)?;
        }
    }
    if !first {
        out.write_all(separator)?;
    }
    write_delimited_field(out, &hit.path, csv)
}

fn write_json_key(out: &mut impl Write, key: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    out.write_all(key.as_bytes())?;
    out.write_all(b"\":")
}

fn write_json_string(out: &mut impl Write, value: &[u8]) -> io::Result<()> {
    out.write_all(b"\"")?;
    for c in String::from_utf8_lossy(value).chars() {
        match c {
            '"' => out.write_all(b"\\\"")?,
            '\\' => out.write_all(b"\\\\")?,
            '\n' => out.write_all(b"\\n")?,
            '\r' => out.write_all(b"\\r")?,
            '\t' => out.write_all(b"\\t")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => {
                let mut buf = [0u8; 4];
                out.write_all(c.encode_utf8(&mut buf).as_bytes())?;
            }
        }
    }
    out.write_all(b"\"")
}

fn write_delimited_field(out: &mut impl Write, field: &[u8], csv: bool) -> io::Result<()> {
    if csv {
        write_csv_field(out, field)
    } else {
        write_tsv_field(out, field)
    }
}

/// RFC 4180 quoting: fields with a comma, quote, newline or carriage return are
/// wrapped in quotes, and inner quotes are doubled.
fn write_csv_field(out: &mut impl Write, field: &[u8]) -> io::Result<()> {
    if !field
        .iter()
        .any(|&byte| matches!(byte, b',' | b'"' | b'\n' | b'\r'))
    {
        return out.write_all(field);
    }
    out.write_all(b"\"")?;
    let mut start = 0;
    for (i, &byte) in field.iter().enumerate() {
        if byte == b'"' {
            out.write_all(&field[start..i])?;
            out.write_all(b"\"\"")?;
            start = i + 1;
        }
    }
    out.write_all(&field[start..])?;
    out.write_all(b"\"")
}

/// TSV has no quoting rules, so the characters that would break a row are
/// backslash-escaped instead.
fn write_tsv_field(out: &mut impl Write, field: &[u8]) -> io::Result<()> {
    let mut start = 0;
    for (i, &byte) in field.iter().enumerate() {
        let escape: &[u8] = match byte {
            b'\\' => b"\\\\",
            b'\t' => b"\\t",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            _ => continue,
        };
        out.write_all(&field[start..i])?;
        out.write_all(escape)?;
        start = i + 1;
    }
    out.write_all(&field[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext4::EntryKind;

    fn hit(path: &[u8]) -> Hit {
        Hit {
            path: path.to_vec(),
            inode: 1,
            kind: EntryKind::File,
            mode: 0o644,
            nlink: 2,
            uid: 0,
            gid: 0,
            size: 1536,
            mtime: 0,
        }
    }

    fn spec(value: &str) -> LineSpec {
        LineSpec::parse(value).unwrap()
    }

    fn record(format: Format, spec: &LineSpec, hit: &Hit) -> String {
        let mut out = Vec::new();
        let mut names = Names::default();
        match format {
            Format::Json | Format::Jsonl => {
                write_json_object(&mut out, Some(spec), hit, &mut names).unwrap()
            }
            Format::Csv | Format::Tsv => {
                write_delimited(&mut out, format, Some(spec), hit, &mut names).unwrap()
            }
            Format::Text => unreachable!(),
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn json_follows_line_and_escapes() {
        let text = record(
            Format::Json,
            &spec("mode,size=b,nlink"),
            &hit(b"/a b,baz\"q\""),
        );
        assert_eq!(
            text,
            r#"{"mode":"0644","size":"1536B","nlink":"2","path":"/a b,baz\"q\""}"#
        );
    }

    #[test]
    fn csv_quotes_separators_and_quotes() {
        let text = record(Format::Csv, &spec("mode,size=b"), &hit(b"/a,b\"c"));
        assert_eq!(text, "0644,1536B,\"/a,b\"\"c\"");
    }

    #[test]
    fn tsv_escapes_control_characters() {
        let text = record(Format::Tsv, &spec("nlink"), &hit(b"/a\tb\nc\\d"));
        assert_eq!(text, "2\t/a\\tb\\nc\\\\d");
    }

    #[test]
    fn json_replaces_non_utf8_bytes() {
        let text = record(Format::Json, &spec("mode"), &hit(b"/bad\xff"));
        assert!(text.contains("path\":\"/bad\u{fffd}\""), "{text}");
    }

    #[test]
    fn a_clean_record_only_carries_the_path() {
        let mut out = Vec::new();
        let mut names = Names::default();
        write_json_object(&mut out, None, &hit(b"/plain"), &mut names).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), r#"{"path":"/plain"}"#);
    }
}
