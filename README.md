# whereis

Instant filename search on Linux, in the spirit of Windows' *Everything*.

The package is called `whereis`; the command it installs is **`wis`**. See
[Why `wis`?](#why-wis).

`whereis` does not walk the directory tree through the kernel. It opens the
block device read-only and decodes the ext4 filesystem's own metadata
(superblock → group descriptors → inode tables → extent trees → directory
blocks), so a full index of a 500,000-entry filesystem takes about a tenth of a
second instead of a second.

## Status

Early. One filesystem type, no index cache. See [Roadmap](#roadmap) for what is
deliberately not here yet.

## Requirements

* Linux
* **ext4** on a block device
* **root** — reading a block device needs it
* Rust 1.75+ to build

## Usage

```
wis [OPTIONS] <NAME>
```

`<NAME>` is matched against file *names* — never full paths — and always on raw
bytes, so non-UTF-8 names work. By default it is a case-insensitive substring;
with `--regex` it is a regular expression.

```console
$ sudo wis sshd_config
/etc/ssh/sshd_config
/etc/ssh/sshd_config.d
/usr/share/man/man5/sshd_config.5.gz
/usr/share/openssh/sshd_config
/usr/share/openssh/sshd_config.md5sum
/var/lib/ucf/cache/:etc:ssh:sshd_config
wis: 6 match(es) in 0.128s -- 501106 entries in 36151 dirs, 170.5 MiB read
```

Results go to stdout, one path per line. The summary goes to stderr, so piping
stdout is clean.

| Option | Meaning |
|---|---|
| `-r`, `--regex` | Treat `<NAME>` as a regular expression |
| `-n`, `--limit <N>` | Print at most N results, applied after sorting |
| `-s`, `--sort <KEY> [DIR]` | Sort by KEY in direction DIR (`asc`, the default, or `desc`) |
| `-d`, `--device <PATH>` | Scan this block device or filesystem image instead of the filesystem mounted at `/` |
| `-j`, `--threads <N>` | Concurrent metadata reads to keep in flight (default 16) |
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print version |

An empty `<NAME>` lists every entry.

### Regular expressions

`-r` switches the pattern from a substring to a regular expression, using the
[Rust `regex` syntax](https://docs.rs/regex/latest/regex/#syntax). The pattern is
*searched* within the name rather than anchored to it, so `-r '\.so\.[0-9]+$'`
finds `libssl.so.3`. Anchor with `^` and `$` yourself.

`-r` changes the pattern language and nothing else, so matching stays
case-insensitive the way substring mode is. A pattern can opt back out with
`(?-i)`, or scope it to part of the pattern with `(?-i:...)`.

Because matching runs against raw bytes, a regex works on names that are not
valid UTF-8.

### Sorting

| KEY | Sorts by |
|---|---|
| `path` | The full path (default) |
| `name` | The file name, ignoring directories |
| `ext` | The file extension; entries with no extension sort first |
| `size` | File size in bytes |
| `mtime` | Modification time |

`DIR` is `asc` (the default) or `desc`. Ties are always broken by ascending path,
so the output is deterministic.

The direction is optional and is only consumed when it really is a direction, so
both of these work:

```console
$ sudo wis -s size libssl        # ascending; "libssl" is the search term
$ sudo wis -s size desc libssl
```

`size` and `mtime` are not free. The walk only reads *directory* inodes, so a
match's own metadata is unknown until it is asked for, and those keys cost one
extra inode read per match (done in parallel). Listing all 500,000 entries on this
filesystem takes 0.38 s sorted by path and 0.74 s sorted by size.

### Limiting

`-n` applies after sorting, so it gives the top N by the chosen key:

```console
$ sudo wis -s size desc -n 10 '\.log$'
```

## How it works

1. Read the ext4 superblock at byte offset 1024 for the filesystem geometry.
2. Read the group descriptor table (the block after the superblock's) to locate
   every block group's inode table.
3. Walk from the root inode (2). For each directory, decode its data blocks
   (extent tree, or the legacy indirect block map) and parse the
   `ext4_dir_entry_2` records inside them.

Because modern ext4 carries the `filetype` feature, each directory entry records
the type of the inode it points at, so only **directory** inodes ever need to be
read. That is what keeps the I/O down to the directory blocks — 170 MiB for a
500,000-entry filesystem.

The walk runs in batches rather than one directory at a time, because those reads
are scattered and would otherwise serialise on latency. A batch of directories has
all of its inodes read in parallel, then all of its data blocks read in parallel;
parsing them afterwards is pure CPU work and stays single-threaded. A directory's
data blocks cannot be known before its inode has been read, so those two steps
cannot be merged — but within each step the reads are independent.

### Two things that are easy to get wrong

**Directory entry names are not NUL-terminated.** They must be sliced using the
on-disk `name_len`. Reaching for a string-length function reads straight through
into the next directory entry; the symptom is a path that looks almost right but
has a few stray bytes glued onto the end, inherited by every child path.

**Reads must stay buffered — never `O_DIRECT`.** A buffered read of `/dev/sdaX`
lands in the same page cache pages the mounted ext4 uses for its metadata, so the
scan sees live in-memory state. `O_DIRECT` bypasses that and returns
not-yet-checkpointed on-disk state: a file created a moment ago disappears.
This is why the tool needs no `sync` beforehand and never shows a stale index.

More on the feasibility work, including measurements, is in
[`probe/FINDINGS.md`](probe/FINDINGS.md).

## Performance

Root filesystem: 112 GiB ext4, ~500,000 entries, cheap SATA SSD (479 MB/s
sequential, ~87 µs random-read latency).

Time to produce results, best of several runs. These are for a search matching a
handful of entries, so they are essentially the scan time; sorting a full
500,000-entry listing costs roughly another 0.24 s.

| threads | warm cache | cold cache |
|---|---|---|
| 1 | 0.142 s | 3.83 s |
| 4 | 0.109 s | 1.58 s |
| **16** (default) | 0.140 s | 1.04 s |
| 32 | 0.172 s | 0.99 s |

Whole command, including process start, sorting and writing the output:

| | warm cache | cold cache |
|---|---|---|
| `wis ""` (full enumeration) | 0.39 s | 1.34 s |
| `find / -xdev` | 1.16 s | 9.59 s |

The metadata reads are scattered, so on a cold page cache the walk is bound by
per-read latency rather than by bandwidth: ~70,000 reads serialise into ~4
seconds, while the device can move the same 170 MiB in under a second given
enough requests in flight. That is what the batching and the thread pool buy.

Past 16 threads the cold curve flattens (the device queue saturates) and the warm
curve turns upward (waking 32 threads costs more than the reads it parallelises).
Warm cache at 4 threads is in fact marginally faster than at 16, but 16 is the
better default: it is half a second faster cold, and the warm difference is 30 ms,
below the threshold where anyone notices.

Even cold, a full scan is now cheap enough that a periodic rescan beats
maintaining an incremental index.

## Known limitations

* **Unsupported features are refused, not guessed at.** `meta_bg`, `inline_data`
  and `bigalloc` change the on-disk layout in ways this reader does not decode,
  and the `filetype` feature is required. On such a filesystem it exits with a
  clear message rather than returning plausible-looking wrong paths.
* **Only ext4 on a block device.** tmpfs, overlayfs, btrfs, XFS, NFS and FUSE
  mounts have no block device to read, or a completely different on-disk format.
  Run against a non-ext4 root, the tool says so and exits.
* **Only the root filesystem** is searched. Other ext4 mounts are reported on
  stderr but skipped.
* **Mount points are transparent to the scan.** The scan reads the underlying
  filesystem, so content hidden beneath a mount point is visible to it. On this
  machine the ext4 root's `/run` really does contain initramfs leftovers
  (`blkid/`, `mount/`) that the VFS hides behind the tmpfs. This is usually a
  feature for a search tool, but it is a deliberate difference from `find`.
* **No index cache.** Every invocation rescans from scratch. At 0.1 s warm this
  is fine; it will need revisiting on much larger filesystems.
* **Patterns apply to the file name, not the full path**, and there is no glob
  matching — only substrings and regular expressions.
* **Encrypted directories would yield ciphertext names.** The `encrypt` feature
  is not checked, so in a directory with the encryption flag the names decoded
  from disk are encrypted. The tree structure stays correct; only the names are
  unreadable. Not detected or reported yet.

## Dependencies

The disk side is deliberately dependency-free: talking to the block device
ourselves is the whole point. The one exception is
[`regex`](https://docs.rs/regex), because hand-rolling a regex engine would be a
project in itself, and it is pure Rust with a `bytes` API that handles non-UTF-8
names. It pulls in four transitive crates (`regex-automata`, `regex-syntax`,
`aho-corasick`, `memchr`).

## Why `wis`?

`/usr/bin/whereis` is already taken: it is util-linux's binary-location tool,
which does something entirely different. Installing this project under that name
would silently shadow it, which is a bad trade for everyone. So the package keeps
the name `whereis` and the command it installs is `wis`.

Same arrangement as ripgrep, whose package is `ripgrep` and whose command is `rg`.

## Development

```sh
cargo build --release     # produces target/release/wis
cargo test
cargo clippy --all-targets

cargo run -- --help
sudo ./target/release/wis sshd_config
```

`probe/` holds the original C feasibility prototype and its write-up. It is kept
for reference and is not part of the build.

## Roadmap

Done: parallel reads. The batched traversal over a thread pool took cold-cache
scans from 6.5 s to 1.0 s.

Not yet, roughly in the order they seem worth doing:

1. An in-memory index with incremental updates, if filesystems get big enough to
   need it.
2. Other filesystems, behind a per-filesystem decoder.
3. A `getdents`-based fallback for filesystems with no block device.
4. Glob matching, and matching against the full path rather than just the name.
5. Resolving extent trees above depth 0 in parallel too. Currently those index
   blocks are read on the serial path, which is fine in practice: this filesystem
   has only 344 of them across 36,000 directories.

## License

MIT
