use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid json at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("corpus directory not found: {0}")]
    CorpusMissing(PathBuf),

    #[error("corpus manifest not found: {0}")]
    ManifestMissing(PathBuf),

    #[error("invalid corpus: {0}")]
    Invalid(String),

    #[error("typedb error: {0}")]
    TypeDb(String),

    #[error("version error: {0}")]
    Version(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Invalid(message.into())
}
