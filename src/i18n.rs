//! Output language selection and the message catalogue.
//!
//! The language is resolved once, from the environment, and then read through a
//! process-wide cell. That is deliberate: it is process-wide configuration in
//! exactly the way the locale itself is, and threading a `Lang` through every
//! function that can produce a message would buy nothing.
//!
//! Locale detection is done here rather than with a crate. On Linux the rules are
//! short and well defined -- `LC_ALL`, then GNU's `LANGUAGE` list, then
//! `LC_MESSAGES`, then `LANG` -- and the parsing is a handful of lines, so a
//! dependency would not pay for itself.

use std::env;
use std::sync::OnceLock;

/// Languages the tool speaks. Anything else falls back to English.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhHans,
    ZhHant,
}

pub const SUPPORTED: &str = "zh-Hans, zh-Hant, en";

impl Lang {
    /// Map a locale string such as `zh_CN.UTF-8`, `zh-Hant-TW` or `en_US` to a
    /// supported language. `None` means "not one we speak".
    pub fn parse(locale: &str) -> Option<Lang> {
        // Drop the encoding (`.UTF-8`) and any modifier (`@euro`), and treat `_`
        // and `-` alike so `zh_CN` and `zh-CN` both work.
        let base = locale
            .split(['.', '@'])
            .next()
            .unwrap_or("")
            .replace('_', "-");
        let mut parts = base.split('-');
        let language = parts.next()?.to_ascii_lowercase();
        let rest: Vec<String> = parts.map(|p| p.to_ascii_lowercase()).collect();

        match language.as_str() {
            "en" => Some(Lang::En),
            "zh" => {
                // Traditional Chinese is used in Taiwan, Hong Kong and Macau, and
                // is what `Hant` asks for; everything else Chinese is Simplified.
                let traditional = rest
                    .iter()
                    .any(|p| matches!(p.as_str(), "hant" | "tw" | "hk" | "mo"));
                Some(if traditional {
                    Lang::ZhHant
                } else {
                    Lang::ZhHans
                })
            }
            _ => None,
        }
    }

    /// Resolve the language from the environment, following the usual precedence.
    pub fn detect() -> Lang {
        if let Some(value) = env_var("LC_ALL") {
            return Lang::parse(&value).unwrap_or(Lang::En);
        }
        if let Some(value) = env_var("LANGUAGE") {
            // GNU extension: a colon-separated priority list.
            return value.split(':').find_map(Lang::parse).unwrap_or(Lang::En);
        }
        if let Some(value) = env_var("LC_MESSAGES") {
            return Lang::parse(&value).unwrap_or(Lang::En);
        }
        if let Some(value) = env_var("LANG") {
            return Lang::parse(&value).unwrap_or(Lang::En);
        }
        Lang::En
    }
}

fn env_var(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

static CURRENT: OnceLock<Lang> = OnceLock::new();

pub fn set_lang(lang: Lang) {
    let _ = CURRENT.set(lang);
}

/// The language in force. English until [`set_lang`] says otherwise.
pub fn lang() -> Lang {
    CURRENT.get().copied().unwrap_or(Lang::En)
}

// ------------------------------------------------------------------ catalogue

/// Every fixed string the tool can print, in one language.
///
/// Messages with substitutions use `{name}` placeholders, filled by [`fill`] (or
/// the `t!` macro). Keeping them as plain data means a translation can be read
/// end to end without following any code.
pub struct Text {
    // usage
    pub usage_title: &'static str,
    pub usage_header: &'static str,
    pub usage_blurb: &'static str,
    pub args_header: &'static str,
    pub arg_name: &'static str,
    pub options_header: &'static str,
    pub opt_regex: &'static str,
    pub opt_limit: &'static str,
    pub opt_sort: &'static str,
    pub opt_device: &'static str,
    pub opt_threads: &'static str,
    pub opt_lang: &'static str,
    pub opt_clean: &'static str,
    pub opt_quiet: &'static str,
    pub opt_help: &'static str,
    pub opt_version: &'static str,
    pub usage_footer: &'static str,

    // runtime output
    pub hint_root: &'static str,
    pub summary: &'static str,
    pub matches_all: &'static str,
    pub matches_limited: &'static str,
    pub matches_stopped: &'static str,
    pub prompt_continue: &'static str,
    pub note_other_ext4: &'static str,

    // argument errors
    pub err_unknown_option: &'static str,
    pub err_expected_one_name: &'static str,
    pub err_missing_name: &'static str,
    pub err_option_needs_value: &'static str,
    pub err_not_a_number: &'static str,
    pub err_threads_min: &'static str,
    pub err_unknown_sort_key: &'static str,
    pub err_unknown_lang: &'static str,

    // top-level errors
    pub err_not_ext4: &'static str,
    pub err_unsupported: &'static str,
    pub err_corrupt: &'static str,
    pub err_no_block_device: &'static str,
    pub err_bad_pattern: &'static str,
    pub err_not_a_device: &'static str,

    // unsupported ext4 features
    pub unsupported_filetype: &'static str,
    pub unsupported_meta_bg: &'static str,
    pub unsupported_inline_data: &'static str,
    pub unsupported_bigalloc: &'static str,
    pub note_no_extent: &'static str,

    // malformed metadata
    pub corrupt_log_block_size: &'static str,
    pub corrupt_inode_size: &'static str,
    pub corrupt_zero_counts: &'static str,
    pub corrupt_desc_size: &'static str,
    pub corrupt_zero_groups: &'static str,
    pub corrupt_extent_depth: &'static str,
    pub corrupt_truncated_extent: &'static str,

    // I/O contexts, rendered as "{context}: {underlying error}"
    pub io_stat: &'static str,
    pub io_open: &'static str,
    pub io_read_superblock: &'static str,
    pub io_read_gdt: &'static str,
    pub io_read_inode: &'static str,
    pub io_read_bytes: &'static str,
    pub io_write_stdout: &'static str,
    pub io_flush_stdout: &'static str,
    pub io_read_mounts: &'static str,
}

pub fn text_for(lang: Lang) -> &'static Text {
    match lang {
        Lang::En => &EN,
        Lang::ZhHans => &ZH_HANS,
        Lang::ZhHant => &ZH_HANT,
    }
}

/// The catalogue for the language in force.
pub fn text() -> &'static Text {
    text_for(lang())
}

/// Substitute `{name}` placeholders. Unknown placeholders are left alone, which
/// makes a typo visible in the output rather than silently dropping text.
pub fn fill(template: &str, pairs: &[(&str, String)]) -> String {
    let mut out = template.to_owned();
    for (key, value) in pairs {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

/// Look up a message, optionally filling placeholders:
///
/// ```ignore
/// i18n::t!(hint_root)
/// i18n::t!(io_open, path = device.display())
/// ```
///
/// The field name is checked at compile time, so a missing translation cannot
/// slip through as a runtime lookup failure.
macro_rules! t {
    ($field:ident) => {
        $crate::i18n::text().$field
    };
    ($field:ident, $($key:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::text().$field,
            &[$((stringify!($key), ($value).to_string())),+],
        )
    };
}
pub(crate) use t;

/// The help text. The command synopsis and option names are the same in every
/// language, so only the prose around them is translated.
pub fn usage(lang: Lang) -> String {
    let text = text_for(lang);
    let mut out = String::new();
    out.push_str(text.usage_title);
    out.push_str("\n\n");
    out.push_str(text.usage_header);
    out.push_str("\n    wis [OPTIONS] <NAME>\n\n");
    out.push_str(text.usage_blurb);
    out.push_str("\n\n");
    out.push_str(text.args_header);
    out.push('\n');
    out.push_str(text.arg_name);
    out.push_str("\n\n");
    out.push_str(text.options_header);
    out.push('\n');
    for option in [
        text.opt_regex,
        text.opt_limit,
        text.opt_sort,
        text.opt_device,
        text.opt_threads,
        text.opt_lang,
        text.opt_clean,
        text.opt_quiet,
        text.opt_help,
        text.opt_version,
    ] {
        out.push_str(option);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(text.usage_footer);
    out.push('\n');
    out
}

// -------------------------------------------------------------------- English

static EN: Text = Text {
    usage_title: "wis - instant filename search on Linux",
    usage_header: "USAGE:",
    usage_blurb: "Searches file names on the root filesystem by reading the ext4 metadata directly\n\
                  from the block device, without walking the directory tree through the kernel.\n\
                  Reading a block device requires root.",
    args_header: "ARGS:",
    arg_name: "    <NAME>    What to look for in a file name: by default a case-insensitive\n\
               \x20             substring, with --regex a regular expression",
    options_header: "OPTIONS:",
    opt_regex: "    -r, --regex             Treat <NAME> as a regular expression, searched within\n\
                \x20                           the file name. Matching stays case-insensitive\n\
                \x20                           unless the pattern says otherwise with (?-i)",
    opt_limit: "    -n, --limit <N>         Print at most N results, applied after sorting",
    opt_sort: "    -s, --sort <KEY> [DIR]  Sort by KEY in direction DIR, which defaults to asc.\n\
               \x20                           KEY: name, path, ext, size, mtime\n\
               \x20                           DIR: asc, desc",
    opt_device: "    -d, --device <PATH>     Scan this block device (or filesystem image) instead\n\
                 \x20                           of the filesystem mounted at /",
    opt_threads: "    -j, --threads <N>       Concurrent metadata reads to keep in flight\n\
                  \x20                           (default 16; the scan is latency-bound, not CPU-bound)",
    opt_lang: "    --lang <LANG>           Force the output language: zh-Hans, zh-Hant, en\n\
               \x20                           (default: from LC_ALL, LANGUAGE, LC_MESSAGES, LANG)",
    opt_clean: "    -c, --clean             Suppress the summary line on stderr",
    opt_quiet: "    -q, --quiet             Never ask before printing more than 1000 results",
    opt_help: "    -h, --help              Print this help",
    opt_version: "    -V, --version           Print version",
    usage_footer: "`size` and `mtime` cost one extra inode read per match, because the walk itself\n\
                   only ever reads directory inodes.",

    hint_root: "hint: reading a block device needs root -- try `sudo wis ...`",
    summary: "{shown} in {secs}s -- {entries} entries in {dirs} dirs, {mib} MiB read \
              ({inodes} inodes, {nodes} extent nodes) on {threads} threads",
    matches_all: "{n} match(es)",
    matches_limited: "{shown} of {total} match(es)",
    matches_stopped: "{shown} of {total} match(es) (output stopped)",
    prompt_continue: "wis: {remaining} more result(s) after the first {shown}; continue? [y/N] ",
    note_other_ext4: "wis: note: {device} at {target} is a separate ext4 filesystem and is not \
                      searched yet",

    err_unknown_option: "unknown option `{option}`",
    err_expected_one_name: "expected exactly one NAME",
    err_missing_name: "missing NAME",
    err_option_needs_value: "{option} requires a value",
    err_not_a_number: "`{value}` is not a number",
    err_threads_min: "--threads must be at least 1",
    err_unknown_sort_key: "unknown sort key `{key}` (expected one of: {keys})",
    err_unknown_lang: "unknown language `{value}` (expected one of: {langs})",

    err_not_ext4: "{device} is not an ext4 filesystem (superblock magic 0x{magic}, expected 0xef53)",
    err_unsupported: "unsupported: {what}",
    err_corrupt: "malformed filesystem metadata: {what}",
    err_no_block_device: "cannot search {path}: it is a {fstype} filesystem, and only ext4 on a \
                          block device is supported so far",
    err_bad_pattern: "bad regular expression: {what}",
    err_not_a_device: "{path} is neither a block device nor a regular file",

    unsupported_filetype: "filesystem lacks the `filetype` feature, so directory entries have a \
                           different layout (enable it with `tune2fs -O filetype`)",
    unsupported_meta_bg: "filesystem uses `meta_bg`: the group descriptors are not stored as one \
                          contiguous table, which this reader does not handle",
    unsupported_inline_data: "filesystem uses `inline_data`: small directories live inside the inode",
    unsupported_bigalloc: "filesystem uses `bigalloc`: extents are counted in clusters, not blocks",
    note_no_extent: "wis: note: filesystem has no `extent` feature; using legacy block maps",

    corrupt_log_block_size: "implausible s_log_block_size {value}",
    corrupt_inode_size: "implausible s_inode_size {value}",
    corrupt_zero_counts: "zero inodes_per_group / blocks_per_group / inode count",
    corrupt_desc_size: "group descriptor size {value} is too small",
    corrupt_zero_groups: "filesystem reports zero block groups",
    corrupt_extent_depth: "extent tree depth {depth} exceeds {max}",
    corrupt_truncated_extent: "truncated extent header",

    io_stat: "stat {path}",
    io_open: "open {path}",
    io_read_superblock: "read superblock of {path}",
    io_read_gdt: "read group descriptor table",
    io_read_inode: "read inode {inode}",
    io_read_bytes: "read {len} bytes at offset {offset}",
    io_write_stdout: "write to stdout",
    io_flush_stdout: "flush stdout",
    io_read_mounts: "read /proc/self/mounts",
};

// ------------------------------------------------------------------- 简体中文

static ZH_HANS: Text = Text {
    usage_title: "wis - 在 Linux 上即时搜索文件名",
    usage_header: "用法:",
    usage_blurb:
        "直接读取块设备上的 ext4 元数据来搜索根文件系统上的文件名，不经过内核遍历目录树。\n\
                  读取块设备需要 root 权限。",
    args_header: "参数:",
    arg_name:
        "    <名称>    要查找的文件名内容。默认是不区分大小写的子串，加 --regex 后视为正则表达式",
    options_header: "选项:",
    opt_regex: "    -r, --regex          把 <名称> 当作正则表达式，在文件名中搜索而非整名匹配。\n\
               \x20                        匹配默认仍不区分大小写，除非用 (?-i) 明确要求",
    opt_limit: "    -n, --limit <N>      最多输出 N 条结果，在排序之后应用",
    opt_sort: "    -s, --sort <键> [方向]\n\
               \x20                        按 <键> 排序，方向可省略\n\
               \x20                        <键>：name、path、ext、size、mtime\n\
               \x20                        方向：asc（正序，默认）、desc（倒序）",
    opt_device:
        "    -d, --device <路径>  扫描指定的块设备（或文件系统镜像），而非挂载在 / 的文件系统",
    opt_threads: "    -j, --threads <N>    并发读取数（默认 16；扫描受设备延迟限制，而非 CPU）",
    opt_lang: "    --lang <语言>        强制指定输出语言：zh-Hans、zh-Hant、en\n\
               \x20                        （默认从 LC_ALL、LANGUAGE、LC_MESSAGES、LANG 推断）",
    opt_clean: "    -c, --clean          不输出 stderr 上的统计信息",
    opt_quiet: "    -q, --quiet          输出超过 1000 行时也不询问，直接全部输出",
    opt_help: "    -h, --help           显示本帮助",
    opt_version: "    -V, --version        显示版本",
    usage_footer:
        "size 和 mtime 排序时，每条命中需要额外读一次 inode，因为遍历本身只读取目录的 inode。",

    hint_root: "提示：读取块设备需要 root 权限，请用 `sudo wis ...`",
    summary: "{shown}，用时 {secs}s —— 扫描 {entries} 个条目、{dirs} 个目录，读取 {mib} MiB\
              （{inodes} 个 inode、{nodes} 个 extent 节点），{threads} 个线程",
    matches_all: "{n} 条命中",
    matches_limited: "{total} 条命中中的 {shown} 条",
    matches_stopped: "{total} 条命中中的 {shown} 条（已停止输出）",
    prompt_continue: "wis: 已输出 {shown} 条，还有 {remaining} 条；继续输出？[y/N] ",
    note_other_ext4: "wis: 提示：{device}（挂载于 {target}）是另一个 ext4 文件系统，暂不搜索",

    err_unknown_option: "未知选项 `{option}`",
    err_expected_one_name: "只接受一个 <名称> 参数",
    err_missing_name: "缺少 <名称> 参数",
    err_option_needs_value: "{option} 需要一个值",
    err_not_a_number: "`{value}` 不是有效的数字",
    err_threads_min: "--threads 至少为 1",
    err_unknown_sort_key: "未知的排序键 `{key}`（可选：{keys}）",
    err_unknown_lang: "未知的语言 `{value}`（可选：{langs}）",

    err_not_ext4: "{device} 不是 ext4 文件系统（超级块 magic 为 0x{magic}，应为 0xef53）",
    err_unsupported: "不支持：{what}",
    err_corrupt: "文件系统元数据损坏：{what}",
    err_no_block_device: "无法搜索 {path}：它是 {fstype} 文件系统，目前只支持块设备上的 ext4",
    err_bad_pattern: "正则表达式有误：{what}",
    err_not_a_device: "{path} 既不是块设备也不是普通文件",

    unsupported_filetype:
        "文件系统缺少 `filetype` 特性，目录项布局不同（可用 `tune2fs -O filetype` 启用）",
    unsupported_meta_bg:
        "文件系统使用了 `meta_bg`：块组描述符不是连续的单一表，本程序不处理这种布局",
    unsupported_inline_data: "文件系统使用了 `inline_data`：小目录存放在 inode 内部",
    unsupported_bigalloc: "文件系统使用了 `bigalloc`：extent 以簇而非块为单位",
    note_no_extent: "wis: 提示：文件系统没有 `extent` 特性，改用传统的块映射",

    corrupt_log_block_size: "s_log_block_size 不合理：{value}",
    corrupt_inode_size: "s_inode_size 不合理：{value}",
    corrupt_zero_counts: "inodes_per_group / blocks_per_group / inode 总数为零",
    corrupt_desc_size: "块组描述符大小 {value} 过小",
    corrupt_zero_groups: "文件系统报告的块组数为零",
    corrupt_extent_depth: "extent 树深度 {depth} 超过上限 {max}",
    corrupt_truncated_extent: "extent 头部被截断",

    io_stat: "无法获取 {path} 的信息",
    io_open: "无法打开 {path}",
    io_read_superblock: "无法读取 {path} 的超级块",
    io_read_gdt: "无法读取块组描述符表",
    io_read_inode: "无法读取 inode {inode}",
    io_read_bytes: "无法读取偏移 {offset} 处的 {len} 字节",
    io_write_stdout: "写入标准输出失败",
    io_flush_stdout: "刷新标准输出失败",
    io_read_mounts: "无法读取 /proc/self/mounts",
};

// ------------------------------------------------------------------- 繁體中文

static ZH_HANT: Text = Text {
    usage_title: "wis - 在 Linux 上即時搜尋檔案名稱",
    usage_header: "用法:",
    usage_blurb: "直接讀取區塊裝置上的 ext4 中介資料來搜尋根檔案系統上的檔案名稱，不透過核心走訪目錄樹。\n\
                  讀取區塊裝置需要 root 權限。",
    args_header: "參數:",
    arg_name: "    <名稱>    要尋找的檔案名稱內容。預設為不分大小寫的子字串，加上 --regex 後視為正規表達式",
    options_header: "選項:",
    opt_regex: "    -r, --regex          把 <名稱> 當作正規表達式，在檔案名稱中搜尋而非整名比對。\n\
               \x20                        比對預設仍不分大小寫，除非用 (?-i) 明確要求",
    opt_limit: "    -n, --limit <N>      最多輸出 N 筆結果，於排序之後套用",
    opt_sort: "    -s, --sort <鍵> [方向]\n\
               \x20                        依 <鍵> 排序，方向可省略\n\
               \x20                        <鍵>：name、path、ext、size、mtime\n\
               \x20                        方向：asc（正序，預設）、desc（倒序）",
    opt_device: "    -d, --device <路徑>  掃描指定的區塊裝置（或檔案系統映像），而非掛載於 / 的檔案系統",
    opt_threads: "    -j, --threads <N>    並行讀取數（預設 16；掃描受裝置延遲限制，而非 CPU）",
    opt_lang: "    --lang <語言>        強制指定輸出語言：zh-Hans、zh-Hant、en\n\
               \x20                        （預設由 LC_ALL、LANGUAGE、LC_MESSAGES、LANG 推斷）",
    opt_clean: "    -c, --clean          不輸出 stderr 上的統計資訊",
    opt_quiet: "    -q, --quiet          輸出超過 1000 行時也不詢問，直接全部輸出",
    opt_help: "    -h, --help           顯示本說明",
    opt_version: "    -V, --version        顯示版本",
    usage_footer: "以 size 和 mtime 排序時，每筆命中需額外讀取一次 inode，因為走訪本身只讀取目錄的 inode。",

    hint_root: "提示：讀取區塊裝置需要 root 權限，請用 `sudo wis ...`",
    summary: "{shown}，耗時 {secs}s —— 掃描 {entries} 個項目、{dirs} 個目錄，讀取 {mib} MiB\
              （{inodes} 個 inode、{nodes} 個 extent 節點），{threads} 個執行緒",
    matches_all: "{n} 筆命中",
    matches_limited: "{total} 筆命中中的 {shown} 筆",
    matches_stopped: "{total} 筆命中中的 {shown} 筆（已停止輸出）",
    prompt_continue: "wis: 已輸出 {shown} 筆，還有 {remaining} 筆；繼續輸出？[y/N] ",
    note_other_ext4: "wis: 提示：{device}（掛載於 {target}）是另一個 ext4 檔案系統，暫不搜尋",

    err_unknown_option: "未知的選項 `{option}`",
    err_expected_one_name: "只接受一個 <名稱> 參數",
    err_missing_name: "缺少 <名稱> 參數",
    err_option_needs_value: "{option} 需要一個值",
    err_not_a_number: "`{value}` 不是有效的數字",
    err_threads_min: "--threads 至少為 1",
    err_unknown_sort_key: "未知的排序鍵 `{key}`（可選：{keys}）",
    err_unknown_lang: "未知的語言 `{value}`（可選：{langs}）",

    err_not_ext4: "{device} 不是 ext4 檔案系統（超級區塊 magic 為 0x{magic}，應為 0xef53）",
    err_unsupported: "不支援：{what}",
    err_corrupt: "檔案系統中介資料損毀：{what}",
    err_no_block_device: "無法搜尋 {path}：它是 {fstype} 檔案系統，目前只支援區塊裝置上的 ext4",
    err_bad_pattern: "正規表達式有誤：{what}",
    err_not_a_device: "{path} 既不是區塊裝置也不是一般檔案",

    unsupported_filetype: "檔案系統缺少 `filetype` 特性，目錄項佈局不同（可用 `tune2fs -O filetype` 啟用）",
    unsupported_meta_bg: "檔案系統使用了 `meta_bg`：區塊群組描述元不是連續的單一表，本程式不處理這種佈局",
    unsupported_inline_data: "檔案系統使用了 `inline_data`：小目錄存放於 inode 內部",
    unsupported_bigalloc: "檔案系統使用了 `bigalloc`：extent 以叢集而非區塊為單位",
    note_no_extent: "wis: 提示：檔案系統沒有 `extent` 特性，改用傳統的區塊映射",

    corrupt_log_block_size: "s_log_block_size 不合理：{value}",
    corrupt_inode_size: "s_inode_size 不合理：{value}",
    corrupt_zero_counts: "inodes_per_group / blocks_per_group / inode 總數為零",
    corrupt_desc_size: "區塊群組描述元大小 {value} 過小",
    corrupt_zero_groups: "檔案系統回報的區塊群組數為零",
    corrupt_extent_depth: "extent 樹深度 {depth} 超過上限 {max}",
    corrupt_truncated_extent: "extent 標頭被截斷",

    io_stat: "無法取得 {path} 的資訊",
    io_open: "無法開啟 {path}",
    io_read_superblock: "無法讀取 {path} 的超級區塊",
    io_read_gdt: "無法讀取區塊群組描述元表",
    io_read_inode: "無法讀取 inode {inode}",
    io_read_bytes: "無法讀取位移 {offset} 處的 {len} 位元組",
    io_write_stdout: "寫入標準輸出失敗",
    io_flush_stdout: "重新整理標準輸出失敗",
    io_read_mounts: "無法讀取 /proc/self/mounts",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_locale_spellings_that_matter() {
        assert_eq!(Lang::parse("zh_CN.UTF-8"), Some(Lang::ZhHans));
        assert_eq!(Lang::parse("zh-CN"), Some(Lang::ZhHans));
        assert_eq!(Lang::parse("zh-Hans"), Some(Lang::ZhHans));
        assert_eq!(Lang::parse("zh"), Some(Lang::ZhHans));
        assert_eq!(Lang::parse("zh_SG"), Some(Lang::ZhHans));

        assert_eq!(Lang::parse("zh_TW.UTF-8"), Some(Lang::ZhHant));
        assert_eq!(Lang::parse("zh-Hant"), Some(Lang::ZhHant));
        assert_eq!(Lang::parse("zh_HK"), Some(Lang::ZhHant));
        assert_eq!(Lang::parse("zh-MO"), Some(Lang::ZhHant));

        assert_eq!(Lang::parse("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::parse("en"), Some(Lang::En));
        assert_eq!(Lang::parse("C"), None);
        assert_eq!(Lang::parse("POSIX"), None);
        assert_eq!(Lang::parse("de_DE"), None);
    }

    #[test]
    fn every_language_fills_every_field() {
        for lang in [Lang::En, Lang::ZhHans, Lang::ZhHant] {
            let text = text_for(lang);
            let all = [
                text.usage_title,
                text.usage_header,
                text.usage_blurb,
                text.args_header,
                text.arg_name,
                text.options_header,
                text.opt_regex,
                text.opt_limit,
                text.opt_sort,
                text.opt_device,
                text.opt_threads,
                text.opt_lang,
                text.opt_clean,
                text.opt_quiet,
                text.opt_help,
                text.opt_version,
                text.usage_footer,
                text.hint_root,
                text.summary,
                text.matches_all,
                text.matches_limited,
                text.matches_stopped,
                text.prompt_continue,
                text.note_other_ext4,
                text.err_unknown_option,
                text.err_expected_one_name,
                text.err_missing_name,
                text.err_option_needs_value,
                text.err_not_a_number,
                text.err_threads_min,
                text.err_unknown_sort_key,
                text.err_unknown_lang,
                text.err_not_ext4,
                text.err_unsupported,
                text.err_corrupt,
                text.err_no_block_device,
                text.err_bad_pattern,
                text.err_not_a_device,
                text.unsupported_filetype,
                text.unsupported_meta_bg,
                text.unsupported_inline_data,
                text.unsupported_bigalloc,
                text.note_no_extent,
                text.corrupt_log_block_size,
                text.corrupt_inode_size,
                text.corrupt_zero_counts,
                text.corrupt_desc_size,
                text.corrupt_zero_groups,
                text.corrupt_extent_depth,
                text.corrupt_truncated_extent,
                text.io_stat,
                text.io_open,
                text.io_read_superblock,
                text.io_read_gdt,
                text.io_read_inode,
                text.io_read_bytes,
                text.io_write_stdout,
                text.io_flush_stdout,
                text.io_read_mounts,
            ];
            for (i, message) in all.iter().enumerate() {
                assert!(!message.is_empty(), "{lang:?} field {i} is empty");
            }
        }
    }

    /// A translated message must use the same placeholders as the English one, or
    /// `fill` would leave a literal `{...}` in the output.
    #[test]
    fn placeholders_match_english() {
        fn placeholders(s: &str) -> Vec<String> {
            let mut found = Vec::new();
            let bytes = s.as_bytes();
            let mut i = 0;
            while let Some(open) = s[i..].find('{') {
                let start = i + open + 1;
                match s[start..].find('}') {
                    Some(close) => {
                        found.push(s[start..start + close].to_owned());
                        i = start + close + 1;
                    }
                    None => break,
                }
            }
            let _ = bytes;
            found.sort();
            found
        }

        let pairs: [(&str, &str, &str); 19] = [
            (EN.summary, ZH_HANS.summary, ZH_HANT.summary),
            (EN.matches_all, ZH_HANS.matches_all, ZH_HANT.matches_all),
            (
                EN.matches_limited,
                ZH_HANS.matches_limited,
                ZH_HANT.matches_limited,
            ),
            (
                EN.matches_stopped,
                ZH_HANS.matches_stopped,
                ZH_HANT.matches_stopped,
            ),
            (
                EN.prompt_continue,
                ZH_HANS.prompt_continue,
                ZH_HANT.prompt_continue,
            ),
            (
                EN.note_other_ext4,
                ZH_HANS.note_other_ext4,
                ZH_HANT.note_other_ext4,
            ),
            (
                EN.err_unknown_option,
                ZH_HANS.err_unknown_option,
                ZH_HANT.err_unknown_option,
            ),
            (
                EN.err_option_needs_value,
                ZH_HANS.err_option_needs_value,
                ZH_HANT.err_option_needs_value,
            ),
            (
                EN.err_not_a_number,
                ZH_HANS.err_not_a_number,
                ZH_HANT.err_not_a_number,
            ),
            (
                EN.err_unknown_sort_key,
                ZH_HANS.err_unknown_sort_key,
                ZH_HANT.err_unknown_sort_key,
            ),
            (
                EN.err_unknown_lang,
                ZH_HANS.err_unknown_lang,
                ZH_HANT.err_unknown_lang,
            ),
            (EN.err_not_ext4, ZH_HANS.err_not_ext4, ZH_HANT.err_not_ext4),
            (
                EN.err_unsupported,
                ZH_HANS.err_unsupported,
                ZH_HANT.err_unsupported,
            ),
            (EN.err_corrupt, ZH_HANS.err_corrupt, ZH_HANT.err_corrupt),
            (
                EN.err_no_block_device,
                ZH_HANS.err_no_block_device,
                ZH_HANT.err_no_block_device,
            ),
            (
                EN.err_bad_pattern,
                ZH_HANS.err_bad_pattern,
                ZH_HANT.err_bad_pattern,
            ),
            (
                EN.err_not_a_device,
                ZH_HANS.err_not_a_device,
                ZH_HANT.err_not_a_device,
            ),
            (
                EN.corrupt_extent_depth,
                ZH_HANS.corrupt_extent_depth,
                ZH_HANT.corrupt_extent_depth,
            ),
            (
                EN.io_read_bytes,
                ZH_HANS.io_read_bytes,
                ZH_HANT.io_read_bytes,
            ),
        ];

        for (en, hans, hant) in pairs {
            let want = placeholders(en);
            assert_eq!(placeholders(hans), want, "zh-Hans differs for {en:?}");
            assert_eq!(placeholders(hant), want, "zh-Hant differs for {en:?}");
        }
    }

    #[test]
    fn usage_lists_every_option() {
        for lang in [Lang::En, Lang::ZhHans, Lang::ZhHant] {
            let text = usage(lang);
            for option in [
                "--regex",
                "--limit",
                "--sort",
                "--device",
                "--threads",
                "--lang",
                "--clean",
                "--quiet",
            ] {
                assert!(text.contains(option), "{lang:?} help omits {option}");
            }
        }
    }

    #[test]
    fn fill_substitutes_and_leaves_unknown_placeholders_alone() {
        assert_eq!(fill("a {x} b", &[("x", "1".into())]), "a 1 b");
        assert_eq!(fill("a {y} b", &[("x", "1".into())]), "a {y} b");
    }
}
