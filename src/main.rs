mod error;
mod ext4;
mod i18n;
mod matcher;
mod mounts;
mod sort;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use error::{Error, Result};
use i18n::Lang;
use matcher::Matcher;
use sort::{SortDir, SortKey};

/// The scan is bound by device latency rather than by CPU, so this sits well
/// above the core count on purpose. Measured on this machine's SATA SSD, for the
/// same 40,000 random 4 KiB reads: 1 thread 5.95 s, 4 threads 1.49 s, 16 threads
/// 0.68 s, 32 threads 0.55 s.
const DEFAULT_THREADS: usize = 16;

struct Args {
    name: String,
    device: Option<PathBuf>,
    threads: Option<usize>,
    regex: bool,
    limit: Option<usize>,
    sort_key: SortKey,
    sort_dir: SortDir,
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

    let device = match &args.device {
        Some(explicit) => explicit.clone(),
        None => resolve_root_device()?,
    };

    let threads = args.threads.unwrap_or(DEFAULT_THREADS);
    let scanner = ext4::Scanner::open(&device, threads)?;
    let matcher = if args.regex {
        Matcher::regex(&args.name)?
    } else {
        Matcher::substring(&args.name)
    };

    let started = Instant::now();
    let result = scanner.scan(|name| matcher.is_match(name))?;
    let mut hits = result.hits;

    // Only pay for inode reads when the requested key actually needs them.
    if args.sort_key.needs_metadata() && !hits.is_empty() {
        scanner.fill_metadata(&mut hits)?;
    }
    sort::apply(&mut hits, args.sort_key, args.sort_dir);
    let elapsed = started.elapsed();

    let matched = hits.len();
    if let Some(limit) = args.limit {
        hits.truncate(limit);
    }

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    for hit in &hits {
        out.write_all(&hit.path)
            .map_err(|e| Error::io(i18n::t!(io_write_stdout), e))?;
        out.write_all(b"\n")
            .map_err(|e| Error::io(i18n::t!(io_write_stdout), e))?;
    }
    out.flush()
        .map_err(|e| Error::io(i18n::t!(io_flush_stdout), e))?;

    let shown = if hits.len() == matched {
        i18n::t!(matches_all, n = matched)
    } else {
        i18n::t!(matches_limited, shown = hits.len(), total = matched)
    };
    let stats = result.stats;
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
    Ok(())
}

fn parse_args(argv: &[String]) -> Result<Args> {
    let mut name: Option<String> = None;
    let mut device: Option<PathBuf> = None;
    let mut threads: Option<usize> = None;
    let mut regex = false;
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
            "-d" | "--device" => {
                device = Some(PathBuf::from(value_of(argv, &mut i, "--device")?));
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
                if name.is_some() {
                    return Err(Error::usage(i18n::t!(err_expected_one_name)));
                }
                name = Some(arg.to_owned());
            }
        }
    }

    let name = name.ok_or_else(|| Error::usage(i18n::t!(err_missing_name)))?;
    Ok(Args {
        name,
        device,
        threads,
        regex,
        limit,
        sort_key,
        sort_dir,
    })
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
