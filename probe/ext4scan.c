/*
 * ext4scan.c - feasibility probe: enumerate every path on an ext4 filesystem
 * by reading the raw block device directly, with zero path-based syscalls
 * (no opendir/readdir/stat/openat on the mounted tree).
 *
 * Strategy (the "Everything on NTFS MFT" analogue):
 *   1. Parse the superblock + group descriptors to locate each group's inode table.
 *   2. BFS from the root inode (2). For every directory inode, decode its
 *      data blocks (extent tree, or legacy indirect blocks) and parse the
 *      ext4_dir_entry_2 records inside.
 *   3. Because the filesystem has the "filetype" feature, each dirent carries
 *      the child's type -- so only *directory* inodes ever need to be read.
 *
 * The device is opened O_RDONLY and only pread() is used. There is no write
 * path in this program.
 *
 * build: gcc -O2 -o ext4scan ext4scan.c
 * run:   sudo ./ext4scan /dev/sda2 [--full] [--quiet]
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <time.h>
#include <sys/stat.h>

typedef uint8_t  u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;

/* ---- little-endian scalar access (x86 is LE, but keep it explicit) ---- */
static inline u16 le16(const void *p) { const u8 *b = p; return (u16)b[0] | ((u16)b[1] << 8); }
static inline u32 le32(const void *p) { const u8 *b = p; return (u32)b[0] | ((u32)b[1] << 8) | ((u32)b[2] << 16) | ((u32)b[3] << 24); }

#define EXT4_SUPER_MAGIC   0xEF53
#define EXT4_EXT_MAGIC     0xF30A
#define EXT4_ROOT_INO      2
#define EXT4_FT_DIR        2

/* superblock field offsets (relative to the 1024-byte superblock) */
#define SB_INODES_COUNT     0x00
#define SB_BLOCKS_COUNT_LO  0x04
#define SB_FREE_INODES      0x10
#define SB_FIRST_DATA_BLOCK 0x14
#define SB_LOG_BLOCK_SIZE   0x18
#define SB_BLOCKS_PER_GROUP 0x20
#define SB_INODES_PER_GROUP 0x28
#define SB_MAGIC            0x38
#define SB_INODE_SIZE       0x58
#define SB_FEATURE_INCOMPAT 0x60
#define SB_DESC_SIZE        0xFE
#define SB_SIZE             1024

/* inode field offsets */
#define INO_MODE            0x00
#define INO_SIZE_LO         0x04
#define INO_SIZE_HIGH       0x6C
#define INO_LINKS           0x1A
#define INO_BLOCK           0x28   /* 15 * u32 = 60 bytes */

/* feature_incompat bits */
#define INCOMPAT_EXTENTS    (1u << 6)
#define INCOMPAT_64BIT      (1u << 7)

static int   fd;
static u32   block_size;
static u32   inode_size;
static u32   inodes_per_group;
static u32   inodes_count;
static u32   desc_size;
static u32   group_count;
static u64  *inode_table;      /* per group: first block of the inode table */
static int   verbose = 1;
static int   full_mode = 0;

static u64 paths_found = 0;
static u64 dirs_visited = 0;
static u64 bytes_read = 0;
static u64 inode_reads = 0;

static void die(const char *what)
{
    fprintf(stderr, "ext4scan: %s: %s\n", what, strerror(errno));
    exit(1);
}

static void pread_full(void *buf, size_t len, u64 off)
{
    u8 *p = buf;
    while (len) {
        ssize_t n = pread(fd, p, len, (off_t)off);
        if (n < 0) die("pread");
        if (n == 0) { memset(p, 0, len); return; }  /* past EOF: treat as hole */
        p += n; off += (u64)n; len -= (size_t)n;
    }
    bytes_read += 0; /* accounted by caller */
}

static void read_block(u64 blk, void *buf)
{
    pread_full(buf, block_size, blk * (u64)block_size);
    bytes_read += block_size;
}

/* ---------------- geometry ---------------- */

static void load_geometry(void)
{
    u8 sb[SB_SIZE];
    pread_full(sb, sizeof sb, 1024);

    if (le16(sb + SB_MAGIC) != EXT4_SUPER_MAGIC) {
        fprintf(stderr, "ext4scan: bad magic 0x%04x -- not an ext4 filesystem\n",
                le16(sb + SB_MAGIC));
        exit(1);
    }

    u32 log_bs  = le32(sb + SB_LOG_BLOCK_SIZE);
    block_size  = 1024u << log_bs;
    u32 inodes_count_sb = le32(sb + SB_INODES_COUNT);
    inodes_count = inodes_count_sb;
    u32 blocks_count = le32(sb + SB_BLOCKS_COUNT_LO);
    u32 first_data_block = le32(sb + SB_FIRST_DATA_BLOCK);
    u32 blocks_per_group = le32(sb + SB_BLOCKS_PER_GROUP);
    inodes_per_group = le32(sb + SB_INODES_PER_GROUP);
    inode_size  = le16(sb + SB_INODE_SIZE);
    if (inode_size == 0) inode_size = 128;   /* pre-rev-1 filesystems */
    u32 incompat = le32(sb + SB_FEATURE_INCOMPAT);

    if (incompat & INCOMPAT_64BIT) {
        desc_size = le16(sb + SB_DESC_SIZE);
        if (desc_size == 0) desc_size = 32;
    } else {
        desc_size = 32;
    }

    u32 g_by_inodes = (inodes_count_sb + inodes_per_group - 1) / inodes_per_group;
    u32 g_by_blocks = (blocks_count - first_data_block + blocks_per_group - 1) / blocks_per_group;
    group_count = g_by_inodes > g_by_blocks ? g_by_inodes : g_by_blocks;

    if (verbose) {
        fprintf(stderr, "[geometry] block_size=%u inode_size=%u inodes_per_group=%u "
               "desc_size=%u groups=%u incompat=%s%s%s\n",
               block_size, inode_size, inodes_per_group, desc_size, group_count,
               (incompat & INCOMPAT_EXTENTS) ? "extents " : "",
               (incompat & INCOMPAT_64BIT) ? "64bit " : "",
               (incompat & (1u << 15)) ? "inline_data " : "");
    }

    /* group descriptor table lives in the block right after the superblock's */
    u64 gdt_block = (u64)first_data_block + 1;
    u64 gdt_bytes = (u64)group_count * desc_size;
    u8 *gdt = malloc(gdt_bytes);
    if (!gdt) die("malloc gdt");
    pread_full(gdt, gdt_bytes, gdt_block * (u64)block_size);
    bytes_read += gdt_bytes;

    inode_table = malloc((size_t)group_count * sizeof(u64));
    if (!inode_table) die("malloc inode_table");
    for (u32 g = 0; g < group_count; g++) {
        const u8 *d = gdt + (u64)g * desc_size;
        u64 lo = le32(d + 0x08);           /* bg_inode_table_lo */
        u64 hi = (desc_size >= 64) ? le32(d + 0x28) : 0;  /* bg_inode_table_hi */
        inode_table[g] = lo | (hi << 32);
    }
    free(gdt);
}

static void read_inode(u32 ino, u8 *buf)
{
    if (ino == 0 || ino > inodes_count) { memset(buf, 0, inode_size); return; }
    u32 g   = (ino - 1) / inodes_per_group;
    u32 idx = (ino - 1) % inodes_per_group;
    if (g >= group_count) { memset(buf, 0, inode_size); return; }
    pread_full(buf, inode_size,
               inode_table[g] * (u64)block_size + (u64)idx * inode_size);
    inode_reads++;
    bytes_read += inode_size;
}

/* ---------------- data block discovery ---------------- */

typedef struct { u64 *v; size_t n, cap; } blkvec;

static void bv_push(blkvec *b, u64 blk)
{
    if (b->n == b->cap) {
        b->cap = b->cap ? b->cap * 2 : 16;
        b->v = realloc(b->v, b->cap * sizeof(u64));
        if (!b->v) die("realloc blkvec");
    }
    b->v[b->n++] = blk;
}

/* walk one node of an extent tree (header + entries) */
static void walk_extent_node(const u8 *node, int depth, blkvec *out)
{
    u16 entries = le16(node + 2);
    for (u16 i = 0; i < entries; i++) {
        const u8 *e = node + 12 + (u64)i * 12;
        if (depth == 0) {
            u16 ee_len = le16(e + 4);
            if (ee_len == 0) continue;
            int uninit = ee_len > 32768;
            u32 len = uninit ? (u32)ee_len - 32768 : ee_len;
            if (uninit) continue;                 /* no meaningful data */
            u64 start = le32(e + 8) | ((u64)le16(e + 6) << 32);
            for (u32 k = 0; k < len; k++) bv_push(out, start + k);
        } else {
            u64 leaf = le32(e + 4) | ((u64)le16(e + 8) << 32);
            u8 *buf = malloc(block_size);
            if (!buf) die("malloc extent node");
            read_block(leaf, buf);
            walk_extent_node(buf, depth - 1, out);
            free(buf);
        }
    }
}

/* legacy (non-extent) block mapping: 12 direct + single/double/triple indirect */
static void walk_indirect(u64 blk, int level, blkvec *out)
{
    if (blk == 0 || level < 0) return;
    u8 *buf = malloc(block_size);
    if (!buf) die("malloc indirect");
    read_block(blk, buf);
    u32 per = block_size / 4;
    for (u32 i = 0; i < per; i++) {
        u32 p = le32(buf + (u64)i * 4);
        if (!p) continue;
        if (level == 0) bv_push(out, p);
        else walk_indirect(p, level - 1, out);
    }
    free(buf);
}

static void collect_data_blocks(const u8 *inode, blkvec *out)
{
    const u8 *ib = inode + INO_BLOCK;
    if (le16(ib) == EXT4_EXT_MAGIC) {
        walk_extent_node(ib, le16(ib + 6), out);
        return;
    }
    /* legacy mapping */
    for (int i = 0; i < 12; i++) {
        u32 p = le32(ib + i * 4);
        if (p) bv_push(out, p);
    }
    walk_indirect(le32(ib + 12), 0, out);
    walk_indirect(le32(ib + 13), 1, out);
    walk_indirect(le32(ib + 14), 2, out);
}

/* ---------------- directory parsing ---------------- */

typedef struct { u32 ino; char *path; } qent;

static qent  *queue;
static size_t qn, qcap;

/* Directory-entry names are NOT NUL-terminated on disk, so the caller must
 * pass the exact on-disk name_len. Using strlen() here reads straight through
 * into the next dirent and corrupts the path. */
static void q_push(u32 ino, const char *parent, const char *name, size_t nlen)
{
    size_t plen = strlen(parent);
    int root = (plen == 1 && parent[0] == '/');
    char *p = malloc(plen + (root ? 0 : 1) + nlen + 1);
    if (!p) die("malloc path");
    memcpy(p, parent, plen);
    size_t o = plen;
    if (!root) p[o++] = '/';
    memcpy(p + o, name, nlen);
    p[o + nlen] = 0;

    if (qn == qcap) {
        qcap = qcap ? qcap * 2 : 1024;
        queue = realloc(queue, qcap * sizeof(qent));
        if (!queue) die("realloc queue");
    }
    queue[qn].ino = ino;
    queue[qn].path = p;
    qn++;
}

static void parse_directory(u32 ino, const char *path, u8 *inode_buf, u8 *block_buf)
{
    read_inode(ino, inode_buf);
    u32 mode = le16(inode_buf + INO_MODE);
    if ((mode & 0xF000) != 0x4000) return;              /* not a directory */

    u64 size = (u64)le32(inode_buf + INO_SIZE_LO) |
               ((u64)le32(inode_buf + INO_SIZE_HIGH) << 32);
    if (size == 0) return;

    blkvec blocks = {0};
    collect_data_blocks(inode_buf, &blocks);
    dirs_visited++;

    u64 remaining = size;
    for (size_t i = 0; i < blocks.n && remaining > 0; i++) {
        read_block(blocks.v[i], block_buf);
        u32 off = 0;
        while (off + 8 <= block_size) {
            u32 child  = le32(block_buf + off);
            u16 rec    = le16(block_buf + off + 4);
            u8  nlen   = block_buf[off + 6];
            u8  ftype  = block_buf[off + 7];

            if (rec < 8 || (rec & 3) || off + rec > block_size) break;  /* corrupt */
            if (child != 0 && nlen > 0 && (u32)nlen + 8 <= rec) {
                const char *name = (const char *)(block_buf + off + 8);
                if (!(nlen == 1 && name[0] == '.') &&
                    !(nlen == 2 && name[0] == '.' && name[1] == '.')) {
                    paths_found++;
                    if (verbose) {
                        /* print path/name */
                        size_t plen = strlen(path);
                        int root = (plen == 1 && path[0] == '/');
                        fwrite(path, 1, plen, stdout);
                        if (!root) fputc('/', stdout);
                        fwrite(name, 1, nlen, stdout);
                        fputc('\n', stdout);
                    }
                    if (ftype == EXT4_FT_DIR && child != EXT4_ROOT_INO)
                        q_push(child, path, name, nlen);
                }
            }
            off += rec;
        }
        remaining = remaining > block_size ? remaining - block_size : 0;
    }
    free(blocks.v);
}

/* ---------------- full inode-table scan (throughput reference) ---------------- */

static void full_scan(void)
{
    u64 used = 0, dirs = 0;
    u8 *buf = malloc(block_size);
    if (!buf) die("malloc full_scan");
    for (u32 g = 0; g < group_count; g++) {
        u32 ipg = inodes_per_group;
        u32 itable_blocks = (ipg * inode_size + block_size - 1) / block_size;
        for (u32 b = 0; b < itable_blocks; b++) {
            read_block(inode_table[g] + b, buf);
            for (u32 k = 0; k < block_size / inode_size; k++) {
                const u8 *ino = buf + (u64)k * inode_size;
                u32 mode = le16(ino + INO_MODE);
                if (mode == 0) continue;
                used++;
                if ((mode & 0xF000) == 0x4000) dirs++;
            }
        }
    }
    fprintf(stderr, "[full] used_inodes=%llu dirs=%llu bytes_read=%llu\n",
           (unsigned long long)used, (unsigned long long)dirs,
           (unsigned long long)bytes_read);
}

static double now_s(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

int main(int argc, char **argv)
{
    const char *dev = NULL;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--full")) full_mode = 1;
        else if (!strcmp(argv[i], "--quiet")) verbose = 0;
        else dev = argv[i];
    }
    if (!dev) { fprintf(stderr, "usage: %s <device> [--full] [--quiet]\n", argv[0]); return 2; }

    fd = open(dev, O_RDONLY);
    if (fd < 0) die(dev);
    struct stat st;
    if (fstat(fd, &st) < 0) die("fstat");
    if (!S_ISBLK(st.st_mode) && !S_ISREG(st.st_mode)) {
        fprintf(stderr, "ext4scan: %s is neither a block device nor a regular file\n", dev);
        return 2;
    }

    double t0 = now_s();
    load_geometry();
    double t1 = now_s();

    if (full_mode) {
        full_scan();
        double t2 = now_s();
        fprintf(stderr, "[timing] geometry=%.3fs full_scan=%.3fs total=%.3fs\n", t1 - t0, t2 - t1, t2 - t0);
        return 0;
    }

    u8 *inode_buf = malloc(inode_size);
    u8 *block_buf = malloc(block_size);
    if (!inode_buf || !block_buf) die("malloc");

    /* BFS from the root directory */
    q_push(EXT4_ROOT_INO, "/", "", 0);
    if (verbose) puts("/");

    while (qn > 0) {
        qent e = queue[--qn];        /* LIFO is fine: it's just a traversal */
        parse_directory(e.ino, e.path, inode_buf, block_buf);
        free(e.path);
    }
    double t2 = now_s();

    fprintf(stderr, "[scan] paths=%llu dirs=%llu inode_reads=%llu bytes_read=%.1f MiB\n",
           (unsigned long long)paths_found, (unsigned long long)dirs_visited,
           (unsigned long long)inode_reads, bytes_read / 1048576.0);
    fprintf(stderr, "[timing] geometry=%.3fs bfs=%.3fs total=%.3fs\n", t1 - t0, t2 - t1, t2 - t0);
    return 0;
}
