# Design notes

For anyone who wants to know how it works. For getting started, see the
[README](../README_en.md).

[简体中文](design.md) · **English**

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

### Why the walk is batched

The metadata reads are scattered, so on a cold page cache a one-directory-at-a-time
walk serialises on per-read latency (~87 µs on this cheap SATA SSD): ~70,000 reads
add up to about 4 seconds, while the device can move the same 170 MiB in under a
second given enough requests in flight.

So a batch of directories has all of its inodes read in parallel, then all of its
data blocks read in parallel; parsing them afterwards is pure CPU work and stays
single-threaded. A directory's data blocks cannot be known before its inode has
been read, so those two steps cannot be merged — but within each step the reads
are independent.

The batch size matters as much as the thread count: with a warm page cache each
read is about a microsecond, so a per-batch thread wake/join cycle costs more than
the reads it parallelises. Measured warm-cache scan time: 512 directories per
batch 0.25 s, 8192 per batch 0.13 s.

### Two things that are easy to get wrong

**Directory entry names are not NUL-terminated.** They must be sliced using the
on-disk `name_len`. Reaching for a string-length function reads straight through
into the next directory entry; the symptom is a path that looks almost right but
has a few stray bytes glued onto the end, inherited by every child path.

**Reads must stay buffered — never `O_DIRECT`.** A buffered read of `/dev/sdaX`
lands in the same page cache pages the mounted ext4 uses for its metadata, so the
scan sees live in-memory state. `O_DIRECT` bypasses that and returns
not-yet-checkpointed on-disk state: a file created a moment ago disappears. This
is why the tool needs no `sync` beforehand and never shows a stale index.

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
| `wis -c ""` (full enumeration, bare paths) | 0.39 s | 1.34 s |
| `find / -xdev` | 1.16 s | 9.59 s |

Past 16 threads the cold curve flattens (the device queue saturates) and the warm
curve turns upward (waking 32 threads costs more than the reads it parallelises).
Warm cache at 4 threads is in fact marginally faster than at 16, but 16 is the
better default: it is half a second faster cold, and the warm difference is 30 ms,
below the threshold where anyone notices.

The default `mode`/`mtime`/`size` columns cost one inode read per match, so a full
enumeration lands near the size-sorted figure below; `-c` turns the columns off and
restores the numbers in the table. Sorting by `size` or `mtime` needs the same
reads, because the walk only reads *directory* inodes and a match's own metadata is
not known in advance. Those reads are parallel. Listing all 500,000 entries on this
filesystem takes 0.38 s sorted by path and 0.74 s sorted by size.

Even cold, a full scan is now cheap enough that a periodic rescan beats
maintaining an incremental index.

## The full list of limitations

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

## Further reading

The feasibility work, with measurements and the original C prototype, is in
[`probe/FINDINGS.md`](../probe/FINDINGS.md).
