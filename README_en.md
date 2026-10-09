# whereis

Instant filename search on Linux, in the spirit of Windows' *Everything*. It does
not walk the directory tree: it opens the block device read-only and decodes the
ext4 filesystem's own metadata, so a full search of a 500,000-entry filesystem
takes about 0.1 s.

[简体中文](README.md) · **English**

## Requirements

* Linux, with **ext4** on a block device as the root filesystem
* **root** — reading a block device needs it

## Install

```sh
cargo build --release     # produces target/release/wis
```

The package is called `whereis` but the command it installs is `wis`:
`/usr/bin/whereis` is already util-linux's binary-location tool, and installing
under that name would silently shadow it. Same arrangement as ripgrep, whose
package is `ripgrep` and whose command is `rg`.

## Usage

```
wis [OPTIONS] <NAME>...
```

`<NAME>` is matched against file *names*, never full paths, and always on raw
bytes, so non-UTF-8 names work. By default it is a case-insensitive substring;
with `-r` it is a regular expression. Several names are OR'ed; with `-l` every
name is parsed as a logical expression.

| Option | Meaning |
|---|---|
| `-r`, `--regex` | Treat `<NAME>` as a regular expression |
| `-l`, `--logical` | Treat every `<NAME>` as a logical expression: uppercase `AND`, `OR`, `NOT`, adjacent terms meaning AND, nested parentheses |
| `-n`, `--limit <N>` | Print at most N results, applied after sorting |
| `-s`, `--sort <KEY> [DIR]` | Sort by `path` (default), `name`, `ext`, `size` or `mtime`; direction `asc` (default) or `desc` |
| `-d`, `--device <PATH>` | Scan this block device instead of the filesystem mounted at `/` |
| `-p`, `--path <PATH>` | Only search inside this absolute directory; with `-r`, PATH is a regular expression and every matching directory is searched recursively |
| `-t`, `--type <TYPE>[,TYPE2]...` | Search only the given types, comma-separated or repeated: `file`/`f` and `directory`/`d`; no filtering when omitted |
| `-b`, `--base <NUM>` | Search only inside an earlier result set; NUM counts back from the most recent record, `1` being the last search, up to `10` |
| `--line <FIELDS>` | Columns before the path and their order, comma-separated: `mode`, `mtime`, `size[=auto\|b\|k\|m\|g\|t]`, `user`, `group`, `nlink` (default `mode,mtime,size`) |
| `-j`, `--threads <N>` | Concurrent metadata reads to keep in flight (default 16) |
| `--lang <LANG>` | Force the output language: `zh-Hans`, `zh-Hant`, `en` |
| `-c`, `--clean` | Print bare paths only: no columns, no summary (`--line` is ignored) |
| `-q`, `--quiet` | Never ask before printing more than 1000 results |
| `-h`, `--help` / `-V`, `--version` | Print help / print version |

```console
$ sudo wis sshd_config                   # substring search
$ sudo wis -r '^libssl\.so\.[0-9]+$'     # regular expression
$ sudo wis -s size desc -n 10 '\.log$'   # the 10 biggest logs
```

### Logical expressions

With `-l`, uppercase `AND`, `OR` and `NOT` are operators, with `NOT` > `AND` >
`OR` precedence; adjacent terms mean `AND`. Parentheses nest, and a
double-quoted span is one literal term: in regex mode its parentheses and spaces
are not syntax.

```console
$ sudo wis -l 'example AND ( .mp4 OR .mp3 )' 'apple NOT .txt'
```

This matches names that contain `example` and either `.mp4` or `.mp3`, or names
that contain `apple` but not `.txt`. Names and expressions are OR'ed at the top
level. To search for parentheses, quotes or an uppercase `AND` literally, quote
the term: `"AND"`.

Each line is one result, and by default three columns come before the path:
`mode` (four-digit octal permissions), `mtime` (local time, `2025-01-01 00:00:00`)
and `size` (adaptive 1024-based units). Widths are aligned across the result set,
so the path always starts in the same column:

```console
$ sudo wis sshd_config
0644 2026-10-08 01:23:45 5.2K /etc/ssh/sshd_config
```

`-c` prints bare paths. `--line` customises the columns and their order, for
example `--line user,group,nlink,size=k,mtime`; column order is display order and
result sorting is still `-s`. User and group names are resolved through the
system NSS, falling back to numbers. The summary goes to stderr only, so stdout
carries nothing but results.

Directories are shown in blue on a terminal; pipes, redirects, `NO_COLOR` and
`TERM=dumb` get no colour. The path itself is never altered or given a trailing
slash.
`file` means a regular file; symlinks, devices, FIFOs and sockets appear only
without `-t`.

When a run would print more than 1000 lines, the first 1000 go to stdout and a
question on stderr asks whether to continue; only `y` prints the rest. The
question appears only when stdin, stdout and stderr are all terminals, so pipes,
redirects and scripts are never interrupted.

## History

Every successful search writes its full result set to `wis_history`, empty
results included; failed searches are not recorded. Only the last 10 records are
kept. The file lives at `$XDG_STATE_HOME/wis_history`, or
`~/.local/state/wis_history` when that is unset; under sudo it belongs to the
invoking user. `-b 1` is the most recent record, `-b 2` the one before it.

A base search rescans the filesystem and keeps only paths that are still in the
record, so deleted files disappear; the base search itself becomes a record too.
Records are NUL-separated, so file names with newlines survive.

## Languages

Output is in Simplified Chinese, Traditional Chinese or English, resolved from the
environment in the order `LC_ALL`, `LANGUAGE`, `LC_MESSAGES`, `LANG`. Anything
else falls back to English. `--lang` forces a language regardless of the
environment.

## Known limitations

* **Only ext4 on a block device**, and only the **root filesystem**. tmpfs,
  btrfs, XFS, NFS and FUSE are not supported.
* No index cache — every invocation rescans from scratch.
* Patterns apply to the file name only; there is no glob matching.
* Mount points are transparent to the scan: content hidden beneath one is still
  found. `--path` is interpreted on the on-disk tree too and does not resolve
  symlinks.

Details in the [design notes](docs/design_en.md).

## Documentation

* [Design notes](docs/design_en.md) — how it works, measurements, the full list of
  limitations
* [Development](docs/development_en.md) — building, testing, code layout,
  dependencies, roadmap

## License

MIT
