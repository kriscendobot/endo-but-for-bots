//! The only module that contains custom libgit2 callback `unsafe` code.

use std::ffi::{CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::{Arc, Mutex};

use git2::{Binding, Odb, Refdb, Repository};
use libc::{c_char, c_int, c_void, size_t};
use libgit2_sys as raw;

use crate::{BackendStorage, GitError, GitObjectFormat, GitObjectId, GitObjectKind, GitRefName};

pub(crate) struct CallbackState {
    format: GitObjectFormat,
    storage: Arc<dyn BackendStorage>,
    last_error: Mutex<Option<GitError>>,
}

impl CallbackState {
    pub(crate) fn new(format: GitObjectFormat, storage: Arc<dyn BackendStorage>) -> Arc<Self> {
        Arc::new(Self {
            format,
            storage,
            last_error: Mutex::new(None),
        })
    }

    pub(crate) fn take_error(&self) -> Result<Option<GitError>, GitError> {
        self.last_error
            .lock()
            .map(|mut error| error.take())
            .map_err(|_| GitError::Callback("callback error mutex was poisoned".to_owned()))
    }

    fn record_error(&self, error: GitError) {
        if let Ok(mut last_error) = self.last_error.lock() {
            *last_error = Some(error);
        }
    }
}

#[repr(C)]
struct ObjectBackend {
    parent: raw::git_odb_backend,
    state: Arc<CallbackState>,
}

#[repr(C)]
struct ReferenceBackend {
    parent: raw::git_refdb_backend,
    state: Arc<CallbackState>,
}

#[repr(C)]
struct ReferenceIterator {
    parent: raw::git_reference_iterator,
    entries: Vec<(CString, GitObjectId)>,
    position: usize,
}

extern "C" {
    fn git_reference__alloc(
        name: *const c_char,
        id: *const raw::git_oid,
        peel: *const raw::git_oid,
    ) -> *mut raw::git_reference;
}

pub(crate) fn install(repository: &Repository, state: Arc<CallbackState>) -> Result<(), GitError> {
    unsafe {
        let mut object_parent: raw::git_odb_backend = std::mem::zeroed();
        check(raw::git_odb_init_backend(
            &mut object_parent,
            raw::GIT_ODB_BACKEND_VERSION,
        ))?;
        object_parent.read = Some(object_read);
        object_parent.read_prefix = Some(object_read_prefix);
        object_parent.read_header = Some(object_read_header);
        object_parent.write = Some(object_write);
        object_parent.exists = Some(object_exists);
        object_parent.exists_prefix = Some(object_exists_prefix);
        object_parent.foreach = Some(object_foreach);
        object_parent.free = Some(object_free);

        let object_backend = Box::into_raw(Box::new(ObjectBackend {
            parent: object_parent,
            state: Arc::clone(&state),
        }));
        let object_database = Odb::new_ext(state.format.to_git2()).map_err(GitError::from_git2)?;
        let result = raw::git_odb_add_backend(object_database.raw(), object_backend.cast(), 1);
        if result < 0 {
            drop(Box::from_raw(object_backend));
            return Err(GitError::from_git2(git2::Error::last_error(result)));
        }
        repository
            .set_odb(&object_database)
            .map_err(GitError::from_git2)?;

        let mut reference_database_raw = ptr::null_mut();
        check(raw::git_refdb_new(
            &mut reference_database_raw,
            repository.raw(),
        ))?;
        let reference_database: Refdb<'_> = Binding::from_raw(reference_database_raw);

        let mut reference_parent: raw::git_refdb_backend = std::mem::zeroed();
        check(raw::git_refdb_init_backend(
            &mut reference_parent,
            raw::GIT_REFDB_BACKEND_VERSION,
        ))?;
        reference_parent.exists = Some(reference_exists);
        reference_parent.lookup = Some(reference_lookup);
        reference_parent.iterator = Some(reference_iterator);
        reference_parent.write = Some(reference_write);
        reference_parent.rename = Some(reference_rename_unsupported);
        reference_parent.del = Some(reference_delete_unsupported);
        reference_parent.compress = Some(reference_compress);
        reference_parent.has_log = Some(reference_has_log);
        reference_parent.ensure_log = Some(reference_ensure_log);
        reference_parent.free = Some(reference_free);
        reference_parent.reflog_read = Some(reference_reflog_read_unsupported);
        reference_parent.reflog_write = Some(reference_reflog_write_unsupported);
        reference_parent.reflog_rename = Some(reference_reflog_rename_unsupported);
        reference_parent.reflog_delete = Some(reference_reflog_delete_unsupported);

        let reference_backend = Box::into_raw(Box::new(ReferenceBackend {
            parent: reference_parent,
            state,
        }));
        let result = raw::git_refdb_set_backend(reference_database.raw(), reference_backend.cast());
        if result < 0 {
            drop(Box::from_raw(reference_backend));
            return Err(GitError::from_git2(git2::Error::last_error(result)));
        }
        repository
            .set_refdb(&reference_database)
            .map_err(GitError::from_git2)?;
    }
    Ok(())
}

fn check(code: c_int) -> Result<(), GitError> {
    if code < 0 {
        Err(GitError::from_git2(git2::Error::last_error(code)))
    } else {
        Ok(())
    }
}

unsafe fn object_backend<'a>(backend: *mut raw::git_odb_backend) -> &'a ObjectBackend {
    &*backend.cast::<ObjectBackend>()
}

unsafe fn reference_backend<'a>(backend: *mut raw::git_refdb_backend) -> &'a ReferenceBackend {
    &*backend.cast::<ReferenceBackend>()
}

fn callback<F>(state: &CallbackState, call: F) -> c_int
where
    F: FnOnce() -> Result<c_int, GitError>,
{
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => callback_error(state, error),
        Err(_) => callback_error(
            state,
            GitError::Callback("panic in custom backend".to_owned()),
        ),
    }
}

fn callback_error(state: &CallbackState, error: GitError) -> c_int {
    let code = match &error {
        GitError::ObjectNotFound(_) => raw::GIT_ENOTFOUND,
        GitError::ReferenceConflict(_) => raw::GIT_EMODIFIED,
        GitError::Unsupported(_) => raw::GIT_ENOTSUPPORTED,
        _ => raw::GIT_ERROR,
    };
    let message = error.to_string().replace('\0', " ");
    state.record_error(error);
    if let Ok(message) = CString::new(message) {
        unsafe {
            raw::git_error_set_str(raw::GIT_ERROR_ODB as c_int, message.as_ptr());
        }
    }
    code
}

unsafe fn id_from_raw(
    state: &CallbackState,
    id: *const raw::git_oid,
) -> Result<GitObjectId, GitError> {
    if id.is_null() {
        return Err(GitError::InvalidInput("null object ID".to_owned()));
    }
    let id = git2::Oid::from_raw(id);
    let id = GitObjectId::from_git2(id)?;
    if id.format() != state.format {
        return Err(GitError::ObjectFormatMismatch);
    }
    Ok(id)
}

unsafe fn copy_id(out: *mut raw::git_oid, id: &GitObjectId) -> Result<(), GitError> {
    if out.is_null() {
        return Err(GitError::InvalidInput("null object ID output".to_owned()));
    }
    *out = *id.to_git2()?.raw();
    Ok(())
}

fn kind_from_raw(kind: raw::git_object_t) -> Result<GitObjectKind, GitError> {
    if kind == raw::GIT_OBJECT_COMMIT {
        Ok(GitObjectKind::Commit)
    } else if kind == raw::GIT_OBJECT_TREE {
        Ok(GitObjectKind::Tree)
    } else if kind == raw::GIT_OBJECT_BLOB {
        Ok(GitObjectKind::Blob)
    } else if kind == raw::GIT_OBJECT_TAG {
        Ok(GitObjectKind::Tag)
    } else {
        Err(GitError::InvalidInput(format!(
            "unsupported raw Git object kind {}",
            kind
        )))
    }
}

fn kind_to_raw(kind: GitObjectKind) -> raw::git_object_t {
    match kind {
        GitObjectKind::Commit => raw::GIT_OBJECT_COMMIT,
        GitObjectKind::Tree => raw::GIT_OBJECT_TREE,
        GitObjectKind::Blob => raw::GIT_OBJECT_BLOB,
        GitObjectKind::Tag => raw::GIT_OBJECT_TAG,
    }
}

extern "C" fn object_read(
    data: *mut *mut c_void,
    length: *mut size_t,
    kind: *mut raw::git_object_t,
    backend: *mut raw::git_odb_backend,
    id: *const raw::git_oid,
) -> c_int {
    unsafe {
        let backend = object_backend(backend);
        callback(&backend.state, || {
            if data.is_null() || length.is_null() || kind.is_null() {
                return Err(GitError::InvalidInput("null object read output".to_owned()));
            }
            let object = backend
                .state
                .storage
                .read_object(&id_from_raw(&backend.state, id)?)?;
            let allocation = raw::git_odb_backend_data_alloc(
                &backend.parent as *const _ as *mut _,
                object.bytes.len(),
            );
            if allocation.is_null() && !object.bytes.is_empty() {
                return Err(GitError::Callback(
                    "libgit2 object allocation failed".to_owned(),
                ));
            }
            if !object.bytes.is_empty() {
                ptr::copy_nonoverlapping(
                    object.bytes.as_ptr(),
                    allocation.cast(),
                    object.bytes.len(),
                );
            }
            *data = allocation;
            *length = object.bytes.len();
            *kind = kind_to_raw(object.kind);
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn object_read_header(
    length: *mut size_t,
    kind: *mut raw::git_object_t,
    backend: *mut raw::git_odb_backend,
    id: *const raw::git_oid,
) -> c_int {
    unsafe {
        let backend = object_backend(backend);
        callback(&backend.state, || {
            if length.is_null() || kind.is_null() {
                return Err(GitError::InvalidInput(
                    "null object header output".to_owned(),
                ));
            }
            let object = backend
                .state
                .storage
                .read_object(&id_from_raw(&backend.state, id)?)?;
            *length = object.bytes.len();
            *kind = kind_to_raw(object.kind);
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn object_write(
    backend: *mut raw::git_odb_backend,
    id: *const raw::git_oid,
    data: *const c_void,
    length: size_t,
    kind: raw::git_object_t,
) -> c_int {
    unsafe {
        let backend = object_backend(backend);
        callback(&backend.state, || {
            if data.is_null() && length != 0 {
                return Err(GitError::InvalidInput("null object data".to_owned()));
            }
            let expected = id_from_raw(&backend.state, id)?;
            let bytes = if length == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(data.cast::<u8>(), length)
            };
            let actual = backend
                .state
                .storage
                .write_object(kind_from_raw(kind)?, bytes)?;
            if actual != expected {
                return Err(GitError::Callback(
                    "custom object store returned a different object ID".to_owned(),
                ));
            }
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn object_exists(backend: *mut raw::git_odb_backend, id: *const raw::git_oid) -> c_int {
    unsafe {
        let backend = object_backend(backend);
        callback(&backend.state, || {
            Ok(i32::from(
                backend
                    .state
                    .storage
                    .object_exists(&id_from_raw(&backend.state, id)?)?,
            ))
        })
    }
}

fn prefix_matches(
    candidate: &GitObjectId,
    prefix: &GitObjectId,
    hexadecimal_length: usize,
) -> bool {
    let whole_bytes = hexadecimal_length / 2;
    if candidate.as_bytes()[..whole_bytes] != prefix.as_bytes()[..whole_bytes] {
        return false;
    }
    if hexadecimal_length.is_multiple_of(2) {
        return true;
    }
    candidate.as_bytes()[whole_bytes] & 0xf0 == prefix.as_bytes()[whole_bytes] & 0xf0
}

unsafe fn find_prefix(
    state: &CallbackState,
    prefix: *const raw::git_oid,
    hexadecimal_length: usize,
) -> Result<GitObjectId, GitError> {
    let prefix = id_from_raw(state, prefix)?;
    let mut matches = state
        .storage
        .object_ids()?
        .into_iter()
        .filter(|candidate| prefix_matches(candidate, &prefix, hexadecimal_length));
    let result = matches
        .next()
        .ok_or_else(|| GitError::ObjectNotFound(prefix.to_string()))?;
    if matches.next().is_some() {
        return Err(GitError::Callback("ambiguous Git object prefix".to_owned()));
    }
    Ok(result)
}

extern "C" fn object_read_prefix(
    out_id: *mut raw::git_oid,
    data: *mut *mut c_void,
    length: *mut size_t,
    kind: *mut raw::git_object_t,
    backend: *mut raw::git_odb_backend,
    prefix: *const raw::git_oid,
    hexadecimal_length: size_t,
) -> c_int {
    unsafe {
        let backend = object_backend(backend);
        callback(&backend.state, || {
            let id = find_prefix(&backend.state, prefix, hexadecimal_length)?;
            copy_id(out_id, &id)?;
            let raw_id = id.to_git2()?;
            let code = object_read(
                data,
                length,
                kind,
                &backend.parent as *const _ as *mut _,
                raw_id.raw(),
            );
            if code < 0 {
                return Ok(code);
            }
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn object_exists_prefix(
    out_id: *mut raw::git_oid,
    backend: *mut raw::git_odb_backend,
    prefix: *const raw::git_oid,
    hexadecimal_length: size_t,
) -> c_int {
    unsafe {
        let backend = object_backend(backend);
        callback(&backend.state, || {
            let id = find_prefix(&backend.state, prefix, hexadecimal_length)?;
            copy_id(out_id, &id)?;
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn object_foreach(
    backend: *mut raw::git_odb_backend,
    visit: raw::git_odb_foreach_cb,
    payload: *mut c_void,
) -> c_int {
    unsafe {
        let backend = object_backend(backend);
        callback(&backend.state, || {
            let visit =
                visit.ok_or_else(|| GitError::InvalidInput("null object visitor".to_owned()))?;
            for id in backend.state.storage.object_ids()? {
                let raw_id = id.to_git2()?;
                let code = visit(raw_id.raw(), payload);
                if code != 0 {
                    return Ok(code);
                }
            }
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn object_free(backend: *mut raw::git_odb_backend) {
    if !backend.is_null() {
        unsafe {
            drop(Box::from_raw(backend.cast::<ObjectBackend>()));
        }
    }
}

unsafe fn reference_name(name: *const c_char) -> Result<GitRefName, GitError> {
    if name.is_null() {
        return Err(GitError::InvalidInput("null reference name".to_owned()));
    }
    let name = CStr::from_ptr(name)
        .to_str()
        .map_err(|_| GitError::InvalidInput("reference name is not UTF-8".to_owned()))?;
    GitRefName::new(name)
}

extern "C" fn reference_exists(
    out: *mut c_int,
    backend: *mut raw::git_refdb_backend,
    name: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback(&backend.state, || {
            if out.is_null() {
                return Err(GitError::InvalidInput(
                    "null reference exists output".to_owned(),
                ));
            }
            *out = i32::from(
                backend
                    .state
                    .storage
                    .resolve_ref(&reference_name(name)?)?
                    .is_some(),
            );
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn reference_lookup(
    out: *mut *mut raw::git_reference,
    backend: *mut raw::git_refdb_backend,
    name: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback(&backend.state, || {
            if out.is_null() {
                return Err(GitError::InvalidInput(
                    "null reference lookup output".to_owned(),
                ));
            }
            let name = reference_name(name)?;
            let id = backend
                .state
                .storage
                .resolve_ref(&name)?
                .ok_or_else(|| GitError::ObjectNotFound(name.as_str().to_owned()))?;
            let name = CString::new(name.as_str())
                .map_err(|error| GitError::InvalidInput(error.to_string()))?;
            let raw_id = id.to_git2()?;
            *out = git_reference__alloc(name.as_ptr(), raw_id.raw(), ptr::null());
            if (*out).is_null() {
                return Err(GitError::Callback(
                    "libgit2 reference allocation failed".to_owned(),
                ));
            }
            Ok(raw::GIT_OK)
        })
    }
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut matches = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    matches[0][0] = true;
    for pattern_index in 0..pattern.len() {
        for value_index in 0..=value.len() {
            if !matches[pattern_index][value_index] {
                continue;
            }
            match pattern[pattern_index] {
                b'*' => {
                    matches[pattern_index + 1][value_index] = true;
                    if value_index < value.len() {
                        matches[pattern_index][value_index + 1] = true;
                    }
                }
                b'?' if value_index < value.len() => {
                    matches[pattern_index + 1][value_index + 1] = true;
                }
                byte if value_index < value.len() && byte == value[value_index] => {
                    matches[pattern_index + 1][value_index + 1] = true;
                }
                _ => {}
            }
        }
    }
    matches[pattern.len()][value.len()]
}

extern "C" fn reference_iterator(
    out: *mut *mut raw::git_reference_iterator,
    backend: *mut raw::git_refdb_backend,
    glob: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback(&backend.state, || {
            if out.is_null() {
                return Err(GitError::InvalidInput(
                    "null reference iterator output".to_owned(),
                ));
            }
            let glob = if glob.is_null() {
                None
            } else {
                Some(CStr::from_ptr(glob).to_str().map_err(|_| {
                    GitError::InvalidInput("reference glob is not UTF-8".to_owned())
                })?)
            };
            let entries = backend
                .state
                .storage
                .references()?
                .into_iter()
                .filter(|(name, _)| glob.is_none_or(|glob| wildcard_matches(glob, name.as_str())))
                .map(|(name, id)| {
                    CString::new(name.as_str())
                        .map(|name| (name, id))
                        .map_err(|error| GitError::InvalidInput(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let iterator = Box::new(ReferenceIterator {
                parent: raw::git_reference_iterator {
                    db: ptr::null_mut(),
                    next: Some(reference_iterator_next),
                    next_name: Some(reference_iterator_next_name),
                    free: Some(reference_iterator_free),
                },
                entries,
                position: 0,
            });
            *out = Box::into_raw(iterator).cast();
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn reference_iterator_next(
    out: *mut *mut raw::git_reference,
    iterator: *mut raw::git_reference_iterator,
) -> c_int {
    unsafe {
        if out.is_null() || iterator.is_null() {
            return raw::GIT_ERROR;
        }
        let iterator = &mut *iterator.cast::<ReferenceIterator>();
        let Some((name, id)) = iterator.entries.get(iterator.position) else {
            return raw::GIT_ITEROVER;
        };
        iterator.position += 1;
        let raw_id = match id.to_git2() {
            Ok(id) => id,
            Err(_) => return raw::GIT_ERROR,
        };
        *out = git_reference__alloc(name.as_ptr(), raw_id.raw(), ptr::null());
        if (*out).is_null() {
            raw::GIT_ERROR
        } else {
            raw::GIT_OK
        }
    }
}

extern "C" fn reference_iterator_next_name(
    out: *mut *const c_char,
    iterator: *mut raw::git_reference_iterator,
) -> c_int {
    unsafe {
        if out.is_null() || iterator.is_null() {
            return raw::GIT_ERROR;
        }
        let iterator = &mut *iterator.cast::<ReferenceIterator>();
        let Some((name, _)) = iterator.entries.get(iterator.position) else {
            return raw::GIT_ITEROVER;
        };
        iterator.position += 1;
        *out = name.as_ptr();
        raw::GIT_OK
    }
}

extern "C" fn reference_iterator_free(iterator: *mut raw::git_reference_iterator) {
    if !iterator.is_null() {
        unsafe {
            drop(Box::from_raw(iterator.cast::<ReferenceIterator>()));
        }
    }
}

extern "C" fn reference_write(
    backend: *mut raw::git_refdb_backend,
    reference: *const raw::git_reference,
    force: c_int,
    _signature: *const raw::git_signature,
    message: *const c_char,
    old: *const raw::git_oid,
    old_target: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback(&backend.state, || {
            if reference.is_null() {
                return Err(GitError::InvalidInput("null reference".to_owned()));
            }
            if !old_target.is_null()
                || raw::git_reference_type(reference) != raw::GIT_REFERENCE_DIRECT
            {
                return Err(GitError::SymbolicReference("custom backend".to_owned()));
            }
            let name = reference_name(raw::git_reference_name(reference))?;
            let target = raw::git_reference_target(reference);
            let target = id_from_raw(&backend.state, target)?;
            let expected = if force != 0 {
                backend.state.storage.resolve_ref(&name)?
            } else if old.is_null() {
                None
            } else {
                Some(id_from_raw(&backend.state, old)?)
            };
            let message = if message.is_null() {
                ""
            } else {
                CStr::from_ptr(message).to_str().unwrap_or("")
            };
            backend
                .state
                .storage
                .update_ref_if(&name, expected.as_ref(), &target, message)?;
            Ok(raw::GIT_OK)
        })
    }
}

extern "C" fn reference_rename_unsupported(
    _out: *mut *mut raw::git_reference,
    backend: *mut raw::git_refdb_backend,
    _old_name: *const c_char,
    _new_name: *const c_char,
    _force: c_int,
    _signature: *const raw::git_signature,
    _message: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback_error(
            &backend.state,
            GitError::Unsupported("reference rename".to_owned()),
        )
    }
}

extern "C" fn reference_delete_unsupported(
    backend: *mut raw::git_refdb_backend,
    _name: *const c_char,
    _old: *const raw::git_oid,
    _old_target: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback_error(
            &backend.state,
            GitError::Unsupported("reference deletion".to_owned()),
        )
    }
}

extern "C" fn reference_compress(_backend: *mut raw::git_refdb_backend) -> c_int {
    raw::GIT_OK
}

extern "C" fn reference_has_log(
    _backend: *mut raw::git_refdb_backend,
    _name: *const c_char,
) -> c_int {
    0
}

extern "C" fn reference_ensure_log(
    _backend: *mut raw::git_refdb_backend,
    _name: *const c_char,
) -> c_int {
    raw::GIT_OK
}

extern "C" fn reference_reflog_read_unsupported(
    _out: *mut *mut raw::git_reflog,
    backend: *mut raw::git_refdb_backend,
    _name: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback_error(
            &backend.state,
            GitError::Unsupported("reference log read".to_owned()),
        )
    }
}

extern "C" fn reference_reflog_write_unsupported(
    backend: *mut raw::git_refdb_backend,
    _reflog: *mut raw::git_reflog,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback_error(
            &backend.state,
            GitError::Unsupported("reference log write".to_owned()),
        )
    }
}

extern "C" fn reference_reflog_rename_unsupported(
    backend: *mut raw::git_refdb_backend,
    _old_name: *const c_char,
    _new_name: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback_error(
            &backend.state,
            GitError::Unsupported("reference log rename".to_owned()),
        )
    }
}

extern "C" fn reference_reflog_delete_unsupported(
    backend: *mut raw::git_refdb_backend,
    _name: *const c_char,
) -> c_int {
    unsafe {
        let backend = reference_backend(backend);
        callback_error(
            &backend.state,
            GitError::Unsupported("reference log deletion".to_owned()),
        )
    }
}

extern "C" fn reference_free(backend: *mut raw::git_refdb_backend) {
    if !backend.is_null() {
        unsafe {
            drop(Box::from_raw(backend.cast::<ReferenceBackend>()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wildcard_matches;

    #[test]
    fn reference_globs_match_without_crossing_ffi() {
        assert!(wildcard_matches("refs/endor/*", "refs/endor/main"));
        assert!(!wildcard_matches("refs/heads/*", "refs/endor/main"));
    }
}
