use std::sync::{Arc, Barrier};

use endor_git::{
    BackendStorage, GitError, GitObject, GitObjectDb, GitObjectFormat, GitObjectId, GitObjectKind,
    GitRefName, GitVerifyScope, InMemoryBackend, Libgit2Backend, Libgit2Repository,
};

fn expected_blob_id(format: GitObjectFormat) -> &'static str {
    match format {
        GitObjectFormat::Sha1 => "ce013625030ba8dba906f756967f9e9ca394464a",
        GitObjectFormat::Sha256 => {
            "2cf8d83d9ee29543b34a87727421fdecb7e3f3a183d337639025de576db9ebb4"
        }
    }
}

fn run_conformance(database: &dyn GitObjectDb, format: GitObjectFormat) {
    let empty_id = database
        .write_object(GitObjectKind::Blob, b"")
        .expect("write empty blob");
    assert_eq!(
        database.read_object(&empty_id).expect("read empty blob"),
        GitObject {
            kind: GitObjectKind::Blob,
            bytes: Vec::new(),
        }
    );

    let blob_id = database
        .write_object(GitObjectKind::Blob, b"hello\n")
        .expect("write blob");
    assert_eq!(blob_id.format(), format);
    assert_eq!(blob_id.to_string(), expected_blob_id(format));
    assert!(database.object_exists(&blob_id).expect("object exists"));
    assert_eq!(
        database.read_object(&blob_id).expect("read blob"),
        GitObject {
            kind: GitObjectKind::Blob,
            bytes: b"hello\n".to_vec(),
        }
    );

    let mut tree_bytes = b"100644 greeting.txt\0".to_vec();
    tree_bytes.extend_from_slice(blob_id.as_bytes());
    let tree_id = database
        .write_object(GitObjectKind::Tree, &tree_bytes)
        .expect("write tree");
    let tree = database.read_tree(&tree_id).expect("read tree");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, b"greeting.txt");
    assert_eq!(tree[0].id, blob_id);
    assert_eq!(tree[0].kind, GitObjectKind::Blob);

    let reference = GitRefName::new("refs/endor/main").expect("valid reference");
    database
        .update_ref_if(&reference, None, &tree_id, "create test ref")
        .expect("create reference");
    assert_eq!(
        database.resolve_ref(&reference).expect("resolve reference"),
        Some(tree_id)
    );

    let report = database
        .verify(GitVerifyScope::Full)
        .expect("verify repository");
    assert_eq!(report.objects_checked, 3);
    assert_eq!(report.references_checked, 1);

    let outside = GitRefName::new("refs/heads/main").expect("valid reference");
    assert!(matches!(
        database.update_ref_if(&outside, None, &tree_id, "denied"),
        Err(GitError::ReferenceOutsideNamespace(_))
    ));
}

#[test]
fn filesystem_sha1_conformance() {
    let directory = tempfile::tempdir().expect("temp directory");
    let database = Libgit2Repository::init_bare(directory.path(), GitObjectFormat::Sha1)
        .expect("init repository");
    run_conformance(&database, GitObjectFormat::Sha1);
}

#[test]
fn filesystem_sha256_conformance() {
    let directory = tempfile::tempdir().expect("temp directory");
    let database = Libgit2Repository::init_bare(directory.path(), GitObjectFormat::Sha256)
        .expect("init repository");
    run_conformance(&database, GitObjectFormat::Sha256);
}

#[test]
fn custom_sha1_conformance() {
    let directory = tempfile::tempdir().expect("temp directory");
    let storage: Arc<dyn BackendStorage> = Arc::new(InMemoryBackend::new(GitObjectFormat::Sha1));
    let database = Libgit2Backend::init_bare(directory.path(), GitObjectFormat::Sha1, storage)
        .expect("init custom repository");
    run_conformance(&database, GitObjectFormat::Sha1);
}

#[test]
fn custom_sha256_conformance() {
    let directory = tempfile::tempdir().expect("temp directory");
    let storage: Arc<dyn BackendStorage> = Arc::new(InMemoryBackend::new(GitObjectFormat::Sha256));
    let database = Libgit2Backend::init_bare(directory.path(), GitObjectFormat::Sha256, storage)
        .expect("init custom repository");
    run_conformance(&database, GitObjectFormat::Sha256);
}

#[test]
fn compare_and_swap_has_one_winner() {
    let directory = tempfile::tempdir().expect("temp directory");
    let storage: Arc<dyn BackendStorage> = Arc::new(InMemoryBackend::new(GitObjectFormat::Sha1));
    let database = Arc::new(
        Libgit2Backend::init_bare(directory.path(), GitObjectFormat::Sha1, storage)
            .expect("init custom repository"),
    );
    let initial = database
        .write_object(GitObjectKind::Blob, b"initial")
        .expect("write initial");
    let first = database
        .write_object(GitObjectKind::Blob, b"first")
        .expect("write first");
    let second = database
        .write_object(GitObjectKind::Blob, b"second")
        .expect("write second");
    let reference = GitRefName::new("refs/endor/race").expect("valid reference");
    database
        .update_ref_if(&reference, None, &initial, "create")
        .expect("create reference");

    let barrier = Arc::new(Barrier::new(3));
    let handles = [first, second].map(|next| {
        let database = Arc::clone(&database);
        let barrier = Arc::clone(&barrier);
        let reference = reference.clone();
        std::thread::spawn(move || {
            barrier.wait();
            database.update_ref_if(&reference, Some(&initial), &next, "race")
        })
    });
    barrier.wait();
    let results = handles.map(|handle| handle.join().expect("thread did not panic"));
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(GitError::ReferenceConflict(_))))
            .count(),
        1
    );
}

struct PanickingStorage;

impl BackendStorage for PanickingStorage {
    fn object_exists(&self, _id: &GitObjectId) -> Result<bool, GitError> {
        panic!("private panic detail must not unwind through C")
    }

    fn read_object(&self, _id: &GitObjectId) -> Result<GitObject, GitError> {
        panic!("private panic detail must not unwind through C")
    }

    fn write_object(&self, _kind: GitObjectKind, _bytes: &[u8]) -> Result<GitObjectId, GitError> {
        Err(GitError::Unsupported("test write".to_owned()))
    }

    fn object_ids(&self) -> Result<Vec<GitObjectId>, GitError> {
        Ok(Vec::new())
    }

    fn resolve_ref(&self, _name: &GitRefName) -> Result<Option<GitObjectId>, GitError> {
        Ok(None)
    }

    fn references(&self) -> Result<Vec<(GitRefName, GitObjectId)>, GitError> {
        Ok(Vec::new())
    }

    fn update_ref_if(
        &self,
        _name: &GitRefName,
        _expected: Option<&GitObjectId>,
        _next: &GitObjectId,
        _message: &str,
    ) -> Result<(), GitError> {
        Err(GitError::Unsupported("test ref update".to_owned()))
    }
}

#[test]
fn callback_panic_is_converted_to_a_sanitized_error() {
    let directory = tempfile::tempdir().expect("temp directory");
    let storage: Arc<dyn BackendStorage> = Arc::new(PanickingStorage);
    let database = Libgit2Backend::init_bare(directory.path(), GitObjectFormat::Sha1, storage)
        .expect("init custom repository");
    let id = GitObjectId::new(GitObjectFormat::Sha1, &[1; 20]).expect("valid object ID");
    let error = database.object_exists(&id).expect_err("callback must fail");
    assert_eq!(
        error.to_string(),
        "custom libgit2 callback failed: panic in custom backend"
    );
}
