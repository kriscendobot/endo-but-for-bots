use thiserror::Error;

/// Errors exposed by the safe `endor-git` boundary.
#[derive(Debug, Error)]
pub enum GitError {
    #[error("libgit2 error: {0}")]
    Libgit2(#[from] git2::Error),
    #[error("invalid Git input: {0}")]
    InvalidInput(String),
    #[error("Git object was not found: {0}")]
    ObjectNotFound(String),
    #[error("Git object format mismatch")]
    ObjectFormatMismatch,
    #[error("Git reference changed concurrently: {0}")]
    ReferenceConflict(String),
    #[error("symbolic Git references are not writable through this boundary: {0}")]
    SymbolicReference(String),
    #[error("Git reference writes are restricted to refs/endor/: {0}")]
    ReferenceOutsideNamespace(String),
    #[error("custom libgit2 callback failed: {0}")]
    Callback(String),
    #[error("custom backend operation is unsupported: {0}")]
    Unsupported(String),
    #[error("Git blocking pool has shut down")]
    Shutdown,
    #[error("Git blocking task failed: {0}")]
    BlockingTask(String),
}

impl GitError {
    pub(crate) fn from_git2(error: git2::Error) -> Self {
        match error.code() {
            git2::ErrorCode::NotFound => Self::ObjectNotFound(error.message().to_owned()),
            git2::ErrorCode::Modified | git2::ErrorCode::Exists => {
                Self::ReferenceConflict(error.message().to_owned())
            }
            _ => Self::Libgit2(error),
        }
    }
}
