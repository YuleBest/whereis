# 开发说明

**简体中文** · [English](development_en.md)

## 构建与测试

```sh
cargo build --release     # 产物：target/release/wis
cargo test
cargo clippy --all-targets

cargo run -- --help
sudo ./target/release/wis sshd_config
```

需要 Rust 1.75+（用到了 `std::thread::scope`、`let-else` 和 `div_ceil`）。

调试用的小技巧：`-d` 可以指向任意块设备或文件系统镜像，所以不必对根文件系统跑测试；
`-j 1` 会退回单线程，便于对比；stderr 上的统计行会给出读取的字节数、inode 数和 extent 节点数。

## 代码结构

| 文件 | 职责 |
|---|---|
| `src/main.rs` | 命令行解析、语言探测、编排流程、输出 |
| `src/ext4.rs` | ext4 磁盘结构解码器与批式遍历 |
| `src/i18n.rs` | 语言选择与三语消息目录 |
| `src/history.rs` | 历史记录：定位状态文件、读写 NUL 分隔格式、保留最近 10 条 |
| `src/listing.rs` | 输出列：`--line` 解析、列宽计算、权限位/时间/大小与用户名渲染 |
| `src/format.rs` | 输出格式：JSON/JSONL/CSV/TSV 的序列化与转义 |
| `src/logical.rs` | 名称查询：多名称 OR、`-l` 的词法与 AND/OR/NOT 递归下降解析 |
| `src/matcher.rs` | 子串与正则匹配 |
| `src/sort.rs` | 排序键与排序方向 |
| `src/mounts.rs` | 解析 `/proc/self/mounts`，按最长前缀定位挂载点 |
| `src/error.rs` | 带上下文的错误类型 |
| `probe/` | 最初的 C 可行性原型及其报告，仅作参考，不参与构建 |

## 依赖

磁盘这一侧刻意不引入依赖：自己跟块设备对话正是这个项目的意义所在。有两处是手写而非引库的：
语言环境检测（规则只有十几行，见 `src/i18n.rs`）和 C 原型。

直接依赖有两个。 [`regex`](https://docs.rs/regex) 无法回避：手写正则引擎本身就是个大工程，而它是
纯 Rust，且 `bytes` API 能处理非 UTF-8 文件名，它连带引入四个传递依赖（`regex-automata`、
`regex-syntax`、`aho-corasick`、`memchr`）。[`libc`](https://docs.rs/libc) 只用于输出列：
本地时间要调 `localtime_r`，用户名和组名要调 `getpwuid_r`/`getgrgid_r`。

## 语言与文档约定

* 界面语言支持简体中文（`zh-Hans`）、繁体中文（`zh-Hant`）和英文（`en`），从
  `LC_ALL`、`LANGUAGE`、`LC_MESSAGES`、`LANG` 依次推断，其余回退英文。`--lang` 可强制指定。
* 消息目录是每种语言一个纯数据结构，占位符由 `t!` 宏填充，字段名在编译期检查。测试会校验所有
  字段非空，且各语言的占位符与英文版完全一致。
* 文档：`README.md` 与 `docs/*.md` 是简体中文（默认），对应的英文版是同目录下带 `_en` 后缀的
  文件。改动文档时两边都要更新。
* 代码与注释一律用英文。
* 界面上仍会出现英文的地方只有两处：文件路径，以及来自 `regex` crate 的错误细节。

## 路线图

已完成：并行读取（冷缓存扫描从 6.5 s 降到 1.0 s）；正则匹配、条数限制与排序；三种语言的界面。

尚未做，大致按值得做的顺序排列：

1. glob 匹配，以及对完整路径而非仅文件名的匹配。
2. 把深度大于 0 的 extent 树也纳入并行。目前这些索引块走的是串行路径，实际影响可以忽略：
   本机 36,000 个目录中只有 344 个这样的块。

以下是有意排除在范围之外的（毕竟这只是个玩具）：

* 带增量更新的常驻索引。热缓存下全量重扫只要约 0.14 s，还不需要缓存。
* 其他文件系统的解码器（xfs、btrfs 的盘上结构完全不同）。
* 为无块设备的文件系统（tmpfs、FUSE 等）准备的 `getdents` 兜底方案。
