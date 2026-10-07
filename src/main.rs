mod error;
mod ext4;
mod mounts;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use error::{Error, Result};

const USAGE: &str = "\
wis - instant filename search on Linux

USAGE:
    wis [OPTIONS] <NAME>

Searches file names on the root filesystem by reading the ext4 metadata directly
from the block device, without walking the directory tree through the kernel.
Reading a block device requires root.

ARGS:
    <NAME>    Case-insensitive substring to look for in file names

OPTIONS:
    -d, --device <PATH>    Scan this block device (or filesystem image) instead
                           of the filesystem mounted at /
    -h, --help             Print this help
    -V, --version          Print version
";

struct Args {
    name: String,
    device: Option<PathBuf>,
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

    let mut scanner = ext4::Scanner::open(&device)?;
    let needle = args.name.as_bytes().to_ascii_lowercase();

    let started = Instant::now();
    let result = scanner.scan(|name| contains_ignore_ascii_case(name, &needle))?;
    let elapsed = started.elapsed();

    let mut paths = result.paths;
    paths.sort_unstable();

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    for path in &paths {
        out.write_all(path).map_err(|e| Error::io("write to stdout", e))?;
        out.write_all(b"\n").map_err(|e| Error::io("write to stdout", e))?;
    }
    out.flush().map_err(|e| Error::io("flush stdout", e))?;

    eprintln!(
        "wis: {} match(es) in {:.3}s -- {} entries in {} dirs, {:.1} MiB read",
        paths.len(),
        elapsed.as_secs_f64(),
        result.stats.entries,
        result.stats.dirs,
        result.stats.bytes_read as f64 / (1024.0 * 1024.0),
    );
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut name: Option<String> = None;
    let mut device: Option<PathBuf> = None;
    let mut argv = std::env::args().skip(1);

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
            "-d" | "--device" => {
                let value = argv
                    .next()
                    .ok_or_else(|| Error::usage("--device requires a value"))?;
                device = Some(PathBuf::from(value));
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
    Ok(Args { name, device })
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

/// Case-insensitive substring search over a file name.
///
/// `needle` must already be lowercased. Names are raw bytes, since ext4 does not
/// require them to be UTF-8.
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

#[cfg(test)]
mod tests {
    use super::contains_ignore_ascii_case;

    #[test]
    fn matches_case_insensitively() {
        assert!(contains_ignore_ascii_case(b"Makefile", b"make"));
        assert!(contains_ignore_ascii_case(b"README.md", b"readme"));
        assert!(contains_ignore_ascii_case(b"libfoo.so", b".so"));
    }

    #[test]
    fn rejects_non_matches() {
        assert!(!contains_ignore_ascii_case(b"Makefile", b"zzz"));
        assert!(!contains_ignore_ascii_case(b"ab", b"abc"));
    }

    #[test]
    fn empty_needle_matches_everything() {
        assert!(contains_ignore_ascii_case(b"anything", b""));
        assert!(contains_ignore_ascii_case(b"", b""));
    }
}
