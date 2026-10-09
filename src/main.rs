mod error;
mod ext4;
mod format;
mod history;
mod i18n;
mod listing;
mod logical;
mod matcher;
mod mounts;
mod sort;

use std::collections::HashSet;
use std::io::{self, IsTerminal, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use error::{Error, Result};
use ext4::{EntryKind, Hit, TypeFilter};
use format::Format;
use i18n::Lang;
use listing::{LineSpec, Names};
use logical::Query;
use matcher::{Matcher, PathFilter};
use sort::{SortDir, SortKey};

/// The scan is bound by device latency rather than by CPU, so this sits well
/// above the core count on purpose. Measured on this machine's SATA SSD, for the
/// same 40,000 random 4 KiB reads: 1 thread 5.95 s, 4 threads 1.49 s, 16 threads
/// 0.68 s, 32 threads 0.55 s.
const DEFAULT_THREADS: usize = 16;

/// Lines to print before asking whether to keep going. The question only appears
/// when stdin, stdout and stderr are all terminals, so pipes, redirects and
/// scripts always get the full output.
const PROMPT_AFTER: usize = 1000;

/// Colours are only ever written when stdout is a terminal and `NO_COLOR` is
/// unset, so pipes stay byte-clean. Directories are ls-blue, matches are
/// grep-red.
const COLOR_DIR: &[u8] = b"\x1b[1;34m";
const COLOR_HIT: &[u8] = b"\x1b[1;31m";
const COLOR_RESET: &[u8] = b"\x1b[0m";

struct Args {
    names: Vec<String>,
    device: Option<PathBuf>,
    path: Option<String>,
    types: Option<TypeFilter>,
    line: Option<String>,
    base: Option<usize>,
    format: Format,
    threads: Option<usize>,
    regex: bool,
    logical: bool,
    limit: Option<usize>,
    sort_key: SortKey,
    sort_dir: SortDir,
    clean: bool,
    quiet: bool,
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // The language has to be settled before anything at all can be reported,
    // argument errors included, so `--lang` is picked out of the raw arguments
    // before the real parse.
    i18n::set_lang(prescan_lang(&argv));

    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Dying quietly on a closed pipe (`| head`) is correct behaviour.
            if e.is_broken_pipe() {
                return ExitCode::SUCCESS;
            }
            eprintln!("wis: {e}");
            if e.is_permission_denied() {
                eprintln!("{}", i18n::t!(hint_root));
            }
            if matches!(e, Error::Usage(_)) {
                eprint!("\n{}", i18n::usage(i18n::lang()));
            }
            ExitCode::FAILURE
        }
    }
}

/// Look for `--lang` in the raw arguments. An unusable value is ignored here and
/// reported by the real parse, which runs in the environment's language.
fn prescan_lang(argv: &[String]) -> Lang {
    argv.iter()
        .rposition(|arg| arg == "--lang")
        .and_then(|i| argv.get(i + 1))
        .and_then(|value| Lang::parse(value))
        .unwrap_or_else(Lang::detect)
}

fn run(argv: &[String]) -> Result<()> {
    let args = parse_args(argv)?;

    let mut history = history::History::open();
    if let Some(error) = history.take_error() {
        if args.base.is_some() {
            return Err(error);
        }
        eprintln!("{}", i18n::t!(note_history_ignored, error = error));
    }
    let base = match args.base {
        Some(back) => match history.record(back) {
            Some(paths) => Some(paths.iter().cloned().collect::<HashSet<Vec<u8>>>()),
            None => {
                return Err(Error::usage(i18n::t!(
                    err_base_out_of_range,
                    num = back,
                    count = history.len()
                )))
            }
        },
        None => None,
    };

    let device = match &args.device {
        Some(explicit) => explicit.clone(),
        None => resolve_root_device()?,
    };

    let threads = args.threads.unwrap_or(DEFAULT_THREADS);
    let scanner = ext4::Scanner::open(&device, threads)?;
    let query = if args.logical {
        Query::logical(&args.names, args.regex)?
    } else {
        Query::simple(&args.names, args.regex)?
    };
    let terms = query.positive_terms();
    let path_filter = match &args.path {
        Some(value) if args.regex => Some(PathFilter::regex(value)?),
        Some(value) => Some(PathFilter::literal(value)?),
        None => None,
    };
    let line_spec = match (&args.line, args.clean) {
        (_, true) => None,
        (Some(value), false) => Some(LineSpec::parse(value)?),
        (None, false) => Some(LineSpec::default()),
    };

    let started = Instant::now();
    let result = scanner.scan(
        |name| query.is_match(name),
        path_filter.as_ref(),
        args.types.as_ref(),
        base.as_ref(),
    )?;
    let mut hits = result.hits;

    // The listing columns and size/mtime sorting both live in the inode.
    if (line_spec.is_some() || args.sort_key.needs_metadata()) && !hits.is_empty() {
        scanner.fill_metadata(&mut hits)?;
    }
    sort::apply(&mut hits, args.sort_key, args.sort_dir);
    let elapsed = started.elapsed();
    let mut stats = result.stats;
    scanner.refresh_stats(&mut stats);
    if let Err(error) = history.save(hits.iter().map(|hit| hit.path.as_slice())) {
        eprintln!("{}", i18n::t!(note_history_not_saved, error = error));
    }

    let matched = hits.len();
    if let Some(limit) = args.limit {
        hits.truncate(limit);
    }

    let format = args.format;
    let mut names = Names::default();
    let widths = if format == Format::Text {
        line_spec
            .as_ref()
            .map(|spec| listing::measure(spec, &hits, &mut names))
    } else {
        None
    };

    let stdout = io::stdout();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let dumb_terminal = std::env::var("TERM").is_ok_and(|value| value == "dumb");
    let color = stdout.is_terminal() && !no_color && !dumb_terminal;
    let interactive = !args.quiet
        && io::stdin().is_terminal()
        && io::stderr().is_terminal()
        && stdout.is_terminal();
    let mut out = io::BufWriter::new(stdout.lock());
    if format == Format::Json && !hits.is_empty() {
        out.write_all(b"[\n")
            .map_err(|e| Error::io(i18n::t!(io_write_stdout), e))?;
    }
    let mut printed = 0usize;
    let mut stopped = false;
    for hit in &hits {
        if printed == PROMPT_AFTER && interactive {
            out.flush()
                .map_err(|e| Error::io(i18n::t!(io_flush_stdout), e))?;
            if !confirm_continue(printed, hits.len() - printed) {
                stopped = true;
                break;
            }
        }
        let written = (|| -> io::Result<()> {
            match format {
                Format::Text => {
                    if let (Some(spec), Some(widths)) = (&line_spec, &widths) {
                        listing::write_row(&mut out, spec, widths, hit, &mut names)?;
                    }
                    write_path(&mut out, hit, color, &terms)
                }
                Format::Json => {
                    if printed > 0 {
                        out.write_all(b",\n")?;
                    }
                    out.write_all(b"  ")?;
                    format::write_json_object(&mut out, line_spec.as_ref(), hit, &mut names)
                }
                Format::Jsonl => {
                    format::write_json_object(&mut out, line_spec.as_ref(), hit, &mut names)?;
                    out.write_all(b"\n")
                }
                Format::Tsv | Format::Csv => {
                    format::write_delimited(&mut out, format, line_spec.as_ref(), hit, &mut names)?;
                    out.write_all(b"\n")
                }
            }
        })();
        written.map_err(|e| Error::io(i18n::t!(io_write_stdout), e))?;
        printed += 1;
    }
    if format == Format::Json {
        let close: &[u8] = if hits.is_empty() { b"[]\n" } else { b"\n]\n" };
        out.write_all(close)
            .map_err(|e| Error::io(i18n::t!(io_write_stdout), e))?;
    }
    out.flush()
        .map_err(|e| Error::io(i18n::t!(io_flush_stdout), e))?;

    let shown = if stopped {
        i18n::t!(matches_stopped, shown = printed, total = matched)
    } else if printed == matched {
        i18n::t!(matches_all, n = matched)
    } else {
        i18n::t!(matches_limited, shown = printed, total = matched)
    };
    if !args.clean {
        eprintln!(
            "wis: {}",
            i18n::t!(
                summary,
                shown = shown,
                secs = format!("{:.3}", elapsed.as_secs_f64()),
                entries = stats.entries,
                dirs = stats.dirs,
                mib = format!("{:.1}", stats.bytes_read as f64 / (1024.0 * 1024.0)),
                inodes = stats.inode_reads,
                nodes = stats.extent_node_reads,
                threads = threads,
            )
        );
    }
    Ok(())
}

fn parse_args(argv: &[String]) -> Result<Args> {
    let mut names: Vec<String> = Vec::new();
    let mut device: Option<PathBuf> = None;
    let mut path: Option<String> = None;
    let mut type_files = false;
    let mut type_dirs = false;
    let mut type_given = false;
    let mut line: Option<String> = None;
    let mut base: Option<usize> = None;
    let mut format = Format::Text;
    let mut threads: Option<usize> = None;
    let mut regex = false;
    let mut logical = false;
    let mut clean = false;
    let mut quiet = false;
    let mut limit: Option<usize> = None;
    let mut sort_key = SortKey::Path;
    let mut sort_dir = SortDir::Asc;

    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        i += 1;
        match arg {
            "-h" | "--help" => {
                print!("{}", i18n::usage(i18n::lang()));
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("wis {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-r" | "--regex" => regex = true,
            "-l" | "--logical" => logical = true,
            "-c" | "--clean" => clean = true,
            "-q" | "--quiet" => quiet = true,
            "-d" | "--device" => {
                device = Some(PathBuf::from(value_of(argv, &mut i, "--device")?));
            }
            "-p" | "--path" => {
                path = Some(value_of(argv, &mut i, "--path")?);
            }
            "-t" | "--type" => {
                let value = value_of(argv, &mut i, "--type")?;
                for token in value.split(',') {
                    match token.trim() {
                        "file" | "f" => type_files = true,
                        "directory" | "d" => type_dirs = true,
                        other => {
                            return Err(Error::usage(i18n::t!(
                                err_unknown_type,
                                value = other,
                                types = TypeFilter::NAMES
                            )))
                        }
                    }
                }
                type_given = true;
            }
            "--line" => {
                line = Some(value_of(argv, &mut i, "--line")?);
            }
            "--format" => {
                let value = value_of(argv, &mut i, "--format")?;
                format = Format::parse(&value)?;
            }
            "-b" | "--base" => {
                let value = value_of(argv, &mut i, "--base")?;
                let back: usize = value
                    .parse()
                    .map_err(|_| Error::usage(i18n::t!(err_not_a_number, value = value)))?;
                if back == 0 {
                    return Err(Error::usage(i18n::t!(err_base_min)));
                }
                base = Some(back);
            }
            "-n" | "--limit" => {
                let value = value_of(argv, &mut i, "--limit")?;
                limit = Some(
                    value
                        .parse()
                        .map_err(|_| Error::usage(i18n::t!(err_not_a_number, value = value)))?,
                );
            }
            "-j" | "--threads" => {
                let value = value_of(argv, &mut i, "--threads")?;
                let count: usize = value
                    .parse()
                    .map_err(|_| Error::usage(i18n::t!(err_not_a_number, value = value)))?;
                if count == 0 {
                    return Err(Error::usage(i18n::t!(err_threads_min)));
                }
                threads = Some(count);
            }
            "--lang" => {
                let value = value_of(argv, &mut i, "--lang")?;
                if Lang::parse(&value).is_none() {
                    return Err(Error::usage(i18n::t!(
                        err_unknown_lang,
                        value = value,
                        langs = i18n::SUPPORTED
                    )));
                }
            }
            "-s" | "--sort" => {
                let value = value_of(argv, &mut i, "--sort")?;
                sort_key = SortKey::parse(&value).ok_or_else(|| {
                    Error::usage(i18n::t!(
                        err_unknown_sort_key,
                        key = value,
                        keys = SortKey::NAMES
                    ))
                })?;
                // The direction is optional, and only consumed when it really is
                // one -- otherwise `wis -s size NAME` would eat the search term.
                if let Some(direction) = argv.get(i).and_then(|next| SortDir::parse(next)) {
                    sort_dir = direction;
                    i += 1;
                }
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(Error::usage(i18n::t!(err_unknown_option, option = other)));
            }
            _ => {
                names.push(arg.to_owned());
            }
        }
    }

    if names.is_empty() {
        return Err(Error::usage(i18n::t!(err_missing_name)));
    }
    Ok(Args {
        names,
        device,
        path,
        types: type_given.then_some(TypeFilter {
            files: type_files,
            dirs: type_dirs,
        }),
        line,
        base,
        format,
        threads,
        regex,
        logical,
        limit,
        sort_key,
        sort_dir,
        clean,
        quiet,
    })
}

/// Write one result line, colouring directories and the parts of the name that
/// matched. The path bytes themselves are never altered: no trailing slash, and
/// no escape sequences at all on a pipe.
fn write_path(out: &mut impl Write, hit: &Hit, color: bool, terms: &[&Matcher]) -> io::Result<()> {
    if !color {
        out.write_all(&hit.path)?;
        out.write_all(b"\n")?;
        return Ok(());
    }

    let ranges = highlight_ranges(&hit.path, terms);
    if ranges.is_empty() {
        if hit.kind == EntryKind::Dir {
            out.write_all(COLOR_DIR)?;
            out.write_all(&hit.path)?;
            out.write_all(COLOR_RESET)?;
        } else {
            out.write_all(&hit.path)?;
        }
        out.write_all(b"\n")?;
        return Ok(());
    }

    let dir = hit.kind == EntryKind::Dir;
    let base = if dir { Style::Dir } else { Style::Plain };
    let mut current = Style::Plain;
    let mut pos = 0;
    for range in ranges {
        write_styled(out, &hit.path[pos..range.start], base, &mut current)?;
        write_styled(
            out,
            &hit.path[range.start..range.end],
            Style::Hit,
            &mut current,
        )?;
        pos = range.end;
    }
    write_styled(out, &hit.path[pos..], base, &mut current)?;
    if current != Style::Plain {
        out.write_all(COLOR_RESET)?;
    }
    out.write_all(b"\n")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    Dir,
    Hit,
}

fn write_styled(
    out: &mut impl Write,
    bytes: &[u8],
    style: Style,
    current: &mut Style,
) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    if *current != style {
        let code = match style {
            Style::Plain => COLOR_RESET,
            Style::Dir => COLOR_DIR,
            Style::Hit => COLOR_HIT,
        };
        out.write_all(code)?;
        *current = style;
    }
    out.write_all(bytes)
}

/// The ranges of the file name that matched a positive term, merged and shifted
/// into the full path.
fn highlight_ranges(path: &[u8], terms: &[&Matcher]) -> Vec<Range<usize>> {
    if terms.is_empty() {
        return Vec::new();
    }
    let start = path
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);
    let name = &path[start..];
    let mut found = Vec::new();
    for term in terms {
        term.find_ranges(name, &mut found);
    }
    found.sort_by_key(|range| (range.start, range.end));

    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in found {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    for range in &mut merged {
        range.start += start;
        range.end += start;
    }
    merged
}

/// Once [`PROMPT_AFTER`] lines are on screen, ask whether to keep going. Only a
/// yes continues; end of input or a read error counts as no.
fn confirm_continue(shown: usize, remaining: usize) -> bool {
    eprint!(
        "{}",
        i18n::t!(prompt_continue, shown = shown, remaining = remaining)
    );
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(_) => answer_is_yes(&answer),
        Err(_) => false,
    }
}

fn answer_is_yes(answer: &str) -> bool {
    matches!(answer.trim().chars().next(), Some('y' | 'Y'))
}

fn value_of(argv: &[String], i: &mut usize, option: &str) -> Result<String> {
    let value = argv
        .get(*i)
        .ok_or_else(|| Error::usage(i18n::t!(err_option_needs_value, option = option)))?;
    *i += 1;
    Ok(value.clone())
}

/// Find the block device backing `/`.
fn resolve_root_device() -> Result<PathBuf> {
    let mounts = mounts::read_mounts().map_err(|e| Error::io(i18n::t!(io_read_mounts), e))?;
    let root = Path::new("/");

    let entry = mounts::mount_for_path(&mounts, root).ok_or_else(|| Error::NoBlockDevice {
        path: "/".to_string(),
        fstype: "unknown".to_string(),
    })?;

    if entry.fstype != "ext4" || !entry.source.starts_with("/dev/") {
        return Err(Error::NoBlockDevice {
            path: "/".to_string(),
            fstype: entry.fstype.clone(),
        });
    }

    // Other ext4 filesystems are out of scope for now, but silently searching
    // only part of the machine would be worse than saying so.
    for other in &mounts {
        if other.fstype == "ext4"
            && other.target != entry.target
            && other.source.starts_with("/dev/")
        {
            eprintln!(
                "{}",
                i18n::t!(
                    note_other_ext4,
                    device = other.source,
                    target = other.target.display()
                )
            );
        }
    }

    Ok(PathBuf::from(&entry.source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, kind: EntryKind) -> Hit {
        Hit {
            path: path.as_bytes().to_vec(),
            inode: 1,
            kind,
            mode: 0o644,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            mtime: 0,
        }
    }

    #[test]
    fn only_y_continues() {
        for yes in ["y", "Y", "yes", " yes \n"] {
            assert!(answer_is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "sure"] {
            assert!(!answer_is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn quiet_flag_is_accepted_in_both_spellings() {
        assert!(parse_args(&["-q".into(), "name".into()]).unwrap().quiet);
        assert!(
            parse_args(&["--quiet".into(), "name".into()])
                .unwrap()
                .quiet
        );
        assert!(!parse_args(&["name".into()]).unwrap().quiet);
    }

    #[test]
    fn path_option_is_parsed() {
        let args = parse_args(&["-p".into(), "/etc".into(), "name".into()]).unwrap();
        assert_eq!(args.path.as_deref(), Some("/etc"));
        let args = parse_args(&["--path".into(), "/var".into(), "name".into()]).unwrap();
        assert_eq!(args.path.as_deref(), Some("/var"));
        assert!(parse_args(&["name".into()]).unwrap().path.is_none());
    }

    #[test]
    fn base_option_is_parsed() {
        let args = parse_args(&["-b".into(), "2".into(), "name".into()]).unwrap();
        assert_eq!(args.base, Some(2));
        assert!(parse_args(&["--base".into(), "1".into(), "name".into()]).is_ok());
        assert!(parse_args(&["-b".into(), "0".into(), "name".into()]).is_err());
        assert!(parse_args(&["-b".into(), "x".into(), "name".into()]).is_err());
        assert!(parse_args(&["name".into()]).unwrap().base.is_none());
    }

    #[test]
    fn multiple_names_and_logical_flag_are_parsed() {
        let args = parse_args(&["foo".into(), "bar".into()]).unwrap();
        assert_eq!(args.names, ["foo", "bar"]);
        assert!(!args.logical);

        let args = parse_args(&["-l".into(), "foo AND bar".into(), "baz".into()]).unwrap();
        assert!(args.logical);
        assert_eq!(args.names.len(), 2);

        assert!(parse_args(&["-l".into()]).is_err());
    }

    #[test]
    fn format_option_is_parsed() {
        let args = parse_args(&["--format".into(), "jsonl".into(), "name".into()]).unwrap();
        assert_eq!(args.format, Format::Jsonl);
        assert_eq!(parse_args(&["name".into()]).unwrap().format, Format::Text);
        assert!(parse_args(&["--format".into(), "xml".into(), "name".into()]).is_err());
    }

    #[test]
    fn only_directories_are_coloured() {
        let dir = hit("/etc", EntryKind::Dir);
        let file = hit("/etc/passwd", EntryKind::File);

        let mut out = Vec::new();
        write_path(&mut out, &dir, true, &[]).unwrap();
        write_path(&mut out, &file, true, &[]).unwrap();
        write_path(&mut out, &dir, false, &[]).unwrap();
        assert_eq!(out, b"\x1b[1;34m/etc\x1b[0m\n/etc/passwd\n/etc\n");
    }

    #[test]
    fn matched_terms_are_highlighted_in_the_name() {
        let term = Matcher::substring("NGINX");
        let file = hit("/etc/nginx.conf", EntryKind::File);
        let dir = hit("/etc/nginx", EntryKind::Dir);

        let mut out = Vec::new();
        write_path(&mut out, &file, true, &[&term]).unwrap();
        assert_eq!(out, b"/etc/\x1b[1;31mnginx\x1b[0m.conf\n");

        out.clear();
        write_path(&mut out, &dir, true, &[&term]).unwrap();
        assert_eq!(out, b"\x1b[1;34m/etc/\x1b[1;31mnginx\x1b[0m\n");

        // A pipe gets the path untouched.
        out.clear();
        write_path(&mut out, &file, false, &[&term]).unwrap();
        assert_eq!(out, b"/etc/nginx.conf\n");

        // Overlapping terms merge into one span.
        let a = Matcher::substring("inx");
        let b = Matcher::substring("ngi");
        out.clear();
        write_path(&mut out, &file, true, &[&a, &b]).unwrap();
        assert_eq!(out, b"/etc/\x1b[1;31mnginx\x1b[0m.conf\n");
    }

    #[test]
    fn type_option_is_parsed_and_combined() {
        let args = parse_args(&["-t".into(), "f".into(), "name".into()]).unwrap();
        let types = args.types.unwrap();
        assert!(types.files && !types.dirs);

        let args = parse_args(&["-t".into(), "file, directory".into(), "name".into()]).unwrap();
        let types = args.types.unwrap();
        assert!(types.files && types.dirs);

        let args = parse_args(&[
            "-t".into(),
            "d".into(),
            "--type".into(),
            "f".into(),
            "name".into(),
        ])
        .unwrap();
        let types = args.types.unwrap();
        assert!(types.files && types.dirs);

        assert!(parse_args(&["name".into()]).unwrap().types.is_none());
        assert!(parse_args(&["-t".into(), "x".into(), "name".into()]).is_err());
        assert!(parse_args(&["-t".into(), "f,".into(), "name".into()]).is_err());
    }
}
