use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::ffi::CallbackState;
use crate::{
    GitError, GitObject, GitObjectDb, GitObjectFormat, GitObjectId, GitObjectKind, GitRefName,
    GitTreeEntry, GitVerifyReport, GitVerifyScope, Libgit2Repository,
};

/// Safe storage operations implemented by a custom object/reference database.
///
/// Implementations may use a CAS and transactional database. The FFI module is
/// solely responsible for translating these calls to libgit2 callbacks.
pub trait BackendStorage: Send + Sync + 'static {
    fn object_exists(&self, id: &GitObjectId) -> Result<bool, GitError>;
    fn read_object(&self, id: &GitObjectId) -> Result<GitObject, GitError>;
    fn write_object(&self, kind: GitObjectKind, bytes: &[u8]) -> Result<GitObjectId, GitError>;
    fn object_ids(&self) -> Result<Vec<GitObjectId>, GitError>;
    fn resolve_ref(&self, name: &GitRefName) -> Result<Option<GitObjectId>, GitError>;
    fn references(&self) -> Result<Vec<(GitRefName, GitObjectId)>, GitError>;
    fn update_ref_if(
        &self,
        name: &GitRefName,
        expected: Option<&GitObjectId>,
        next: &GitObjectId,
        message: &str,
    ) -> Result<(), GitError>;
}

/// A repository whose object and reference databases are installed through FFI.
pub struct Libgit2Backend {
    repository: Libgit2Repository,
    callbacks: Arc<CallbackState>,
}

impl Libgit2Backend {
    pub fn init_bare(
        path: impl AsRef<Path>,
        format: GitObjectFormat,
        storage: Arc<dyn BackendStorage>,
    ) -> Result<Self, GitError> {
        let repository = Libgit2Repository::init_bare(path, format)?;
        let callbacks = CallbackState::new(format, storage);
        {
            let locked = repository.lock_repository()?;
            crate::ffi::install(&locked, Arc::clone(&callbacks))?;
        }
        Ok(Self {
            repository,
            callbacks,
        })
    }

    pub fn object_format(&self) -> GitObjectFormat {
        self.repository.object_format()
    }

    fn callback_result<T>(&self, result: Result<T, GitError>) -> Result<T, GitError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => match self.callbacks.take_error()? {
                Some(callback_error) => Err(callback_error),
                None => Err(error),
            },
        }
    }
}

impl GitObjectDb for Libgit2Backend {
    fn object_exists(&self, id: &GitObjectId) -> Result<bool, GitError> {
        self.callback_result(self.repository.object_exists(id))
    }

    fn read_object(&self, id: &GitObjectId) -> Result<GitObject, GitError> {
        self.callback_result(self.repository.read_object(id))
    }

    fn write_object(&self, kind: GitObjectKind, bytes: &[u8]) -> Result<GitObjectId, GitError> {
        self.callback_result(self.repository.write_object(kind, bytes))
    }

    fn read_tree(&self, id: &GitObjectId) -> Result<Vec<GitTreeEntry>, GitError> {
        self.callback_result(self.repository.read_tree(id))
    }

    fn resolve_ref(&self, name: &GitRefName) -> Result<Option<GitObjectId>, GitError> {
        self.callback_result(self.repository.resolve_ref(name))
    }

    fn update_ref_if(
        &self,
        name: &GitRefName,
        expected: Option<&GitObjectId>,
        next: &GitObjectId,
        message: &str,
    ) -> Result<(), GitError> {
        self.callback_result(self.repository.update_ref_if(name, expected, next, message))
    }

    fn verify(&self, scope: GitVerifyScope) -> Result<GitVerifyReport, GitError> {
        self.callback_result(self.repository.verify(scope))
    }
}

/// A generic in-memory adapter used by the shared conformance suite.
pub struct InMemoryBackend {
    format: GitObjectFormat,
    objects: Mutex<HashMap<GitObjectId, GitObject>>,
    references: Mutex<HashMap<GitRefName, GitObjectId>>,
}

impl InMemoryBackend {
    pub fn new(format: GitObjectFormat) -> Self {
        Self {
            format,
            objects: Mutex::new(HashMap::new()),
            references: Mutex::new(HashMap::new()),
        }
    }

    fn lock_objects(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<GitObjectId, GitObject>>, GitError> {
        self.objects
            .lock()
            .map_err(|_| GitError::Callback("in-memory object mutex was poisoned".to_owned()))
    }

    fn lock_references(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<GitRefName, GitObjectId>>, GitError> {
        self.references
            .lock()
            .map_err(|_| GitError::Callback("in-memory reference mutex was poisoned".to_owned()))
    }
}

impl BackendStorage for InMemoryBackend {
    fn object_exists(&self, id: &GitObjectId) -> Result<bool, GitError> {
        if id.format() != self.format {
            return Ok(false);
        }
        Ok(self.lock_objects()?.contains_key(id))
    }

    fn read_object(&self, id: &GitObjectId) -> Result<GitObject, GitError> {
        self.lock_objects()?
            .get(id)
            .cloned()
            .ok_or_else(|| GitError::ObjectNotFound(id.to_string()))
    }

    fn write_object(&self, kind: GitObjectKind, bytes: &[u8]) -> Result<GitObjectId, GitError> {
        let id = git2::Oid::hash_object_ext(kind.to_git2(), bytes, self.format.to_git2())
            .map_err(GitError::from_git2)?;
        let id = GitObjectId::from_git2(id)?;
        self.lock_objects()?.insert(
            id,
            GitObject {
                kind,
                bytes: bytes.to_vec(),
            },
        );
        Ok(id)
    }

    fn object_ids(&self) -> Result<Vec<GitObjectId>, GitError> {
        Ok(self.lock_objects()?.keys().copied().collect())
    }

    fn resolve_ref(&self, name: &GitRefName) -> Result<Option<GitObjectId>, GitError> {
        Ok(self.lock_references()?.get(name).copied())
    }

    fn references(&self) -> Result<Vec<(GitRefName, GitObjectId)>, GitError> {
        Ok(self
            .lock_references()?
            .iter()
            .map(|(name, id)| (name.clone(), *id))
            .collect())
    }

    fn update_ref_if(
        &self,
        name: &GitRefName,
        expected: Option<&GitObjectId>,
        next: &GitObjectId,
        _message: &str,
    ) -> Result<(), GitError> {
        if next.format() != self.format {
            return Err(GitError::ObjectFormatMismatch);
        }
        if !self.object_exists(next)? {
            return Err(GitError::ObjectNotFound(next.to_string()));
        }
        let mut references = self.lock_references()?;
        if references.get(name) != expected {
            return Err(GitError::ReferenceConflict(name.as_str().to_owned()));
        }
        references.insert(name.clone(), *next);
        Ok(())
    }
}
