use std::fmt;

use crate::GitError;

/// The hash algorithm used by a Git repository.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GitObjectFormat {
    Sha1,
    Sha256,
}

impl GitObjectFormat {
    pub(crate) fn width(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha256 => 32,
        }
    }

    pub(crate) fn to_git2(self) -> git2::ObjectFormat {
        match self {
            Self::Sha1 => git2::ObjectFormat::Sha1,
            Self::Sha256 => git2::ObjectFormat::Sha256,
        }
    }

    pub(crate) fn from_git2(format: git2::ObjectFormat) -> Self {
        match format {
            git2::ObjectFormat::Sha1 => Self::Sha1,
            git2::ObjectFormat::Sha256 => Self::Sha256,
        }
    }
}

/// A fixed-width Git object identifier, distinct from an Endor content ID.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct GitObjectId {
    format: GitObjectFormat,
    bytes: [u8; 32],
}

impl GitObjectId {
    pub fn new(format: GitObjectFormat, bytes: &[u8]) -> Result<Self, GitError> {
        if bytes.len() != format.width() {
            return Err(GitError::InvalidInput(format!(
                "{} object ID requires {} bytes, received {}",
                match format {
                    GitObjectFormat::Sha1 => "SHA-1",
                    GitObjectFormat::Sha256 => "SHA-256",
                },
                format.width(),
                bytes.len()
            )));
        }
        let mut fixed = [0; 32];
        fixed[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            format,
            bytes: fixed,
        })
    }

    pub fn format(self) -> GitObjectFormat {
        self.format
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.format.width()]
    }

    pub(crate) fn from_git2(id: git2::Oid) -> Result<Self, GitError> {
        Self::new(
            GitObjectFormat::from_git2(id.object_format()),
            id.as_bytes(),
        )
    }

    pub(crate) fn to_git2(self) -> Result<git2::Oid, GitError> {
        git2::Oid::from_bytes(self.as_bytes()).map_err(GitError::from_git2)
    }
}

impl fmt::Debug for GitObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for GitObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.as_bytes() {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// A Git object kind accepted by the local storage profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl GitObjectKind {
    pub(crate) fn to_git2(self) -> git2::ObjectType {
        match self {
            Self::Commit => git2::ObjectType::Commit,
            Self::Tree => git2::ObjectType::Tree,
            Self::Blob => git2::ObjectType::Blob,
            Self::Tag => git2::ObjectType::Tag,
        }
    }

    pub(crate) fn from_git2(kind: git2::ObjectType) -> Result<Self, GitError> {
        match kind {
            git2::ObjectType::Commit => Ok(Self::Commit),
            git2::ObjectType::Tree => Ok(Self::Tree),
            git2::ObjectType::Blob => Ok(Self::Blob),
            git2::ObjectType::Tag => Ok(Self::Tag),
            _ => Err(GitError::InvalidInput(format!(
                "unsupported Git object kind {kind:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitObject {
    pub kind: GitObjectKind,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitTreeEntry {
    pub name: Vec<u8>,
    pub id: GitObjectId,
    pub file_mode: i32,
    pub kind: GitObjectKind,
}

/// A validated Git reference name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GitRefName(String);

impl GitRefName {
    pub fn new(name: impl Into<String>) -> Result<Self, GitError> {
        let name = name.into();
        if !git2::Reference::is_valid_name(&name) {
            return Err(GitError::InvalidInput(format!(
                "invalid Git reference name {name:?}"
            )));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitVerifyScope {
    Object(GitObjectId),
    Full,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitVerifyReport {
    pub objects_checked: usize,
    pub references_checked: usize,
}
