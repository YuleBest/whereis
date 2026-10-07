//! Read-only decoder for the ext4 on-disk format.
//!
//! The scanner never goes through the VFS. It opens the block device read-only
//! and decodes the filesystem's own metadata: superblock, group descriptors,
//! inode tables, extent trees and directory blocks.
//!
//! Two properties of the on-disk format are load-bearing here:
//!
//! * Directory entry names are **not** NUL-terminated, so every name must be
//!   sliced with the exact on-disk `name_len`.
//! * With the `filetype` feature (universal on modern ext4) each directory entry
//!   records the type of the inode it points at, so only *directory* inodes ever
//!   need to be read. That is what keeps the I/O down to the directory blocks.
//!
//! # Why the traversal is batched
//!
//! The metadata reads are scattered across the device, so on a cold page cache a
//! one-directory-at-a-time walk is bound by per-read latency (~87 us on a cheap
//! SATA SSD) rather than by bandwidth: ~70,000 reads serialise into ~6 seconds,
//! while the device can move the same 170 MiB in well under a second given
//! enough requests in flight.
//!
//! So the walk runs in batches. A batch of directories has all of its inodes read
//! in parallel, then all of its directory data blocks read in parallel, and only
//! then parsed -- parsing is pure CPU work and needs no parallelism. The
//! dependency structure is respected because a directory's data blocks cannot be
//! known before its inode has been read, but within each of those two steps the
//! reads are independent.
//!
//! All reads are buffered `pread`s. Buffered is not an accident: a buffered read
//! of `/dev/sdaX` goes through the same page cache pages the mounted ext4 uses
//! for its metadata, so the scan sees live in-memory state rather than
//! not-yet-checkpointed on-disk state. `O_DIRECT` would lose that.

use std::fs::File;
use std::os::unix::fs::{FileExt, FileTypeExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::error::{Error, Result};
use crate::i18n;

// ---------------------------------------------------------------- constants

const SUPERBLOCK_OFFSET: u64 = 1024;
const SUPERBLOCK_LEN: usize = 1024;
const MAGIC: u16 = 0xEF53;
const EXTENT_MAGIC: u16 = 0xF30A;
const ROOT_INO: u32 = 2;

const S_IFMT: u16 = 0xF000;
const S_IFDIR: u16 = 0x4000;
const FT_DIR: u8 = 2;

/// ext4 nests at most 5 levels of extent index blocks.
const MAX_EXTENT_DEPTH: usize = 5;

/// Directories taken from the frontier per batch. Batching amortises the cost of
/// waking the worker threads: with a warm page cache each read is about a
/// microsecond, so a wake/join cycle per small batch costs more than the reads it
/// parallelises. Measured warm-cache scan at 16 threads: 512 dirs per batch
/// 0.25 s, 8192 per batch 0.13 s.
const BATCH_DIRS: usize = 8192;

/// Cap on the data blocks read in one parallel step, so the block arena stays
/// bounded however large a single directory is. 8192 blocks is 32 MiB.
const MAX_BLOCKS_PER_READ: usize = 8192;

/// How many pieces to split a batch's reads into, per thread. More pieces than
/// threads means one slow region cannot leave a worker idle at the end.
const PIECES_PER_THREAD: usize = 4;

// superblock field offsets, relative to the start of the 1024-byte superblock
const SB_INODES_COUNT: usize = 0x00;
const SB_BLOCKS_COUNT_LO: usize = 0x04;
const SB_FIRST_DATA_BLOCK: usize = 0x14;
const SB_LOG_BLOCK_SIZE: usize = 0x18;
const SB_BLOCKS_PER_GROUP: usize = 0x20;
const SB_INODES_PER_GROUP: usize = 0x28;
const SB_MAGIC: usize = 0x38;
const SB_INODE_SIZE: usize = 0x58;
const SB_FEATURE_INCOMPAT: usize = 0x60;
const SB_FEATURE_RO_COMPAT: usize = 0x64;
const SB_DESC_SIZE: usize = 0xFE;

// inode field offsets
const INO_MODE: usize = 0x00;
const INO_SIZE_LO: usize = 0x04;
const INO_MTIME: usize = 0x10;
const INO_SIZE_HIGH: usize = 0x6C;
const INO_BLOCK: usize = 0x28;
const INO_BLOCK_LEN: usize = 60;

// feature_incompat bits
const INCOMPAT_FILETYPE: u32 = 0x0002;
const INCOMPAT_META_BG: u32 = 0x0010;
const INCOMPAT_EXTENTS: u32 = 0x0040;
const INCOMPAT_64BIT: u32 = 0x0080;
const INCOMPAT_INLINE_DATA: u32 = 0x8000;

// feature_ro_compat bits
const RO_COMPAT_BIGALLOC: u32 = 0x0200;

// group descriptor field offsets
const GD_INODE_TABLE_LO: usize = 0x08;
const GD_INODE_TABLE_HI: usize = 0x28;

fn le16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn le32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

// ------------------------------------------------------------------ geometry

#[derive(Debug)]
pub struct Geometry {
    pub block_size: u32,
    pub inode_size: u32,
    pub inodes_per_group: u32,
    pub inodes_count: u32,
    /// First block of the inode table, per block group.
    inode_table: Vec<u64>,
}

// -------------------------------------------------------------------- stats

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Directory entries seen (excluding `.` and `..`).
    pub entries: u64,
    pub dirs: u64,
    pub inode_reads: u64,
    /// Extent tree index blocks read while resolving directory data blocks.
    pub extent_node_reads: u64,
    pub bytes_read: u64,
}

pub struct ScanResult {
    pub hits: Vec<Hit>,
    pub stats: Stats,
}

/// One matched entry.
///
/// `size` and `mtime` stay zero during the scan: the walk only reads *directory*
/// inodes, so a matched entry's own metadata is not known until it is asked for
/// with [`Scanner::fill_metadata`].
pub struct Hit {
    pub path: Vec<u8>,
    pub inode: u32,
    pub size: u64,
    pub mtime: u32,
}

/// A directory in the current batch whose data blocks are being read.
struct DirWork {
    batch_index: usize,
    /// First slot in the block arena holding this directory's data.
    first_slot: usize,
    slots: usize,
    size: u64,
}

/// What the parse phase accumulates.
struct ParseOut<'a> {
    hits: &'a mut Vec<Hit>,
    frontier: &'a mut Vec<(u32, Vec<u8>)>,
    stats: &'a mut Stats,
    child_inode: &'a mut [u8],
}

// ------------------------------------------------------------------ scanner

pub struct Scanner {
    file: File,
    geo: Geometry,
    threads: usize,
    bytes_read: AtomicU64,
    inode_reads: AtomicU64,
    extent_node_reads: AtomicU64,
}

impl Scanner {
    pub fn open(device: &Path, threads: usize) -> Result<Self> {
        let meta = std::fs::metadata(device)
            .map_err(|e| Error::io(i18n::t!(io_stat, path = device.display()), e))?;
        let ft = meta.file_type();
        if !ft.is_block_device() && !ft.is_file() {
            return Err(Error::unsupported(i18n::t!(err_not_a_device, path = device.display())));
        }

        let file = File::open(device)
            .map_err(|e| Error::io(i18n::t!(io_open, path = device.display()), e))?;

        let mut sb = [0u8; SUPERBLOCK_LEN];
        read_exact_at(&file, &mut sb, SUPERBLOCK_OFFSET)
            .map_err(|e| Error::io(i18n::t!(io_read_superblock, path = device.display()), e))?;

        let magic = le16(&sb, SB_MAGIC);
        if magic != MAGIC {
            return Err(Error::NotExt4 { device: device.display().to_string(), magic });
        }

        let geo = Self::parse_geometry(&file, &sb)?;
        Ok(Scanner {
            file,
            geo,
            threads: threads.max(1),
            bytes_read: AtomicU64::new(0),
            inode_reads: AtomicU64::new(0),
            extent_node_reads: AtomicU64::new(0),
        })
    }

    fn parse_geometry(file: &File, sb: &[u8; SUPERBLOCK_LEN]) -> Result<Geometry> {
        let log_block_size = le32(sb, SB_LOG_BLOCK_SIZE);
        if log_block_size > 6 {
            return Err(Error::corrupt(i18n::t!(corrupt_log_block_size, value = log_block_size)));
        }
        let block_size = 1024u32 << log_block_size;

        let inode_size = {
            let raw = le16(sb, SB_INODE_SIZE);
            if raw == 0 {
                128 // pre-revision-1 filesystems
            } else {
                raw as u32
            }
        };
        if !(128..=block_size).contains(&inode_size) || !inode_size.is_power_of_two() {
            return Err(Error::corrupt(i18n::t!(corrupt_inode_size, value = inode_size)));
        }

        let inodes_count = le32(sb, SB_INODES_COUNT);
        let blocks_count = le32(sb, SB_BLOCKS_COUNT_LO);
        let first_data_block = le32(sb, SB_FIRST_DATA_BLOCK);
        let blocks_per_group = le32(sb, SB_BLOCKS_PER_GROUP);
        let inodes_per_group = le32(sb, SB_INODES_PER_GROUP);

        if inodes_per_group == 0 || blocks_per_group == 0 || inodes_count == 0 {
            return Err(Error::corrupt(i18n::t!(corrupt_zero_counts)));
        }

        let incompat = le32(sb, SB_FEATURE_INCOMPAT);
        let ro_compat = le32(sb, SB_FEATURE_RO_COMPAT);

        if incompat & INCOMPAT_FILETYPE == 0 {
            return Err(Error::unsupported(
                i18n::t!(unsupported_filetype),
            ));
        }
        if incompat & INCOMPAT_META_BG != 0 {
            // With meta_bg the group descriptors are not one contiguous table
            // after the superblock, so reading them as one would silently yield
            // the wrong inode table locations -- and plausible-looking garbage
            // instead of an error. Refuse rather than guess.
            return Err(Error::unsupported(
                i18n::t!(unsupported_meta_bg),
            ));
        }
        if incompat & INCOMPAT_INLINE_DATA != 0 {
            return Err(Error::unsupported(
                i18n::t!(unsupported_inline_data),
            ));
        }
        if ro_compat & RO_COMPAT_BIGALLOC != 0 {
            return Err(Error::unsupported(
                i18n::t!(unsupported_bigalloc),
            ));
        }
        if incompat & INCOMPAT_EXTENTS == 0 {
            // Not fatal -- we fall back to legacy indirect block maps -- but worth
            // knowing, since it means the whole filesystem predates extents.
            eprintln!("{}", i18n::t!(note_no_extent));
        }

        let desc_size = if incompat & INCOMPAT_64BIT != 0 {
            let raw = le16(sb, SB_DESC_SIZE) as u32;
            if raw == 0 {
                32
            } else {
                raw
            }
        } else {
            32
        };
        if desc_size < 32 {
            return Err(Error::corrupt(i18n::t!(corrupt_desc_size, value = desc_size)));
        }

        let groups_by_inodes = (inodes_count as u64).div_ceil(inodes_per_group as u64);
        let groups_by_blocks = (blocks_count as u64 - first_data_block as u64)
            .div_ceil(blocks_per_group as u64);
        let group_count = groups_by_inodes.max(groups_by_blocks) as u32;
        if group_count == 0 {
            return Err(Error::corrupt(i18n::t!(corrupt_zero_groups)));
        }

        // The group descriptor table sits in the block right after the one
        // holding the superblock.
        let gdt_offset = (first_data_block as u64 + 1) * block_size as u64;
        let gdt_len = group_count as u64 * desc_size as u64;
        let mut gdt = vec![0u8; gdt_len as usize];
        read_exact_at(file, &mut gdt, gdt_offset)
            .map_err(|e| Error::io(i18n::t!(io_read_gdt), e))?;

        let mut inode_table = Vec::with_capacity(group_count as usize);
        for g in 0..group_count as usize {
            let d = &gdt[g * desc_size as usize..];
            let lo = le32(d, GD_INODE_TABLE_LO) as u64;
            let hi = if desc_size >= 64 { le32(d, GD_INODE_TABLE_HI) as u64 } else { 0 };
            inode_table.push(lo | (hi << 32));
        }

        Ok(Geometry { block_size, inode_size, inodes_per_group, inodes_count, inode_table })
    }

    /// Walk the tree from the root directory, calling `matches` on every file
    /// name and collecting the full paths of the ones it accepts.
    pub fn scan<F>(&self, matches: F) -> Result<ScanResult>
    where
        F: Fn(&[u8]) -> bool,
    {
        let block_size = self.geo.block_size as usize;
        let inode_size = self.geo.inode_size as usize;

        // Directories still to visit. Each carries its own full path.
        let mut frontier: Vec<(u32, Vec<u8>)> = vec![(ROOT_INO, b"/".to_vec())];
        let mut hits: Vec<Hit> = Vec::new();
        let mut stats = Stats::default();

        // Batch scratch, reused so the steady state allocates nothing.
        let mut inode_arena: Vec<u8> = Vec::new();
        let mut block_arena: Vec<u8> = Vec::new();
        let mut offsets: Vec<u64> = Vec::new();
        let mut block_offsets: Vec<u64> = Vec::new();
        let mut work: Vec<DirWork> = Vec::new();
        let mut child_inode = vec![0u8; inode_size];

        while !frontier.is_empty() {
            let take = frontier.len().min(BATCH_DIRS);
            let mut batch = frontier.split_off(frontier.len() - take);
            batch.retain(|(ino, _)| *ino != 0 && *ino <= self.geo.inodes_count);
            if batch.is_empty() {
                continue;
            }

            // -- step 1: every inode in the batch, read in parallel ----------
            // The arenas are grown but never cleared: read_many() overwrites
            // every byte it is given, so stale content can never be observed,
            // and re-zeroing ~160 MiB per scan would cost more than the reads.
            let inode_need = batch.len() * inode_size;
            if inode_arena.len() < inode_need {
                inode_arena.resize(inode_need, 0);
            }
            offsets.clear();
            offsets.extend(batch.iter().map(|(ino, _)| self.inode_offset(*ino)));
            self.read_many(&offsets, inode_size, &mut inode_arena[..inode_need])?;
            self.inode_reads.fetch_add(batch.len() as u64, Ordering::Relaxed);

            // -- step 2: work out which data blocks each directory needs -----
            work.clear();
            block_offsets.clear();
            for (i, _) in batch.iter().enumerate() {
                let inode = &inode_arena[i * inode_size..(i + 1) * inode_size];
                if le16(inode, INO_MODE) & S_IFMT != S_IFDIR {
                    continue;
                }
                stats.dirs += 1;

                let size = le32(inode, INO_SIZE_LO) as u64
                    | (le32(inode, INO_SIZE_HIGH) as u64) << 32;
                if size == 0 {
                    continue;
                }

                let first_slot = block_offsets.len();
                self.collect_data_blocks(inode, &mut block_offsets)?;
                let slots = block_offsets.len() - first_slot;
                if slots == 0 {
                    continue;
                }
                work.push(DirWork { batch_index: i, first_slot, slots, size });
            }

            // Resolving extent trees above depth 0 reads index blocks; those are
            // rare enough to stay on the serial path for now.
            for off in block_offsets.iter_mut() {
                *off *= block_size as u64;
            }

            // -- steps 3 and 4: read each group's data blocks in parallel, then
            // parse them. Groups are capped so the block arena stays bounded no
            // matter how large a single directory is.
            let mut cursor = 0;
            while cursor < work.len() {
                let first_slot = work[cursor].first_slot;
                let mut end = cursor;
                let mut slots = 0usize;
                // Always take at least one directory, even one that alone exceeds
                // the cap, so the loop is guaranteed to make progress.
                while end < work.len()
                    && (slots == 0 || slots + work[end].slots <= MAX_BLOCKS_PER_READ)
                {
                    slots += work[end].slots;
                    end += 1;
                }

                let need = slots * block_size;
                if block_arena.len() < need {
                    block_arena.resize(need, 0);
                }
                let group = &block_offsets[first_slot..first_slot + slots];
                self.read_many(group, block_size, &mut block_arena[..need])?;

                // Parsing is pure CPU work, so it stays single-threaded.
                for w in &work[cursor..end] {
                    let base = (w.first_slot - first_slot) * block_size;
                    let data = &block_arena[base..base + w.slots * block_size];
                    let mut out = ParseOut {
                        hits: &mut hits,
                        frontier: &mut frontier,
                        stats: &mut stats,
                        child_inode: &mut child_inode,
                    };
                    self.parse_directory(&batch[w.batch_index].1, w.size, data, &matches, &mut out)?;
                }
                cursor = end;
            }
        }

        stats.inode_reads = self.inode_reads.load(Ordering::Relaxed);
        stats.extent_node_reads = self.extent_node_reads.load(Ordering::Relaxed);
        stats.bytes_read = self.bytes_read.load(Ordering::Relaxed);
        Ok(ScanResult { hits, stats })
    }

    /// Parse one directory's data blocks, recording matches and queueing child
    /// directories. Entries never straddle a block boundary, so the blocks can be
    /// walked independently; `size` only bounds how much of the data is live.
    fn parse_directory<F>(
        &self,
        dir_path: &[u8],
        size: u64,
        data: &[u8],
        matches: &F,
        out: &mut ParseOut<'_>,
    ) -> Result<()>
    where
        F: Fn(&[u8]) -> bool,
    {
        let block_size = self.geo.block_size as usize;
        let mut remaining = size;

        for block in data.chunks_exact(block_size) {
            if remaining == 0 {
                break;
            }
            remaining = remaining.saturating_sub(block_size as u64);

            let mut off = 0usize;
            while off + 8 <= block_size {
                let child = le32(block, off);
                let rec_len = le16(block, off + 4) as usize;
                let name_len = block[off + 6] as usize;
                let file_type = block[off + 7];

                // A rec_len that is misaligned or runs past the block means we
                // have lost sync; stop rather than read nonsense.
                if rec_len < 8 || rec_len % 4 != 0 || off + rec_len > block_size {
                    break;
                }

                // inode 0 marks a deleted entry; the padding of the last entry in
                // a block has inode 0 too. Both are skipped.
                if child != 0 && name_len > 0 && name_len + 8 <= rec_len {
                    let name = &block[off + 8..off + 8 + name_len];
                    if !is_dot(name) {
                        out.stats.entries += 1;

                        let is_dir = match file_type {
                            FT_DIR => true,
                            0 => {
                                // The type field is missing; the only way to know
                                // is to look at the inode itself.
                                self.read_inode_serial(child, out.child_inode)?;
                                le16(out.child_inode, INO_MODE) & S_IFMT == S_IFDIR
                            }
                            _ => false,
                        };

                        if matches(name) {
                            out.hits.push(Hit {
                                path: join_path(dir_path, name),
                                inode: child,
                                size: 0,
                                mtime: 0,
                            });
                        }
                        if is_dir && child != ROOT_INO && child <= self.geo.inodes_count {
                            out.frontier.push((child, join_path(dir_path, name)));
                        }
                    }
                }
                off += rec_len;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------- parallel reads

    /// Read a `len`-byte record at each offset into `out`, which must hold
    /// exactly `offsets.len() * len` bytes. The records are independent, so they
    /// go out concurrently across the thread pool.
    fn read_many(&self, offsets: &[u64], len: usize, out: &mut [u8]) -> Result<()> {
        debug_assert_eq!(out.len(), offsets.len() * len);
        if offsets.is_empty() {
            return Ok(());
        }

        let threads = self.threads.min(offsets.len()).max(1);
        if threads == 1 {
            for (i, &off) in offsets.iter().enumerate() {
                self.read_into(off, &mut out[i * len..(i + 1) * len])?;
            }
            return Ok(());
        }

        let pieces = (threads * PIECES_PER_THREAD).min(offsets.len());
        let chunk = offsets.len().div_ceil(pieces);
        let work: Vec<(&[u64], &mut [u8])> = offsets
            .chunks(chunk)
            .zip(out.chunks_mut(chunk * len))
            .collect();
        let queue = Mutex::new(work);
        let failure: Mutex<Option<Error>> = Mutex::new(None);

        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| loop {
                    // Taking a whole piece at a time keeps lock traffic to one
                    // acquisition per piece rather than one per read.
                    let Some((piece_offsets, piece_bufs)) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    for (k, &off) in piece_offsets.iter().enumerate() {
                        let slot = &mut piece_bufs[k * len..(k + 1) * len];
                        if let Err(e) = self.read_into(off, slot) {
                            // Keep going: one unreadable block should not throw
                            // away the rest of the batch. Treat it as a hole.
                            slot.fill(0);
                            let mut first = failure.lock().unwrap();
                            if first.is_none() {
                                *first = Some(e);
                            }
                        }
                    }
                });
            }
        });

        match failure.into_inner().unwrap() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn read_into(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        read_exact_at(&self.file, buf, offset).map_err(|e| {
            Error::io(i18n::t!(io_read_bytes, len = buf.len(), offset = offset), e)
        })?;
        self.bytes_read.fetch_add(buf.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Single-record read, used where a read is discovered mid-parse.
    fn read_inode_serial(&self, ino: u32, buf: &mut [u8]) -> Result<()> {
        if ino == 0 || ino > self.geo.inodes_count {
            buf.fill(0);
            return Ok(());
        }
        let off = self.inode_offset(ino);
        read_exact_at(&self.file, buf, off)
            .map_err(|e| Error::io(i18n::t!(io_read_inode, inode = ino), e))?;
        self.inode_reads.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(buf.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Read the inodes behind `hits` and fill in their size and mtime.
    ///
    /// Only called when the requested sort key needs it, so an ordinary
    /// path-sorted search never pays for it. The reads go through the same
    /// parallel path as the scan, and entries from one directory tend to have
    /// neighbouring inodes, so many of these land in the same block.
    pub fn fill_metadata(&self, hits: &mut [Hit]) -> Result<()> {
        let inode_size = self.geo.inode_size as usize;

        let mut offsets = Vec::new();
        let mut targets = Vec::new();
        for (i, hit) in hits.iter().enumerate() {
            if let Some(offset) = self.inode_offset_checked(hit.inode) {
                offsets.push(offset);
                targets.push(i);
            }
        }
        if offsets.is_empty() {
            return Ok(());
        }

        let mut arena = vec![0u8; offsets.len() * inode_size];
        self.read_many(&offsets, inode_size, &mut arena)?;
        self.inode_reads.fetch_add(offsets.len() as u64, Ordering::Relaxed);

        for (k, &i) in targets.iter().enumerate() {
            let inode = &arena[k * inode_size..(k + 1) * inode_size];
            hits[i].size =
                le32(inode, INO_SIZE_LO) as u64 | (le32(inode, INO_SIZE_HIGH) as u64) << 32;
            hits[i].mtime = le32(inode, INO_MTIME);
        }
        Ok(())
    }

    /// Byte offset of an inode, or `None` when the inode number is out of range.
    fn inode_offset_checked(&self, ino: u32) -> Option<u64> {
        if ino == 0 || ino > self.geo.inodes_count {
            return None;
        }
        Some(self.inode_offset(ino))
    }

    /// Byte offset of an inode. The caller guarantees the inode is in range.
    fn inode_offset(&self, ino: u32) -> u64 {
        let group = (ino - 1) / self.geo.inodes_per_group;
        let index = (ino - 1) % self.geo.inodes_per_group;
        self.geo.inode_table[group as usize] * self.geo.block_size as u64
            + index as u64 * self.geo.inode_size as u64
    }

    // ------------------------------------------------------------ block maps

    /// Append the physical blocks holding an inode's data to `out`.
    fn collect_data_blocks(&self, inode: &[u8], out: &mut Vec<u64>) -> Result<()> {
        let ib = &inode[INO_BLOCK..INO_BLOCK + INO_BLOCK_LEN];

        if le16(ib, 0) == EXTENT_MAGIC {
            let depth = le16(ib, 6) as usize;
            return self.walk_extent_node(ib, depth, out);
        }

        // Legacy block map: 12 direct pointers, then single/double/triple indirect.
        for i in 0..12 {
            let p = le32(ib, i * 4) as u64;
            if p != 0 {
                out.push(p);
            }
        }
        self.walk_indirect(le32(ib, 12) as u64, 0, out)?;
        self.walk_indirect(le32(ib, 13) as u64, 1, out)?;
        self.walk_indirect(le32(ib, 14) as u64, 2, out)?;
        Ok(())
    }

    /// `node` is a block (or the inode's inline area) holding an extent header.
    fn walk_extent_node(&self, node: &[u8], depth: usize, out: &mut Vec<u64>) -> Result<()> {
        if depth > MAX_EXTENT_DEPTH {
            return Err(Error::corrupt(i18n::t!(corrupt_extent_depth, depth = depth, max = MAX_EXTENT_DEPTH)));
        }
        if node.len() < 12 {
            return Err(Error::corrupt(i18n::t!(corrupt_truncated_extent)));
        }

        // Clamp the entry count to what the node can physically hold, so a
        // corrupt header cannot walk us off the end of the buffer.
        let declared = le16(node, 2) as usize;
        let entries = declared.min((node.len() - 12) / 12);

        for i in 0..entries {
            let e = 12 + i * 12;
            if depth == 0 {
                let ee_len = le16(node, e + 4);
                if ee_len == 0 {
                    continue;
                }
                // A length above 32768 marks an uninitialised extent: the range
                // is allocated but holds no data, so there is nothing to read.
                if ee_len > 32768 {
                    continue;
                }
                let start = le32(node, e + 8) as u64 | (le16(node, e + 6) as u64) << 32;
                for k in 0..ee_len as u64 {
                    out.push(start + k);
                }
            } else {
                let leaf = le32(node, e + 4) as u64 | (le16(node, e + 8) as u64) << 32;
                let mut buf = vec![0u8; self.geo.block_size as usize];
                self.read_into(leaf * self.geo.block_size as u64, &mut buf)?;
                self.extent_node_reads.fetch_add(1, Ordering::Relaxed);
                self.walk_extent_node(&buf, depth - 1, out)?;
            }
        }
        Ok(())
    }

    fn walk_indirect(&self, block: u64, level: usize, out: &mut Vec<u64>) -> Result<()> {
        if block == 0 {
            return Ok(());
        }
        let mut buf = vec![0u8; self.geo.block_size as usize];
        self.read_into(block * self.geo.block_size as u64, &mut buf)?;
        let per_block = self.geo.block_size as usize / 4;
        for i in 0..per_block {
            let p = le32(&buf, i * 4) as u64;
            if p == 0 {
                continue;
            }
            if level == 0 {
                out.push(p);
            } else {
                self.walk_indirect(p, level - 1, out)?;
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ helpers

/// A buffered pread. Short reads are retried; a read starting at or past EOF
/// yields zeros, which is the right thing for a hole.
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    match file.read_exact_at(buf, offset) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            buf.fill(0);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn is_dot(name: &[u8]) -> bool {
    name == b"." || name == b".."
}

/// Directory entry names are not NUL-terminated; `name` is exactly `name_len`
/// bytes long. Root is stored as `/`, which already ends in a separator.
fn join_path(parent: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(parent.len() + 1 + name.len());
    out.extend_from_slice(parent);
    if !parent.ends_with(b"/") {
        out.push(b'/');
    }
    out.extend_from_slice(name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_below_root_does_not_double_the_slash() {
        assert_eq!(join_path(b"/", b"etc"), b"/etc");
        assert_eq!(join_path(b"/etc", b"passwd"), b"/etc/passwd");
    }

    #[test]
    fn dot_entries_are_recognised() {
        assert!(is_dot(b"."));
        assert!(is_dot(b".."));
        assert!(!is_dot(b"..."));
        assert!(!is_dot(b"a"));
    }
}
