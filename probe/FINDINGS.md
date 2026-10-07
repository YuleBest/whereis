# whereis 技术可行性验证报告

日期：2026-10-08 · 验证机：Debian 13 (trixie), kernel 6.12, x86_64

## 结论

**方案可行，且实测比 `find` 快一个数量级（热缓存）。** 原型 `probe/ext4scan.c` 直接 `pread`
块设备解析 ext4 磁盘结构，在 50 万条目的根文件系统上与 `find / -xdev` 做全量集合对比：
**遗漏 0 条**。

## 验证对象

| 项目 | 值 |
|---|---|
| 设备 / 文件系统 | `/dev/sda2` ext4（单分区，无 sda1） |
| 容量 / 使用率 | 111.8 GiB / 27% |
| 块大小 / inode 大小 | 4096 / 256 |
| inode 总数 / 已用 | 7,331,840 / 455,553 |
| 目录数 / 全部条目 | 36,123 / ~500,700 |
| 关键 features | `extent` `dir_index` `filetype` `64bit` `flex_bg` `uninit_bg` `has_journal` |
| **不存在**的 features | `inline_data` `metadata_csum` `bigalloc` `encrypt` `casefold` |

没有 `inline_data` / `metadata_csum` / `bigalloc` 这三种最麻烦的特性，解析器实现难度显著降低。
但实现仍需覆盖 `extent`（现代 ext4 默认）、`64bit`（64 字节 group descriptor）、
`dir_index`（htree 索引块）、`filetype`（目录项 `name_len` 是 u8 且带 type 字段）。

## 方案

不遍历目录树、不调用任何路径类系统调用（无 `opendir`/`readdir`/`stat`/`openat`）：

1. 读 superblock（偏移 1024）→ 块大小、inode 大小、每块组 inode 数、descriptor 大小。
2. 读 group descriptor 表（位于 superblock 所在块的下一个块）→ 每个块组的 inode table 起始块。
3. 从根 inode(2) 做 BFS。对每个目录 inode 解析其数据块（extent 树，或退化情况的间接块），
   再逐块解析块内的 `ext4_dir_entry_2` 记录。
4. **因为 fs 有 `filetype` feature，每个目录项自带子项类型**，所以只有*目录*的 inode 需要读，
   普通文件的 inode 完全不用碰。这正是能把 I/O 压到 170 MiB 的原因。

## 性能实测

```
                              冷缓存(丢 page cache)   热缓存
find / -xdev                      9.52 s             1.26 s
ext4scan (裸块 BFS)               6.28 s             0.094 s
ext4scan --full (全量 inode 表)    3.85 s             —       # 1.75 GiB 顺序读，464 MB/s
dd 顺序读设备吞吐                                    479 MB/s
```

- 热缓存下 **0.094s vs 1.26s，快 13 倍**；只读了 170 MiB。
- 冷缓存的 6.28s 瓶颈是约 7 万次随机 I/O 的**延迟**（约 87 µs/次），不是带宽。
  用 `io_uring` 提高队列深度并行读取，冷启动还有很大优化空间。
- `--full` 全量顺序扫 inode 表虽然顺序吞吐好（贴着设备上限 464 MB/s），
  但 1.75 GiB 的绝对量更大，而且见下方"陷阱 2"——它不可靠。

## 三条经过实验证实的关键结论

### 1. 必须用缓冲读，绝对不能用 O_DIRECT

裸读 `/dev/sdaX` 与 ext4 **共享同一份 block device page cache**。ext4 修改元数据时改的就是
这些 page，所以裸读拿到的是**实时内存元数据**，不是盘上可能陈旧的状态。

实验：`touch` 一个新文件后立刻用两种方式读该目录块——

```
缓冲读 (page cache)     → 立刻看到新文件名
O_DIRECT (绕过 cache)   → 看不到
```

推论：**工具不需要先 `sync`，也不存在"索引陈旧"问题**，实时性天然成立。
反过来说，若为追求速度改用 `O_DIRECT` 会丢掉这个性质，是错误取舍。

### 2. `uninit_bg` 下 inode 表有残留旧数据，`i_mode != 0` 不能判断 inode 在用

`uninit_bg` 特性下未初始化块组的 inode 表**不清零**，里面残留旧数据。
全量扫 inode 表按 `i_mode != 0` 统计，得到 472,538 个已用 inode，
而 `dumpe2fs` 的真实值是 **455,553**，高估了 17,000 个。

**必须查 inode bitmap** 才能判断 inode 是否在用。
反过来，BFS 方案天然免疫：它只读被活跃目录项引用到的 inode。

### 3. 挂载点会遮蔽底层目录，裸扫描能看见 VFS 看不见的东西

ext4 根上的 `/run` 目录里真实存在 initramfs 阶段遗留的 `blkid/`（含 `blkid.tab`）和 `mount/`。
systemd 把 tmpfs 挂到 `/run` 后这些内容被遮蔽，`find / -xdev` 看不到，裸扫描能看到。
这正是与 `find` 对比时多出的 4 条路径。

推论：工具需要读 `/proc/mounts` 来决定——
(a) 被遮蔽的底层内容是要跳过还是保留（对搜索工具，"能找到隐藏的东西"通常是优点）；
(b) 挂载点下的**其他文件系统**必须单独枚举其块设备来扫描。

## 已知限制与后续设计要点

- **纯块层方案只能覆盖块设备型文件系统**（ext4/xfs/btrfs…）。本机 `/run` `/tmp` `/dev/shm` 是
  tmpfs、`/mnt/webdav` 是 fuse.rclone，这些没有块设备，**必须退回 `getdents` 遍历**。
  所以最终工具是混合架构：块扫描 + 目录遍历兜底。
- **其他文件系统需要各自的解析器**（xfs、btrfs 结构完全不同）。建议先只做 ext4，
  接口上把"文件系统解析器"抽象成可替换模块。
- **内存占用**：50 万条路径约 30 MB，可接受；但 1000 万级文件需要紧凑的字符串 arena / 前缀树，
  不能每条路径一个 `malloc`。
- **增量更新可以不做**：热缓存全量重扫只要 0.094s，周期性全量重扫比 fanotify/inotify 简单得多。
  只有文件规模大到元数据放不进 page cache 时，才需要考虑增量。
- 解析器应校验 `rec_len` 的合法性与对齐（原型已做），并对 htree 的 `dx_root`/`dx_node`
  索引块做好跳过（原型的做法是依赖索引块的伪目录项 `rec_len` 覆盖整块来自动跳过）。

## 原型使用

```sh
cd probe && gcc -O2 -o ext4scan ext4scan.c
sudo ./ext4scan /dev/sda2            # stdout 输出全部路径，状态行走 stderr
sudo ./ext4scan /dev/sda2 --quiet    # 只输出统计
sudo ./ext4scan /dev/sda2 --full     # 全量顺序扫 inode 表（吞吐参考，注意结论 2）
```

设备以 `O_RDONLY` 打开，程序内**没有任何写路径**。

## 踩过的两个坑（供后续参考）

- `q_push` 里最初用 `strlen(name)` 取目录项名字——**盘上的名字不是 NUL 结尾的**，
  `strlen` 会一直读到下一个 0 字节，把后续字节吃进路径。
  症状极具迷惑性：路径看起来基本正确、只是在真名后面粘了几个乱码字节（如 `cups` → `cups\x1056`），
  且乱码会沿着子目录路径逐层继承。必须用盘上的 `name_len` 精确取长度。
- 用 `sudo sh -c 'time ...'` 计时——dash 没有 `time` 关键字，命令静默失败、计数变成 0。
  要用 `sudo bash -c`。
