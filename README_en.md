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
wis [OPTIONS] <NAME>
```

`<NAME>` is matched against file *names*, never full paths, and always on raw
bytes, so non-UTF-8 names work. By default it is a case-insensitive substring;
with `-r` it is a regular expression.

| Option | Meaning |
|---|---|
| `-r`, `--regex` | Treat `<NAME>` as a regular expression |
| `-n`, `--limit <N>` | Print at most N results, applied after sorting |
| `-s`, `--sort <KEY> [DIR]` | Sort by `path` (default), `name`, `ext`, `size` or `mtime`; direction `asc` (default) or `desc` |
| `-d`, `--device <PATH>` | Scan this block device instead of the filesystem mounted at `/` |
| `-j`, `--threads <N>` | Concurrent metadata reads to keep in flight (default 16) |
| `--lang <LANG>` | Force the output language: `zh-Hans`, `zh-Hant`, `en` |
| `-c`, `--clean` | Suppress the summary line on stderr |
| `-q`, `--quiet` | Never ask before printing more than 1000 results |
| `-h`, `--help` / `-V`, `--version` | Print help / print version |

```console
$ sudo wis sshd_config                   # substring search
$ sudo wis -r '^libssl\.so\.[0-9]+$'     # regular expression
$ sudo wis -s size desc -n 10 '\.log$'   # the 10 biggest logs
```

Results go to stdout, one path per line; the summary goes to stderr, so piping
stdout is clean.

When a run would print more than 1000 lines, the first 1000 go to stdout and a
question on stderr asks whether to continue; only `y` prints the rest. The
question appears only when stdin, stdout and stderr are all terminals, so pipes,
redirects and scripts are never interrupted.

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
  found.

Details in the [design notes](docs/design_en.md).

## Documentation

* [Design notes](docs/design_en.md) — how it works, measurements, the full list of
  limitations
* [Development](docs/development_en.md) — building, testing, code layout,
  dependencies, roadmap

## License

MIT
