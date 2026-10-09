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
    pub usage_blurb: &'static str,
    pub examples_header: &'static str,
    pub examples: &'static [(&'static str, &'static str)],
    pub usage_header: &'static str,
    pub usage_synopsis: &'static str,
    pub args_header: &'static str,
    pub arg_name: &'static str,
    pub arg_help: &'static str,
    pub options_header: &'static str,
    pub options: &'static [(&'static str, &'static str)],

    // runtime output
    pub hint_root: &'static str,
    pub summary: &'static str,
    pub matches_all: &'static str,
    pub matches_limited: &'static str,
    pub matches_stopped: &'static str,
    pub prompt_continue: &'static str,
    pub note_other_ext4: &'static str,
    pub note_history_ignored: &'static str,
    pub note_history_not_saved: &'static str,

    // argument errors
    pub err_unknown_option: &'static str,
    pub err_missing_name: &'static str,
    pub err_option_needs_value: &'static str,
    pub err_not_a_number: &'static str,
    pub err_threads_min: &'static str,
    pub err_unknown_sort_key: &'static str,
    pub err_unknown_lang: &'static str,
    pub err_bad_path: &'static str,
    pub err_unknown_type: &'static str,
    pub err_unknown_line_field: &'static str,
    pub err_unknown_size_unit: &'static str,
    pub err_base_min: &'static str,
    pub err_base_out_of_range: &'static str,
    pub err_logical_empty: &'static str,
    pub err_logical_operand: &'static str,
    pub err_logical_paren: &'static str,
    pub err_logical_quote: &'static str,

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
    pub io_history_path: &'static str,
    pub io_read_history: &'static str,
    pub io_write_history: &'static str,
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

/// The help text, laid out like the one from `zed --help`: each name on its own
/// line with the description indented under it, sections separated by blank
/// lines.
pub fn usage(lang: Lang) -> String {
    let text = text_for(lang);
    let mut out = String::new();
    out.push_str(text.usage_title);
    out.push_str("\n\n");
    out.push_str(text.usage_blurb);
    out.push_str("\n\n");

    out.push_str(text.examples_header);
    out.push('\n');
    for (command, help) in text.examples {
        push_block(&mut out, command, help, 4);
    }
    out.push('\n');

    out.push_str(text.usage_header);
    out.push(' ');
    out.push_str(text.usage_synopsis);
    out.push_str("\n\n");

    out.push_str(text.args_header);
    out.push('\n');
    push_block(&mut out, text.arg_name, text.arg_help, 2);
    out.push('\n');

    out.push_str(text.options_header);
    out.push('\n');
    for (i, (flag, help)) in text.options.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        push_block(&mut out, flag, help, 2);
    }
    out
}

/// One help item: the name on its own line, then the description indented under
/// it. Blank lines in the description become paragraph breaks.
fn push_block(out: &mut String, name: &str, help: &str, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push_str(name);
    out.push('\n');
    for line in help.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str("          ");
            out.push_str(line);
            out.push('\n');
        }
    }
}

// -------------------------------------------------------------------- English

static EN: Text = Text {
    usage_title: "wis - instant filename search on Linux",
    usage_blurb: "Searches file names on the root filesystem by reading the ext4 metadata\n\
                  directly from the block device, without walking the directory tree\n\
                  through the kernel. Reading a block device requires root; results go\n\
                  to stdout and the summary to stderr.",
    examples_header: "Examples:",
    examples: &[
        ("sudo wis sshd_config", "substring search"),
        ("sudo wis -r '^libssl\\.so\\.[0-9]+$'", "regular expression"),
        (
            "sudo wis -s size desc -n 10 '\\.log$'",
            "the 10 biggest log files",
        ),
        (
            "sudo wis -l 'example AND ( .mp4 OR .mp3 )' 'apple NOT .txt'",
            "logical expressions",
        ),
    ],
    usage_header: "Usage:",
    usage_synopsis: "wis [OPTIONS] <NAME>...",
    args_header: "Arguments:",
    arg_name: "<NAME>...",
    arg_help: "What to look for in a file name. A case-insensitive substring by\n\
               default, or a regular expression with -r. Several names are OR'ed;\n\
               with -l every name is a logical expression.",
    options_header: "Options:",
    options: &[
        (
            "-r, --regex",
            "Regular expression. <NAME> is matched case-insensitively by\n\
             default; add (?-i) to opt out.",
        ),
        (
            "-l, --logical",
            "Treat every name as a logical expression. Uppercase AND, OR and\n\
             NOT combine terms; adjacent terms mean AND and parentheses nest.\n\
             Multiple names are OR'ed. Quote a term to search it literally.",
        ),
        (
            "-n, --limit <N>",
            "Limit the number of results. Applied after sorting.",
        ),
        (
            "-s, --sort <KEY> [DIR]",
            "Sort the results. KEY is name, path (default), ext, size or\n\
             mtime; DIR is asc (default) or desc.",
        ),
        (
            "-d, --device <PATH>",
            "Scan a block device or filesystem image instead of the\n\
             filesystem mounted at /.",
        ),
        (
            "-p, --path <PATH>",
            "Restrict the search path. Only entries strictly below this\n\
             absolute directory are searched; the on-disk tree is used and\n\
             symlinks are not followed.\n\
             \n\
             With -r, PATH is a regular expression and every directory it\n\
             matches has its whole subtree searched.",
        ),
        (
            "-t, --type <TYPE>",
            "Filter by entry type. TYPE is file (f) or directory (d),\n\
             comma-separated or repeated.\n\
             \n\
             file means a regular file; symlinks and other special entries\n\
             appear only without -t.",
        ),
        (
            "-b, --base <NUM>",
            "Search only inside an earlier result set. NUM counts back from the\n\
             most recent record: 1 is the last search, 2 the one before, up to\n\
             10. Every successful search is recorded, empty ones included.",
        ),
        (
            "--line <FIELDS>",
            "Custom output columns. List the fields in the order they should\n\
             appear before the path: mode, mtime, size, user, group, nlink\n\
             (default mode,mtime,size).\n\
             \n\
             size uses adaptive 1024-based units, or size=auto|b|k|m|g|t to\n\
             pin one. mtime is local time; user and group names come from the\n\
             system NSS and fall back to numbers.",
        ),
        (
            "-j, --threads <N>",
            "Concurrent metadata reads (default 16; the scan is\n\
             latency-bound, not CPU-bound).",
        ),
        (
            "--lang <LANG>",
            "Force the output language: zh-Hans, zh-Hant, en. The default\n\
             comes from LC_ALL, LANGUAGE, LC_MESSAGES and LANG.",
        ),
        (
            "-c, --clean",
            "Bare paths only: no columns and no summary on stderr.",
        ),
        (
            "-q, --quiet",
            "Never ask, print everything. By default an output longer than\n\
             1000 lines is asked about after the first 1000, and only y\n\
             continues.\n\
             \n\
             The question appears only when stdin, stdout and stderr are all\n\
             terminals, so pipes and redirects are not interrupted.",
        ),
        ("-h, --help", "Print this help."),
        ("-V, --version", "Print version."),
    ],

    hint_root: "hint: reading a block device needs root -- try `sudo wis ...`",
    summary: "{shown} in {secs}s -- {entries} entries in {dirs} dirs, {mib} MiB read \
              ({inodes} inodes, {nodes} extent nodes) on {threads} threads",
    matches_all: "{n} match(es)",
    matches_limited: "{shown} of {total} match(es)",
    matches_stopped: "{shown} of {total} match(es) (output stopped)",
    prompt_continue: "wis: {remaining} more result(s) after the first {shown}; continue? [y/N] ",
    note_other_ext4: "wis: note: {device} at {target} is a separate ext4 filesystem and is not \
                      searched yet",
    note_history_ignored: "wis: note: history not loaded: {error}",
    note_history_not_saved: "wis: note: history not saved: {error}",

    err_unknown_option: "unknown option `{option}`",
    err_missing_name: "missing NAME",
    err_option_needs_value: "{option} requires a value",
    err_not_a_number: "`{value}` is not a number",
    err_threads_min: "--threads must be at least 1",
    err_unknown_sort_key: "unknown sort key `{key}` (expected one of: {keys})",
    err_unknown_lang: "unknown language `{value}` (expected one of: {langs})",
    err_bad_path: "`{value}` is not an absolute path",
    err_unknown_type: "unknown type `{value}` (expected one of: {types})",
    err_unknown_line_field: "unknown line field `{field}` (expected one of: {fields})",
    err_unknown_size_unit: "unknown size unit `{unit}` (expected one of: {units})",
    err_base_min: "`--base` must be at least 1",
    err_base_out_of_range: "history has only {count} record(s), so `--base {num}` does not exist",
    err_logical_empty: "empty logical expression",
    err_logical_operand: "missing an operand in `{expr}`",
    err_logical_paren: "unbalanced parentheses in `{expr}`",
    err_logical_quote: "unterminated quote in `{expr}`",

    err_not_ext4:
        "{device} is not an ext4 filesystem (superblock magic 0x{magic}, expected 0xef53)",
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
    unsupported_inline_data:
        "filesystem uses `inline_data`: small directories live inside the inode",
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
    io_history_path: "locate wis_history",
    io_read_history: "read history file {path}",
    io_write_history: "write history file {path}",
};

// ------------------------------------------------------------------- 简体中文

static ZH_HANS: Text = Text {
    usage_title: "wis - 在 Linux 上即时搜索文件名",
    usage_blurb:
        "直接读取块设备上的 ext4 元数据来搜索根文件系统上的文件名，不经过内核遍历目录树。\n\
                  读取块设备需要 root 权限；结果写入标准输出，统计和询问写入标准错误。",
    examples_header: "示例:",
    examples: &[
        ("sudo wis sshd_config", "子串搜索"),
        ("sudo wis -r '^libssl\\.so\\.[0-9]+$'", "正则搜索"),
        (
            "sudo wis -s size desc -n 10 '\\.log$'",
            "按大小倒序取前 10 个日志",
        ),
        (
            "sudo wis -l 'example AND ( .mp4 OR .mp3 )' 'apple NOT .txt'",
            "逻辑表达式",
        ),
    ],
    usage_header: "用法:",
    usage_synopsis: "wis [选项] <名称>...",
    args_header: "参数:",
    arg_name: "<名称>...",
    arg_help: "要搜索的文件名内容，默认按不区分大小写的子串匹配，加 -r 后按\n\
               正则表达式匹配。多个名称之间是 OR；加 -l 后每个名称是逻辑表达式。",
    options_header: "选项:",
    options: &[
        (
            "-r, --regex",
            "正则匹配。默认不区分大小写，可用 (?-i) 关闭。",
        ),
        (
            "-l, --logical",
            "把每个名称当作逻辑表达式。\n\
             \n\
             大写 AND、OR、NOT 组合各项，相邻两项等价于 AND，可用括号嵌套；\n\
             多个名称之间是 OR。要用字面词搜索时加双引号。",
        ),
        ("-n, --limit <N>", "限制输出条数，在排序之后应用。"),
        (
            "-s, --sort <键> [方向]",
            "按字段排序。\n\
             \n\
             键：name、path（默认）、ext、size、mtime。\n\
             方向：asc（默认）、desc。",
        ),
        (
            "-d, --device <路径>",
            "指定块设备。\n\
             \n\
             也可以指向文件系统镜像，而非挂载在 / 的文件系统。",
        ),
        (
            "-p, --path <路径>",
            "限定搜索路径。\n\
             \n\
             只搜索该绝对路径之下的条目，按盘上目录结构解释，不解析符号链接。\n\
             与 -r 同用时路径按正则表达式匹配，命中目录的整棵子树都会被搜索。",
        ),
        (
            "-t, --type <类型>",
            "按类型筛选。\n\
             \n\
             可选 file（f）、directory（d），逗号分隔或重复给出。\n\
             file 指普通文件；符号链接、设备等特殊条目只在不加 -t 时出现。",
        ),
        (
            "-b, --base <序号>",
            "只在一条历史记录的结果集里搜索。\n\
             \n\
             <序号> 从最近一条记录往回数：1 是最近一次搜索，2 是上一次，最多 10。\n\
             每次成功的搜索都会记录，空结果也会记录。",
        ),
        (
            "--line <字段>",
            "自定义输出列。\n\
             \n\
             列按给定顺序排在路径之前：mode、mtime、size、user、group、nlink，\n\
             默认 mode,mtime,size。\n\
             size 默认自适应单位，也可用 size=auto|b|k|m|g|t 固定；mtime 为本地时间；\n\
             user、group 通过系统 NSS 解析，查不到时显示数字。",
        ),
        (
            "-j, --threads <N>",
            "并发读取数（默认 16；扫描受设备延迟限制，而非 CPU）。",
        ),
        (
            "--lang <语言>",
            "强制输出语言：zh-Hans、zh-Hant、en。\n\
             默认从 LC_ALL、LANGUAGE、LC_MESSAGES、LANG 推断。",
        ),
        ("-c, --clean", "只输出纯路径，不加列也不输出统计信息。"),
        (
            "-q, --quiet",
            "不询问，直接输出全部结果。\n\
             \n\
             默认输出超过 1000 行时，会先输出 1000 行再询问，只有输入 y 才继续。\n\
             该询问只在标准输入、输出、错误都是终端时出现，管道和重定向不受影响。",
        ),
        ("-h, --help", "显示本帮助。"),
        ("-V, --version", "显示版本。"),
    ],

    hint_root: "提示：读取块设备需要 root 权限，请用 `sudo wis ...`",
    summary: "{shown}，用时 {secs}s —— 扫描 {entries} 个条目、{dirs} 个目录，读取 {mib} MiB\
              （{inodes} 个 inode、{nodes} 个 extent 节点），{threads} 个线程",
    matches_all: "{n} 条命中",
    matches_limited: "{total} 条命中中的 {shown} 条",
    matches_stopped: "{total} 条命中中的 {shown} 条（已停止输出）",
    prompt_continue: "wis: 已输出 {shown} 条，还有 {remaining} 条；继续输出？[y/N] ",
    note_other_ext4: "wis: 提示：{device}（挂载于 {target}）是另一个 ext4 文件系统，暂不搜索",
    note_history_ignored: "wis: 提示：未加载历史记录：{error}",
    note_history_not_saved: "wis: 提示：历史记录未保存：{error}",

    err_unknown_option: "未知选项 `{option}`",
    err_missing_name: "缺少 <名称> 参数",
    err_option_needs_value: "{option} 需要一个值",
    err_not_a_number: "`{value}` 不是有效的数字",
    err_threads_min: "--threads 至少为 1",
    err_unknown_sort_key: "未知的排序键 `{key}`（可选：{keys}）",
    err_unknown_lang: "未知的语言 `{value}`（可选：{langs}）",
    err_bad_path: "`{value}` 不是绝对路径",
    err_unknown_type: "未知的类型 `{value}`（可选：{types}）",
    err_unknown_line_field: "未知的输出列 `{field}`（可选：{fields}）",
    err_unknown_size_unit: "未知的大小单位 `{unit}`（可选：{units}）",
    err_base_min: "`--base` 至少为 1",
    err_base_out_of_range: "历史记录只有 {count} 条，`--base {num}` 不存在",
    err_logical_empty: "空的逻辑表达式",
    err_logical_operand: "`{expr}` 中缺少操作数",
    err_logical_paren: "`{expr}` 的括号不配对",
    err_logical_quote: "`{expr}` 中的引号没有闭合",

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
    io_history_path: "定位 wis_history",
    io_read_history: "读取历史记录文件 {path}",
    io_write_history: "写入历史记录文件 {path}",
};

// ------------------------------------------------------------------- 繁體中文

static ZH_HANT: Text = Text {
    usage_title: "wis - 在 Linux 上即時搜尋檔案名稱",
    usage_blurb:
        "直接讀取區塊裝置上的 ext4 中介資料來搜尋根檔案系統上的檔案名稱，不透過核心走訪目錄樹。\n\
                  讀取區塊裝置需要 root 權限；結果寫入標準輸出，統計與詢問寫入標準錯誤。",
    examples_header: "範例:",
    examples: &[
        ("sudo wis sshd_config", "子字串搜尋"),
        ("sudo wis -r '^libssl\\.so\\.[0-9]+$'", "正規表達式搜尋"),
        (
            "sudo wis -s size desc -n 10 '\\.log$'",
            "依大小倒序取前 10 個日誌",
        ),
        (
            "sudo wis -l 'example AND ( .mp4 OR .mp3 )' 'apple NOT .txt'",
            "邏輯表達式",
        ),
    ],
    usage_header: "用法:",
    usage_synopsis: "wis [選項] <名稱>...",
    args_header: "參數:",
    arg_name: "<名稱>...",
    arg_help: "要搜尋的檔案名稱內容，預設為不分大小寫的子字串，加 -r 後視為\n\
               正規表達式。多個名稱之間是 OR；加 -l 後每個名稱是邏輯表達式。",
    options_header: "選項:",
    options: &[
        (
            "-r, --regex",
            "以正規表達式比對。預設不分大小寫，可用 (?-i) 關閉。",
        ),
        (
            "-l, --logical",
            "把每個名稱當作邏輯表達式。\n\
             \n\
             大寫 AND、OR、NOT 組合各項，相鄰兩項等價於 AND，可用括號嵌套；\n\
             多個名稱之間是 OR。要用字面詞搜尋時加雙引號。",
        ),
        ("-n, --limit <N>", "限制輸出筆數，於排序之後套用。"),
        (
            "-s, --sort <鍵> [方向]",
            "依欄位排序。\n\
             \n\
             <鍵>：name、path（預設）、ext、size、mtime。\n\
             方向：asc（預設）、desc。",
        ),
        (
            "-d, --device <路徑>",
            "指定區塊裝置。\n\
             \n\
             也可以指向檔案系統映像，而非掛載於 / 的檔案系統。",
        ),
        (
            "-p, --path <路徑>",
            "限定搜尋路徑。\n\
             \n\
             只搜尋該絕對路徑之下的項目，依磁碟上的目錄結構解讀，不解析符號連結。\n\
             與 -r 同用時，路徑視為正規表達式，符合的目錄整棵子樹都會被搜尋。",
        ),
        (
            "-t, --type <類型>",
            "依類型篩選。\n\
             \n\
             可選 file（f）、directory（d），以逗號分隔或重複給出。\n\
             file 指一般檔案；符號連結、裝置等特殊項目只在不加 -t 時出現。",
        ),
        (
            "-b, --base <序號>",
            "只在一筆歷史記錄的結果集裡搜尋。\n\
             \n\
             <序號> 從最近一筆記錄往回數：1 是最近一次搜尋，2 是上一次，最多 10。\n\
             每次成功的搜尋都會記錄，空結果也會記錄。",
        ),
        (
            "--line <欄位>",
            "自訂輸出欄位。\n\
             \n\
             欄位依給定順序排在路徑之前：mode、mtime、size、user、group、nlink，\n\
             預設 mode,mtime,size。\n\
             size 預設自適應單位，也可用 size=auto|b|k|m|g|t 固定；mtime 為本地時間；\n\
             user、group 透過系統 NSS 解析，查不到時顯示數字。",
        ),
        (
            "-j, --threads <N>",
            "並行讀取數（預設 16；掃描受裝置延遲限制，而非 CPU）。",
        ),
        (
            "--lang <語言>",
            "強制輸出語言：zh-Hans、zh-Hant、en。\n\
             預設由 LC_ALL、LANGUAGE、LC_MESSAGES、LANG 推斷。",
        ),
        ("-c, --clean", "只輸出純路徑，不加欄位也不輸出統計資訊。"),
        (
            "-q, --quiet",
            "不詢問，直接輸出全部結果。\n\
             \n\
             預設輸出超過 1000 行時，會先輸出 1000 行再詢問，只有輸入 y 才繼續。\n\
             該詢問只在標準輸入、輸出、錯誤都是終端時出現，管道與重定向不受影響。",
        ),
        ("-h, --help", "顯示本說明。"),
        ("-V, --version", "顯示版本。"),
    ],

    hint_root: "提示：讀取區塊裝置需要 root 權限，請用 `sudo wis ...`",
    summary: "{shown}，耗時 {secs}s —— 掃描 {entries} 個項目、{dirs} 個目錄，讀取 {mib} MiB\
              （{inodes} 個 inode、{nodes} 個 extent 節點），{threads} 個執行緒",
    matches_all: "{n} 筆命中",
    matches_limited: "{total} 筆命中中的 {shown} 筆",
    matches_stopped: "{total} 筆命中中的 {shown} 筆（已停止輸出）",
    prompt_continue: "wis: 已輸出 {shown} 筆，還有 {remaining} 筆；繼續輸出？[y/N] ",
    note_other_ext4: "wis: 提示：{device}（掛載於 {target}）是另一個 ext4 檔案系統，暫不搜尋",
    note_history_ignored: "wis: 提示：未載入歷史記錄：{error}",
    note_history_not_saved: "wis: 提示：歷史記錄未儲存：{error}",

    err_unknown_option: "未知的選項 `{option}`",
    err_missing_name: "缺少 <名稱> 參數",
    err_option_needs_value: "{option} 需要一個值",
    err_not_a_number: "`{value}` 不是有效的數字",
    err_threads_min: "--threads 至少為 1",
    err_unknown_sort_key: "未知的排序鍵 `{key}`（可選：{keys}）",
    err_unknown_lang: "未知的語言 `{value}`（可選：{langs}）",
    err_bad_path: "`{value}` 不是絕對路徑",
    err_unknown_type: "未知的類型 `{value}`（可選：{types}）",
    err_unknown_line_field: "未知的輸出欄位 `{field}`（可選：{fields}）",
    err_unknown_size_unit: "未知的大小單位 `{unit}`（可選：{units}）",
    err_base_min: "`--base` 至少為 1",
    err_base_out_of_range: "歷史記錄只有 {count} 筆，`--base {num}` 不存在",
    err_logical_empty: "空的邏輯表達式",
    err_logical_operand: "`{expr}` 中缺少運算元",
    err_logical_paren: "`{expr}` 的括號不配對",
    err_logical_quote: "`{expr}` 中的引號沒有閉合",

    err_not_ext4: "{device} 不是 ext4 檔案系統（超級區塊 magic 為 0x{magic}，應為 0xef53）",
    err_unsupported: "不支援：{what}",
    err_corrupt: "檔案系統中介資料損毀：{what}",
    err_no_block_device: "無法搜尋 {path}：它是 {fstype} 檔案系統，目前只支援區塊裝置上的 ext4",
    err_bad_pattern: "正規表達式有誤：{what}",
    err_not_a_device: "{path} 既不是區塊裝置也不是一般檔案",

    unsupported_filetype:
        "檔案系統缺少 `filetype` 特性，目錄項佈局不同（可用 `tune2fs -O filetype` 啟用）",
    unsupported_meta_bg:
        "檔案系統使用了 `meta_bg`：區塊群組描述元不是連續的單一表，本程式不處理這種佈局",
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
    io_history_path: "定位 wis_history",
    io_read_history: "讀取歷史記錄檔案 {path}",
    io_write_history: "寫入歷史記錄檔案 {path}",
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
            let mut all: Vec<&str> = vec![
                text.usage_title,
                text.usage_blurb,
                text.examples_header,
                text.usage_header,
                text.usage_synopsis,
                text.args_header,
                text.arg_name,
                text.arg_help,
                text.options_header,
                text.hint_root,
                text.summary,
                text.matches_all,
                text.matches_limited,
                text.matches_stopped,
                text.prompt_continue,
                text.note_other_ext4,
                text.note_history_ignored,
                text.note_history_not_saved,
                text.err_unknown_option,
                text.err_missing_name,
                text.err_option_needs_value,
                text.err_not_a_number,
                text.err_threads_min,
                text.err_unknown_sort_key,
                text.err_unknown_lang,
                text.err_bad_path,
                text.err_unknown_type,
                text.err_unknown_line_field,
                text.err_unknown_size_unit,
                text.err_base_min,
                text.err_base_out_of_range,
                text.err_logical_empty,
                text.err_logical_operand,
                text.err_logical_paren,
                text.err_logical_quote,
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
                text.io_history_path,
                text.io_read_history,
                text.io_write_history,
            ];
            for (name, help) in text.examples {
                all.push(*name);
                all.push(*help);
            }
            for (flag, help) in text.options {
                all.push(*flag);
                all.push(*help);
            }
            for (i, message) in all.iter().enumerate() {
                assert!(!message.is_empty(), "{lang:?} field {i} is empty");
            }
        }
    }

    #[test]
    fn every_option_has_a_name_and_help() {
        for lang in [Lang::En, Lang::ZhHans, Lang::ZhHant] {
            let text = text_for(lang);
            assert!(!text.examples.is_empty(), "{lang:?} has no examples");
            assert!(text.options.len() >= 13, "{lang:?} lost an option");
            for (name, help) in text.examples.iter().chain(text.options.iter()) {
                assert!(!name.trim().is_empty(), "{lang:?} has an empty name");
                assert!(!help.trim().is_empty(), "{lang:?} {name} has no help");
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

        let pairs: [(&str, &str, &str); 31] = [
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
                EN.note_history_ignored,
                ZH_HANS.note_history_ignored,
                ZH_HANT.note_history_ignored,
            ),
            (
                EN.note_history_not_saved,
                ZH_HANS.note_history_not_saved,
                ZH_HANT.note_history_not_saved,
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
            (EN.err_bad_path, ZH_HANS.err_bad_path, ZH_HANT.err_bad_path),
            (
                EN.err_unknown_type,
                ZH_HANS.err_unknown_type,
                ZH_HANT.err_unknown_type,
            ),
            (
                EN.err_unknown_line_field,
                ZH_HANS.err_unknown_line_field,
                ZH_HANT.err_unknown_line_field,
            ),
            (
                EN.err_unknown_size_unit,
                ZH_HANS.err_unknown_size_unit,
                ZH_HANT.err_unknown_size_unit,
            ),
            (
                EN.err_base_out_of_range,
                ZH_HANS.err_base_out_of_range,
                ZH_HANT.err_base_out_of_range,
            ),
            (
                EN.err_logical_operand,
                ZH_HANS.err_logical_operand,
                ZH_HANT.err_logical_operand,
            ),
            (
                EN.err_logical_paren,
                ZH_HANS.err_logical_paren,
                ZH_HANT.err_logical_paren,
            ),
            (
                EN.err_logical_quote,
                ZH_HANS.err_logical_quote,
                ZH_HANT.err_logical_quote,
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
            (
                EN.io_read_history,
                ZH_HANS.io_read_history,
                ZH_HANT.io_read_history,
            ),
            (
                EN.io_write_history,
                ZH_HANS.io_write_history,
                ZH_HANT.io_write_history,
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
                "--logical",
                "--limit",
                "--sort",
                "--device",
                "--path",
                "--type",
                "--line",
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
