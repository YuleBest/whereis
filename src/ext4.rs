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
//! All reads are buffered `pread`s. Buffered is not an accident: a buffered read
//! of `/dev/sdaX` goes through the same page cache pages the mounted ext4 uses
//! for its metadata, so the scan sees live in-memory state rather than
//! not-yet-checkpointed on-disk state. `O_DIRECT` would lose that.

use std::cell::Cell;
use std::fs::File;
use std::os::unix::fs::{FileExt, FileTypeExt};
use std::path::Path;

use crate::error::{Error, Result};

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
const INO_SIZE_HIGH: usize = 0x6C;
const INO_BLOCK: usize = 0x28;
const INO_BLOCK_LEN: usize = 60;

// feature_incompat bits
const INCOMPAT_FILETYPE: u32 = 0x0002;
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
    pub bytes_read: u64,
}

pub struct ScanResult {
    pub paths: Vec<Vec<u8>>,
    pub stats: Stats,
}

// ------------------------------------------------------------------ scanner

pub struct Scanner {
    file: File,
    geo: Geometry,
    bytes_read: Cell<u64>,
    inode_reads: Cell<u64>,
}

impl Scanner {
    pub fn open(device: &Path) -> Result<Self> {
        let meta = std::fs::metadata(device)
            .map_err(|e| Error::io(format!("stat {}", device.display()), e))?;
        let ft = meta.file_type();
        if !ft.is_block_device() && !ft.is_file() {
            return Err(Error::unsupported(format!(
                "{} is neither a block device nor a regular file",
                device.display()
            )));
        }

        let file = File::open(device)
            .map_err(|e| Error::io(format!("open {}", device.display()), e))?;

        let mut sb = [0u8; SUPERBLOCK_LEN];
        read_exact_at(&file, &mut sb, SUPERBLOCK_OFFSET)
            .map_err(|e| Error::io(format!("read superblock of {}", device.display()), e))?;

        let magic = le16(&sb, SB_MAGIC);
        if magic != MAGIC {
            return Err(Error::NotExt4 { device: device.display().to_string(), magic });
        }

        let geo = Self::parse_geometry(&file, &sb)?;
        Ok(Scanner { file, geo, bytes_read: Cell::new(0), inode_reads: Cell::new(0) })
    }

    fn parse_geometry(file: &File, sb: &[u8; SUPERBLOCK_LEN]) -> Result<Geometry> {
        let log_block_size = le32(sb, SB_LOG_BLOCK_SIZE);
        if log_block_size > 6 {
            return Err(Error::corrupt(format!("implausible s_log_block_size {log_block_size}")));
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
            return Err(Error::corrupt(format!("implausible s_inode_size {inode_size}")));
        }

        let inodes_count = le32(sb, SB_INODES_COUNT);
        let blocks_count = le32(sb, SB_BLOCKS_COUNT_LO);
        let first_data_block = le32(sb, SB_FIRST_DATA_BLOCK);
        let blocks_per_group = le32(sb, SB_BLOCKS_PER_GROUP);
        let inodes_per_group = le32(sb, SB_INODES_PER_GROUP);

        if inodes_per_group == 0 || blocks_per_group == 0 || inodes_count == 0 {
            return Err(Error::corrupt("zero inodes_per_group / blocks_per_group / inode count"));
        }

        let incompat = le32(sb, SB_FEATURE_INCOMPAT);
        let ro_compat = le32(sb, SB_FEATURE_RO_COMPAT);

        if incompat & INCOMPAT_FILETYPE == 0 {
            return Err(Error::unsupported(
                "filesystem lacks the `filetype` feature, so directory entries have a \
                 different layout (enable it with `tune2fs -O filetype`)",
            ));
        }
        if incompat & INCOMPAT_INLINE_DATA != 0 {
            return Err(Error::unsupported(
                "filesystem uses `inline_data`: small directories live inside the inode",
            ));
        }
        if ro_compat & RO_COMPAT_BIGALLOC != 0 {
            return Err(Error::unsupported(
                "filesystem uses `bigalloc`: extents are counted in clusters, not blocks",
            ));
        }
        if incompat & INCOMPAT_EXTENTS == 0 {
            // Not fatal -- we fall back to legacy indirect block maps -- but worth
            // knowing, since it means the whole filesystem predates extents.
            eprintln!("wis: note: filesystem has no `extent` feature; using legacy block maps");
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
            return Err(Error::corrupt(format!("group descriptor size {desc_size} is too small")));
        }

        let groups_by_inodes =
            (inodes_count as u64 + inodes_per_group as u64 - 1) / inodes_per_group as u64;
        let groups_by_blocks = (blocks_count as u64 - first_data_block as u64
            + blocks_per_group as u64
            - 1)
            / blocks_per_group as u64;
        let group_count = groups_by_inodes.max(groups_by_blocks) as u32;
        if group_count == 0 {
            return Err(Error::corrupt("filesystem reports zero block groups"));
        }

        // The group descriptor table sits in the block right after the one
        // holding the superblock.
        let gdt_offset = (first_data_block as u64 + 1) * block_size as u64;
        let gdt_len = group_count as u64 * desc_size as u64;
        let mut gdt = vec![0u8; gdt_len as usize];
        read_exact_at(file, &mut gdt, gdt_offset)
            .map_err(|e| Error::io("read group descriptor table", e))?;

        let mut inode_table = Vec::with_capacity(group_count as usize);
        for g in 0..group_count as usize {
            let d = &gdt[g * desc_size as usize..];
            let lo = le32(d, GD_INODE_TABLE_LO) as u64;
            let hi = if desc_size >= 64 { le32(d, GD_INODE_TABLE_HI) as u64 } else { 0 };
            inode_table.push(lo | (hi << 32));
        }

        Ok(Geometry {
            block_size,
            inode_size,
            inodes_per_group,
            inodes_count,
            inode_table,
        })
    }

    /// Walk the tree from the root directory, calling `matches` on every file
    /// name and collecting the full paths of the ones it accepts.
    pub fn scan<F>(&mut self, matches: F) -> Result<ScanResult>
    where
        F: Fn(&[u8]) -> bool,
    {
        let block_size = self.geo.block_size as usize;
        let inode_size = self.geo.inode_size as usize;

        let mut dir_stack: Vec<(u32, Vec<u8>)> = vec![(ROOT_INO, b"/".to_vec())];
        let mut paths: Vec<Vec<u8>> = Vec::new();
        let mut stats = Stats::default();

        let mut dir_inode = vec![0u8; inode_size];
        let mut child_inode = vec![0u8; inode_size];
        let mut block = vec![0u8; block_size];
        let mut data_blocks: Vec<u64> = Vec::new();

        while let Some((ino, dir_path)) = dir_stack.pop() {
            self.read_inode(ino, &mut dir_inode)?;
            if le16(&dir_inode, INO_MODE) & S_IFMT != S_IFDIR {
                continue;
            }
            stats.dirs += 1;

            let size = le32(&dir_inode, INO_SIZE_LO) as u64
                | (le32(&dir_inode, INO_SIZE_HIGH) as u64) << 32;
            if size == 0 {
                continue;
            }

            data_blocks.clear();
            self.collect_data_blocks(&dir_inode, &mut data_blocks)?;

            // Directories are never sparse, so the data blocks are consumed in
            // order and `size` simply bounds how much of them is live.
            let mut remaining = size;
            for &blk in &data_blocks {
                if remaining == 0 {
                    break;
                }
                self.read_block(blk, &mut block)?;
                remaining = remaining.saturating_sub(block_size as u64);

                let mut off = 0usize;
                while off + 8 <= block_size {
                    let child = le32(&block, off);
                    let rec_len = le16(&block, off + 4) as usize;
                    let name_len = block[off + 6] as usize;
                    let file_type = block[off + 7];

                    // A rec_len that is misaligned or runs past the block means
                    // we have lost sync; stop rather than read nonsense.
                    if rec_len < 8 || rec_len % 4 != 0 || off + rec_len > block_size {
                        break;
                    }

                    // inode 0 marks a deleted entry; the padding of the last
                    // entry in a block has inode 0 too. Both are skipped.
                    if child != 0 && name_len > 0 && name_len + 8 <= rec_len {
                        let name = &block[off + 8..off + 8 + name_len];
                        if !is_dot(name) {
                            stats.entries += 1;

                            let is_dir = match file_type {
                                FT_DIR => true,
                                0 => {
                                    // The type field is missing; the only way to
                                    // know is to look at the inode itself.
                                    self.read_inode(child, &mut child_inode)?;
                                    le16(&child_inode, INO_MODE) & S_IFMT == S_IFDIR
                                }
                                _ => false,
                            };

                            if matches(name) {
                                paths.push(join_path(&dir_path, name));
                            }
                            if is_dir && child != ROOT_INO {
                                dir_stack.push((child, join_path(&dir_path, name)));
                            }
                        }
                    }
                    off += rec_len;
                }
            }
        }

        stats.inode_reads = self.inode_reads.get();
        stats.bytes_read = self.bytes_read.get();
        Ok(ScanResult { paths, stats })
    }

    // --------------------------------------------------------------- reading

    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<()> {
        let off = block * self.geo.block_size as u64;
        read_exact_at(&self.file, buf, off)
            .map_err(|e| Error::io(format!("read block {block}"), e))?;
        self.bytes_read.set(self.bytes_read.get() + buf.len() as u64);
        Ok(())
    }

    fn read_inode(&self, ino: u32, buf: &mut [u8]) -> Result<()> {
        if ino == 0 || ino > self.geo.inodes_count {
            buf.fill(0);
            return Ok(());
        }
        let group = (ino - 1) / self.geo.inodes_per_group;
        let index = (ino - 1) % self.geo.inodes_per_group;
        let table = match self.geo.inode_table.get(group as usize) {
            Some(t) => *t,
            None => {
                buf.fill(0);
                return Ok(());
            }
        };
        let off = table * self.geo.block_size as u64 + index as u64 * self.geo.inode_size as u64;
        read_exact_at(&self.file, buf, off)
            .map_err(|e| Error::io(format!("read inode {ino}"), e))?;
        self.inode_reads.set(self.inode_reads.get() + 1);
        self.bytes_read.set(self.bytes_read.get() + buf.len() as u64);
        Ok(())
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
            return Err(Error::corrupt(format!("extent tree depth {depth} exceeds {MAX_EXTENT_DEPTH}")));
        }
        if node.len() < 12 {
            return Err(Error::corrupt("truncated extent header"));
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
                self.read_block(leaf, &mut buf)?;
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
        self.read_block(block, &mut buf)?;
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
