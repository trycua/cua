/// Share errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Bad input.
    #[error("{0}")]
    InvalidArgument(String),
    /// No such share.
    #[error("{0}")]
    NotFound(String),
    /// The store could not be read or written.
    #[error("share store: {0}")]
    Store(String),
    /// I/O failure.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// `InvalidArgument` from anything printable.
    pub fn invalid(m: impl Into<String>) -> Self {
        Error::InvalidArgument(m.into())
    }
}

/// `Result` with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
