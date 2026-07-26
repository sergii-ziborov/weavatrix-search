use std::error::Error as StdError;
use std::fmt;
use std::path::PathBuf;

/// Result type returned by search operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Fatal query, scanner, decoding, I/O, or archive failure.
#[derive(Debug)]
pub enum Error {
    /// Empty patterns are rejected to avoid accidental match-everywhere work.
    EmptyQuery,
    /// A regular expression could not be compiled.
    Regex(Box<regex_automata::meta::BuildError>),
    /// Repository discovery or content delivery failed.
    Scan(weavatrix_scan::Error),
    /// An explicit encoding label is not recognized.
    InvalidEncoding(String),
    /// An archive or decoder read failed.
    Io {
        /// Source or virtual member path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// An archive could not be inspected safely.
    Archive {
        /// Archive or virtual member path.
        path: String,
        /// Failure detail.
        message: String,
    },
    /// A configured source or decoded-content limit was exceeded.
    Limit {
        /// Source or virtual member path.
        path: String,
        /// Limit detail.
        message: String,
    },
}

impl Error {
    #[cfg(feature = "archives")]
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn archive(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Archive {
            path: path.into(),
            message: message.into(),
        }
    }

    pub(crate) fn limit(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Limit {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyQuery => formatter.write_str("search query must not be empty"),
            Self::Regex(error) => write!(formatter, "invalid regular expression: {error}"),
            Self::Scan(error) => write!(formatter, "repository scan failed: {error}"),
            Self::InvalidEncoding(label) => write!(formatter, "unknown encoding label: {label}"),
            Self::Io { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::Archive { path, message } => {
                write!(formatter, "failed to search archive {path}: {message}")
            }
            Self::Limit { path, message } => {
                write!(formatter, "search limit reached for {path}: {message}")
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Regex(error) => Some(error.as_ref()),
            Self::Scan(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::EmptyQuery
            | Self::InvalidEncoding(_)
            | Self::Archive { .. }
            | Self::Limit { .. } => None,
        }
    }
}

impl From<regex_automata::meta::BuildError> for Error {
    fn from(error: regex_automata::meta::BuildError) -> Self {
        Self::Regex(Box::new(error))
    }
}

impl From<weavatrix_scan::Error> for Error {
    fn from(error: weavatrix_scan::Error) -> Self {
        Self::Scan(error)
    }
}
