use std::path::Path;
use std::sync::Mutex;

use git2::{ErrorCode, Repository, RepositoryInitOptions};

use crate::{
    GitError, GitObject, GitObjectFormat, GitObjectId, GitObjectKind, GitRefName, GitTreeEntry,
    GitVerifyReport, GitVerifyScope,
};

/// The synchronous storage contract shared by filesystem and custom backends.
pub trait GitObjectDb: Send + Sync {
    fn object_exists(&self, id: &GitObjectId) -> Result<bool, GitError>;
    fn read_object(&self, id: &GitObjectId) -> Result<GitObject, GitError>;
    fn write_object(&self, kind: GitObjectKind, bytes: &[u8]) -> Result<GitObjectId, GitError>;
    fn read_tree(&self, id: &GitObjectId) -> Result<Vec<GitTreeEntry>, GitError>;
    fn resolve_ref(&self, name: &GitRefName) -> Result<Option<GitObjectId>, GitError>;
    fn update_ref_if(
        &self,
        name: &GitRefName,
        expected: Option<&GitObjectId>,
        next: &GitObjectId,
        message: &str,
    ) -> Result<(), GitError>;
    fn verify(&self, scope: GitVerifyScope) -> Result<GitVerifyReport, GitError>;
}

/// An ordinary bare or non-bare repository using libgit2's filesystem stores.
pub struct Libgit2Repository {
    pub(crate) repository: Mutex<Repository>,
    pub(crate) format: GitObjectFormat,
}

impl Libgit2Repository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, GitError> {
        let repository = Repository::open(path).map_err(GitError::from_git2)?;
        let format = GitObjectFormat::from_git2(repository.object_format());
        Ok(Self {
            repository: Mutex::new(repository),
            format,
        })
    }

    pub fn init_bare(path: impl AsRef<Path>, format: GitObjectFormat) -> Result<Self, GitError> {
        let mut options = RepositoryInitOptions::new();
        options.bare(true).object_format(format.to_git2());
        let repository = Repository::init_opts(path, &options).map_err(GitError::from_git2)?;
        Ok(Self {
            repository: Mutex::new(repository),
            format,
        })
    }

    pub fn object_format(&self) -> GitObjectFormat {
        self.format
    }

    pub(crate) fn lock_repository(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Repository>, GitError> {
        self.repository
            .lock()
            .map_err(|_| GitError::Callback("repository mutex was poisoned".to_owned()))
    }

    pub(crate) fn check_id_format(&self, id: &GitObjectId) -> Result<(), GitError> {
        if id.format() != self.format {
            return Err(GitError::ObjectFormatMismatch);
        }
        Ok(())
    }
}

impl GitObjectDb for Libgit2Repository {
    fn object_exists(&self, id: &GitObjectId) -> Result<bool, GitError> {
        self.check_id_format(id)?;
        let repository = self.lock_repository()?;
        let database = repository.odb().map_err(GitError::from_git2)?;
        match database.read_header(id.to_git2()?) {
            Ok(_) => Ok(true),
            Err(error) if error.code() == ErrorCode::NotFound => Ok(false),
            Err(error) => Err(GitError::from_git2(error)),
        }
    }

    fn read_object(&self, id: &GitObjectId) -> Result<GitObject, GitError> {
        self.check_id_format(id)?;
        let repository = self.lock_repository()?;
        let database = repository.odb().map_err(GitError::from_git2)?;
        let object = database.read(id.to_git2()?).map_err(GitError::from_git2)?;
        Ok(GitObject {
            kind: GitObjectKind::from_git2(object.kind())?,
            bytes: object.data().to_vec(),
        })
    }

    fn write_object(&self, kind: GitObjectKind, bytes: &[u8]) -> Result<GitObjectId, GitError> {
        let repository = self.lock_repository()?;
        let id = repository
            .odb()
            .map_err(GitError::from_git2)?
            .write(kind.to_git2(), bytes)
            .map_err(GitError::from_git2)?;
        GitObjectId::from_git2(id)
    }

    fn read_tree(&self, id: &GitObjectId) -> Result<Vec<GitTreeEntry>, GitError> {
        self.check_id_format(id)?;
        let repository = self.lock_repository()?;
        let tree = repository
            .find_tree(id.to_git2()?)
            .map_err(GitError::from_git2)?;
        tree.iter()
            .map(|entry| {
                let kind = entry.kind().ok_or_else(|| {
                    GitError::InvalidInput("tree entry has an unknown object kind".to_owned())
                })?;
                Ok(GitTreeEntry {
                    name: entry.name_bytes().to_vec(),
                    id: GitObjectId::from_git2(entry.id())?,
                    file_mode: entry.filemode(),
                    kind: GitObjectKind::from_git2(kind)?,
                })
            })
            .collect()
    }

    fn resolve_ref(&self, name: &GitRefName) -> Result<Option<GitObjectId>, GitError> {
        let repository = self.lock_repository()?;
        let result = match repository.find_reference(name.as_str()) {
            Ok(reference) => {
                let target = reference
                    .target()
                    .ok_or_else(|| GitError::SymbolicReference(name.as_str().to_owned()))?;
                Ok(Some(GitObjectId::from_git2(target)?))
            }
            Err(error) if error.code() == ErrorCode::NotFound => Ok(None),
            Err(error) => Err(GitError::from_git2(error)),
        };
        result
    }

    fn update_ref_if(
        &self,
        name: &GitRefName,
        expected: Option<&GitObjectId>,
        next: &GitObjectId,
        message: &str,
    ) -> Result<(), GitError> {
        if !name.as_str().starts_with("refs/endor/") {
            return Err(GitError::ReferenceOutsideNamespace(
                name.as_str().to_owned(),
            ));
        }
        self.check_id_format(next)?;
        if let Some(expected) = expected {
            self.check_id_format(expected)?;
        }
        if !self.object_exists(next)? {
            return Err(GitError::ObjectNotFound(next.to_string()));
        }

        let repository = self.lock_repository()?;
        let result = match expected {
            Some(expected) => repository.reference_matching(
                name.as_str(),
                next.to_git2()?,
                false,
                expected.to_git2()?,
                message,
            ),
            None => repository.reference(name.as_str(), next.to_git2()?, false, message),
        };
        result.map(|_| ()).map_err(GitError::from_git2)
    }

    fn verify(&self, scope: GitVerifyScope) -> Result<GitVerifyReport, GitError> {
        match scope {
            GitVerifyScope::Object(id) => {
                self.read_object(&id)?;
                Ok(GitVerifyReport {
                    objects_checked: 1,
                    references_checked: 0,
                })
            }
            GitVerifyScope::Full => {
                let repository = self.lock_repository()?;
                let database = repository.odb().map_err(GitError::from_git2)?;
                let mut object_ids = Vec::new();
                database
                    .foreach(|id| {
                        object_ids.push(*id);
                        true
                    })
                    .map_err(GitError::from_git2)?;
                for id in &object_ids {
                    database.read(*id).map_err(GitError::from_git2)?;
                }
                let mut references_checked = 0;
                for reference in repository.references().map_err(GitError::from_git2)? {
                    let reference = reference.map_err(GitError::from_git2)?;
                    if let Some(target) = reference.target() {
                        if !database.exists(target) {
                            return Err(GitError::ObjectNotFound(target.to_string()));
                        }
                    }
                    references_checked += 1;
                }
                Ok(GitVerifyReport {
                    objects_checked: object_ids.len(),
                    references_checked,
                })
            }
        }
    }
}
