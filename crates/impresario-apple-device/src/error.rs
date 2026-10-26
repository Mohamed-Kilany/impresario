use std::path::PathBuf;

/// An Apple device operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The device or host could not be reached, or a protocol step failed.
    #[error("{0}")]
    Device(String),
    /// A local file, directory or plist could not be read or written.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// The global Apple account session is missing or unusable.
    #[error("{0}")]
    Account(String),
    /// Signing materials or the signing operation failed.
    #[error("{0}")]
    Signing(String),
    /// The request is valid but the product does not support it yet.
    #[error("{0}")]
    Unsupported(String),
}

impl Error {
    /// Attach a path or operation to an I/O error.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// A display path for diagnostics without leaking the whole home path.
    #[must_use]
    pub fn path(path: &std::path::Path) -> String {
        path.display().to_string()
    }
}

impl From<idevice::IdeviceError> for Error {
    fn from(value: idevice::IdeviceError) -> Self {
        Self::Device(value.to_string())
    }
}

impl From<plist::Error> for Error {
    fn from(value: plist::Error) -> Self {
        Self::Device(format!("plist: {value}"))
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(value: zip::result::ZipError) -> Self {
        Self::Device(format!("zip: {value}"))
    }
}

/// Convenient result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// A path and a human-readable reason, used by CLI diagnostics.
#[derive(Debug, Clone)]
pub struct Located {
    pub path: PathBuf,
    pub reason: String,
}
