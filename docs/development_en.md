# Development

[简体中文](development.md) · **English**

## Building and testing

```sh
cargo build --release     # produces target/release/wis
cargo test
cargo clippy --all-targets

cargo run -- --help
sudo ./target/release/wis sshd_config
```

Rust 1.75+ is required (`std::thread::scope`, `let-else`, `div_ceil`).

Two things that help while debugging: `-d` accepts any block device or filesystem
image, so you do not have to test against the root filesystem; and `-j 1` falls
back to a single thread, which makes before/after comparisons easier. The summary
on stderr reports the bytes, inodes and extent nodes actually read.

## Code layout

| File | Responsibility |
|---|---|
| `src/main.rs` | Argument parsing, language resolution, orchestration, output |
| `src/ext4.rs` | ext4 on-disk decoder and the batched walk |
| `src/i18n.rs` | Language selection and the three-language message catalogue |
| `src/history.rs` | History: state file location, NUL-separated records, last 10 kept |
| `src/listing.rs` | Output columns: `--line` parsing, widths, mode/time/size and user rendering |
| `src/logical.rs` | Name query: multiple names OR'ed, `-l` lexer and AND/OR/NOT parser |
| `src/matcher.rs` | Substring and regular-expression matching |
| `src/sort.rs` | Sort keys and directions |
| `src/mounts.rs` | Parses `/proc/self/mounts`, finds a mount by longest prefix |
| `src/error.rs` | Error type carrying context |
| `probe/` | The original C feasibility prototype and write-up; not part of the build |

## Dependencies

The disk side is deliberately dependency-free: talking to the block device
ourselves is the whole point. Two things are hand-rolled rather than pulled in:
locale detection (the rules are a handful of lines, see `src/i18n.rs`) and the C
prototype.

There are two direct dependencies. [`regex`](https://docs.rs/regex) is
unavoidable: hand-rolling a regex engine would be a project in itself, and it is
pure Rust with a `bytes` API that handles non-UTF-8 names, pulling in four
transitive crates (`regex-automata`, `regex-syntax`, `aho-corasick`, `memchr`).
[`libc`](https://docs.rs/libc) is only used by the listing columns: `localtime_r`
for local timestamps and `getpwuid_r`/`getgrgid_r` for user and group names.

## Language and documentation conventions

* The interface speaks Simplified Chinese (`zh-Hans`), Traditional Chinese
  (`zh-Hant`) and English (`en`), resolved from `LC_ALL`, `LANGUAGE`,
  `LC_MESSAGES`, `LANG` in that order, falling back to English. `--lang` forces
  one.
* The catalogue is one plain data struct per language, with placeholders filled by
  a `t!` macro whose field name is checked at compile time. Tests assert that no
  field is empty and that every translation uses the same placeholders as the
  English original.
* Documentation: `README.md` and `docs/*.md` are Simplified Chinese (the default);
  the English version of each is the file with an `_en` suffix in the same
  directory. Both sides need updating together.
* Code and comments are always in English.
* Only two things are still English in the interface: file paths, and error detail
  from the `regex` crate.

## Roadmap

Done: parallel reads, which took cold-cache scans from 6.5 s to 1.0 s; regex
matching, result limiting and sorting; and the three-language interface.

Not yet, roughly in the order they seem worth doing:

1. Glob matching, and matching against the full path rather than just the name.
2. Resolving extent trees above depth 0 in parallel too. Currently those index
   blocks are read on the serial path, which is fine in practice: this filesystem
   has only 344 of them across 36,000 directories.

Deliberately out of scope, because this is a toy:

* An in-memory index with incremental updates. A full warm rescan takes about
  0.14 s, so it is not needed yet.
* Decoders for other filesystems (xfs and btrfs have entirely different on-disk
  structures).
* A `getdents` fallback for filesystems with no block device (tmpfs, FUSE, ...).
