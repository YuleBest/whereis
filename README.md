# whereis

Instant filename search on Linux, in the spirit of Windows' *Everything*.

`whereis` does not walk the directory tree through the kernel. It opens the
block device read-only and decodes the ext4 filesystem's own metadata
(superblock → group descriptors → inode tables → extent trees → directory
blocks), so a full index of a 500,000-entry filesystem takes about a tenth of a
second instead of a second.

## Status

Early. One command, one filesystem type, no index cache. See
[Roadmap](#roadmap) for what is deliberately not here yet.

## Requirements

* Linux
* **ext4** on a block device
* **root** — reading a block device needs it
* Rust 1.70+ to build

## Usage

```
whereis [OPTIONS] <NAME>
```

`<NAME>` is a case-insensitive substring matched against file *names* (not full
paths). Matching is done on raw bytes, so non-UTF-8 names work.

```console
$ sudo whereis sshd_config
/etc/ssh/sshd_config
/etc/ssh/sshd_config.d
/usr/share/man/man5/sshd_config.5.gz
/usr/share/openssh/sshd_config
/usr/share/openssh/sshd_config.md5sum
/var/lib/ucf/cache/:etc:ssh:sshd_config
whereis: 6 match(es) in 0.128s -- 501106 entries in 36151 dirs, 170.5 MiB read
```

Results go to stdout, sorted, one path per line. The summary goes to stderr, so
piping stdout is clean.

| Option | Meaning |
|---|---|
| `-d`, `--device <PATH>` | Scan this block device or filesystem image instead of the filesystem mounted at `/` |
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print version |

An empty `<NAME>` lists every entry.

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

Root filesystem: 112 GiB ext4, ~500,000 entries, cheap SATA SSD.

| | warm cache | cold cache |
|---|---|---|
| `whereis ""` (full enumeration) | 0.16 s scan | 6.5 s |
| `find / -xdev` | 0.97 s | 9.7 s |

Cold cache is bounded by the latency of ~70,000 scattered reads rather than by
bandwidth, so there is a lot of headroom in parallelising them. Warm cache is
already at the point where a periodic full rescan is cheaper than maintaining an
incremental index.

## Known limitations

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
* **No regex, no globs, no path matching** — plain substring only.

## Name clash

`/usr/bin/whereis` already exists; it is util-linux's binary-location tool and
does something entirely different. Installing this project as `whereis` will
shadow it. Run it via `cargo run` or `./target/release/whereis`, or install it
under another name.

## Development

```sh
cargo build --release
cargo test
cargo clippy --all-targets
```

`probe/` holds the original C feasibility prototype and its write-up. It is kept
for reference and is not part of the build.

## Roadmap

Not yet, roughly in the order they seem worth doing:

1. Parallel reads (io_uring) to cut cold-start time.
2. An in-memory index with incremental updates, if filesystems get big enough to
   need it.
3. Other filesystems, behind a per-filesystem decoder.
4. A `getdents`-based fallback for filesystems with no block device.
5. Regex / glob matching, and matching against the full path.

## License

MIT
