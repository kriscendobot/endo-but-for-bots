//! A narrow, synchronous Git object and reference database over libgit2.
//!
//! Git object identifiers intentionally have their own type in this crate.
//! They are not interchangeable with Endor `ContentStore` identifiers.

mod backend;
mod error;
mod ffi;
mod repository;
mod types;

pub use backend::{BackendStorage, InMemoryBackend, Libgit2Backend};
pub use error::GitError;
pub use repository::{GitObjectDb, Libgit2Repository};
pub use types::{
    GitObject, GitObjectFormat, GitObjectId, GitObjectKind, GitRefName, GitTreeEntry,
    GitVerifyReport, GitVerifyScope,
};

use std::future::Future;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A reusable bound on concurrent synchronous Git calls from async services.
#[derive(Clone, Debug)]
pub struct GitBlockingPool {
    permits: Arc<Semaphore>,
}

impl GitBlockingPool {
    /// Creates a blocking pool that permits at most `parallelism` active calls.
    pub fn new(parallelism: usize) -> Result<Self, GitError> {
        if parallelism == 0 {
            return Err(GitError::InvalidInput(
                "Git blocking pool parallelism must be positive".to_owned(),
            ));
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(parallelism)),
        })
    }

    /// Runs one synchronous database call without blocking an async executor.
    pub fn run<T, F>(&self, call: F) -> impl Future<Output = Result<T, GitError>> + Send + 'static
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, GitError> + Send + 'static,
    {
        let permits = Arc::clone(&self.permits);
        async move {
            let permit: OwnedSemaphorePermit = permits
                .acquire_owned()
                .await
                .map_err(|_| GitError::Shutdown)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                call()
            })
            .await
            .map_err(|error| GitError::BlockingTask(error.to_string()))?
        }
    }
}

/// Returns the statically linked libgit2 version for diagnostics and audits.
pub fn libgit2_version() -> (u32, u32, u32) {
    let version = git2::Version::get();
    version.libgit2_version()
}

#[cfg(test)]
mod tests {
    use super::{GitBlockingPool, GitError};

    #[test]
    fn blocking_pool_requires_positive_parallelism() {
        assert!(matches!(
            GitBlockingPool::new(0),
            Err(GitError::InvalidInput(_))
        ));
    }

    #[test]
    fn blocking_pool_returns_the_synchronous_result() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build Tokio runtime");
        let pool = GitBlockingPool::new(1).expect("build blocking pool");
        let result = runtime
            .block_on(pool.run(|| Ok(42)))
            .expect("blocking call succeeds");
        assert_eq!(result, 42);
    }

    #[test]
    fn blocking_pool_reports_a_panicking_task() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build Tokio runtime");
        let pool = GitBlockingPool::new(1).expect("build blocking pool");
        let error = runtime
            .block_on(pool.run(|| -> Result<(), GitError> { panic!("test panic") }))
            .expect_err("panicking blocking task must fail");
        assert!(matches!(error, GitError::BlockingTask(_)));
    }

    #[test]
    fn blocking_pool_preserves_the_synchronous_error() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build Tokio runtime");
        let pool = GitBlockingPool::new(1).expect("build blocking pool");
        let error = runtime
            .block_on(pool.run(|| -> Result<(), GitError> {
                Err(GitError::Unsupported("test operation".to_owned()))
            }))
            .expect_err("synchronous error must be preserved");
        assert!(matches!(error, GitError::Unsupported(message) if message == "test operation"));
    }
}
