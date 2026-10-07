use std::fmt;
use std::io;

use crate::i18n;

/// Everything that can go wrong, with enough context to be actionable.
///
/// Messages are rendered at [`fmt::Display`] time from the catalogue, so the
/// output follows whatever language was resolved at startup.
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
    /// The regular expression did not compile.
    BadPattern(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
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

    pub fn bad_pattern(what: impl Into<String>) -> Self {
        Error::BadPattern(what.into())
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
            Error::NotExt4 { device, magic } => f.write_str(&i18n::t!(
                err_not_ext4,
                device = device,
                magic = format!("{magic:04x}")
            )),
            Error::Unsupported(what) => f.write_str(&i18n::t!(err_unsupported, what = what)),
            Error::Corrupt(what) => f.write_str(&i18n::t!(err_corrupt, what = what)),
            Error::NoBlockDevice { path, fstype } => {
                f.write_str(&i18n::t!(err_no_block_device, path = path, fstype = fstype))
            }
            Error::Usage(what) => f.write_str(what),
            Error::BadPattern(what) => f.write_str(&i18n::t!(err_bad_pattern, what = what)),
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
