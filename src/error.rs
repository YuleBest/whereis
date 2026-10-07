use std::fmt;
use std::io;

/// Everything that can go wrong, with enough context to be actionable.
#[derive(Debug)]
pub enum Error {
    /// An I/O operation failed; the string says which one.
    Io { context: String, source: io::Error },
    /// The device does not carry an ext4 superblock.
    NotExt4 { device: String, magic: u16 },
    /// We understood the filesystem, but it uses a feature we cannot decode.
    Unsupported(String),
    /// The filesystem metadata is malformed.
    Corrupt(String),
    /// We could not find a block device to read the requested tree from.
    NoBlockDevice { path: String, fstype: String },
    /// The command line did not make sense.
    Usage(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Error::Io { context: context.into(), source }
    }

    pub fn unsupported(what: impl Into<String>) -> Self {
        Error::Unsupported(what.into())
    }

    pub fn corrupt(what: impl Into<String>) -> Self {
        Error::Corrupt(what.into())
    }

    pub fn usage(what: impl Into<String>) -> Self {
        Error::Usage(what.into())
    }

    /// The underlying I/O error, if this is an I/O error at all.
    fn io_kind(&self) -> Option<io::ErrorKind> {
        match self {
            Error::Io { source, .. } => Some(source.kind()),
            _ => None,
        }
    }

    pub fn is_permission_denied(&self) -> bool {
        self.io_kind() == Some(io::ErrorKind::PermissionDenied)
    }

    pub fn is_broken_pipe(&self) -> bool {
        self.io_kind() == Some(io::ErrorKind::BrokenPipe)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { context, source } => write!(f, "{context}: {source}"),
            Error::NotExt4 { device, magic } => write!(
                f,
                "{device} is not an ext4 filesystem (superblock magic 0x{magic:04x}, expected 0xef53)"
            ),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Corrupt(what) => write!(f, "malformed filesystem metadata: {what}"),
            Error::NoBlockDevice { path, fstype } => write!(
                f,
                "cannot search {path}: it is a {fstype} filesystem, and only ext4 on a block device is supported so far"
            ),
            Error::Usage(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
