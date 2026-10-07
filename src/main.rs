mod error;
mod ext4;
mod matcher;
mod mounts;
mod sort;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use error::{Error, Result};
use matcher::Matcher;
use sort::{SortDir, SortKey};

const USAGE: &str = "\
wis - instant filename search on Linux

USAGE:
    wis [OPTIONS] <NAME>

Searches file names on the root filesystem by reading the ext4 metadata directly
from the block device, without walking the directory tree through the kernel.
Reading a block device requires root.

ARGS:
    <NAME>    What to look for in a file name: by default a case-insensitive
              substring, with --regex a regular expression

OPTIONS:
    -r, --regex             Treat <NAME> as a regular expression, searched within
                            the file name. Matching stays case-insensitive unless
                            the pattern says otherwise with (?-i)
    -n, --limit <N>         Print at most N results, applied after sorting
    -s, --sort <KEY> [DIR]  Sort by KEY in direction DIR, which defaults to asc.
                            KEY: name, path, ext, size, mtime
                            DIR: asc, desc
    -d, --device <PATH>     Scan this block device (or filesystem image) instead
                            of the filesystem mounted at /
    -j, --threads <N>       Concurrent metadata reads to keep in flight
                            (default 16; the scan is latency-bound, not CPU-bound)
    -h, --help              Print this help
    -V, --version           Print version

`size` and `mtime` cost one extra inode read per match, because the walk itself
only ever reads directory inodes.
";

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
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Dying quietly on a closed pipe (`| head`) is correct behaviour.
            if e.is_broken_pipe() {
                return ExitCode::SUCCESS;
            }
            eprintln!("wis: {e}");
            if e.is_permission_denied() {
                eprintln!("hint: reading a block device needs root -- try `sudo wis ...`");
            }
            if matches!(e, Error::Usage(_)) {
                eprint!("\n{USAGE}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;

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
        out.write_all(&hit.path).map_err(|e| Error::io("write to stdout", e))?;
        out.write_all(b"\n").map_err(|e| Error::io("write to stdout", e))?;
    }
    out.flush().map_err(|e| Error::io("flush stdout", e))?;

    let shown = if hits.len() == matched {
        format!("{matched} match(es)")
    } else {
        format!("{} of {matched} match(es)", hits.len())
    };
    eprintln!(
        "wis: {shown} in {:.3}s -- {} entries in {} dirs, {:.1} MiB read \
         ({} inodes, {} extent nodes) on {} threads",
        elapsed.as_secs_f64(),
        result.stats.entries,
        result.stats.dirs,
        result.stats.bytes_read as f64 / (1024.0 * 1024.0),
        result.stats.inode_reads,
        result.stats.extent_node_reads,
        threads,
    );
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut name: Option<String> = None;
    let mut device: Option<PathBuf> = None;
    let mut threads: Option<usize> = None;
    let mut regex = false;
    let mut limit: Option<usize> = None;
    let mut sort_key = SortKey::Path;
    let mut sort_dir = SortDir::Asc;

    let mut argv = std::env::args().skip(1).peekable();
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("wis {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-r" | "--regex" => regex = true,
            "-d" | "--device" => {
                let value = argv
                    .next()
                    .ok_or_else(|| Error::usage("--device requires a value"))?;
                device = Some(PathBuf::from(value));
            }
            "-n" | "--limit" => {
                let value = argv
                    .next()
                    .ok_or_else(|| Error::usage("--limit requires a value"))?;
                limit = Some(
                    value
                        .parse()
                        .map_err(|_| Error::usage(format!("`{value}` is not a count")))?,
                );
            }
            "-j" | "--threads" => {
                let value = argv
                    .next()
                    .ok_or_else(|| Error::usage("--threads requires a value"))?;
                let n: usize = value
                    .parse()
                    .map_err(|_| Error::usage(format!("`{value}` is not a thread count")))?;
                if n == 0 {
                    return Err(Error::usage("--threads must be at least 1"));
                }
                threads = Some(n);
            }
            "-s" | "--sort" => {
                let value = argv
                    .next()
                    .ok_or_else(|| Error::usage("--sort requires a key"))?;
                sort_key = SortKey::parse(&value).ok_or_else(|| {
                    Error::usage(format!(
                        "unknown sort key `{value}` (expected one of: {})",
                        SortKey::NAMES
                    ))
                })?;
                // The direction is optional, but only consume the next argument
                // if it really is one -- otherwise `wis -s size NAME` would eat
                // the search term.
                if let Some(dir) = argv.peek().and_then(|next| SortDir::parse(next)) {
                    sort_dir = dir;
                    argv.next();
                }
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(Error::usage(format!("unknown option `{other}`")));
            }
            _ => {
                if name.is_some() {
                    return Err(Error::usage("expected exactly one NAME"));
                }
                name = Some(arg);
            }
        }
    }

    let name = name.ok_or_else(|| Error::usage("missing NAME"))?;
    Ok(Args { name, device, threads, regex, limit, sort_key, sort_dir })
}

/// Find the block device backing `/`.
fn resolve_root_device() -> Result<PathBuf> {
    let mounts = mounts::read_mounts().map_err(|e| Error::io("read /proc/self/mounts", e))?;
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
        if other.fstype == "ext4" && other.target != entry.target && other.source.starts_with("/dev/")
        {
            eprintln!(
                "wis: note: {} at {} is a separate ext4 filesystem and is not searched yet",
                other.source,
                other.target.display()
            );
        }
    }

    Ok(PathBuf::from(&entry.source))
}
