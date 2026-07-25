//! Capability-scoped directory watching.
//!
//! A `DirectoryWatch` owns a clone of an already-authorized `cap_std::fs::Dir`.
//! Its portable backend observes changes by taking successive directory
//! snapshots through that capability. On kqueue platforms, an `EVFILT_VNODE`
//! registration on the directory descriptor wakes the same snapshot-diff
//! engine promptly. Neither backend recovers or resolves an ambient path.

use cap_std::fs::Dir;
use std::collections::BTreeMap;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A directory-entry change synthesized from capability-scoped snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    pub kind: ChangeKind,
    pub name: String,
}

/// The flattened public change kinds shared by every backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Add,
    Remove,
    Replace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EntryIdentity {
    kind: EntryKind,
    length: u64,
    modified: Option<cap_std::time::SystemTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

enum Wakeup {
    Poll,
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    Kqueue(std::os::raw::c_int),
}

/// Watches the immediate children of a directory capability.
pub struct DirectoryWatch {
    directory: Dir,
    snapshot: BTreeMap<String, EntryIdentity>,
    wakeup: Wakeup,
}

/// Start watching an already-authorized directory.
pub fn watch_directory(directory: &Dir) -> io::Result<DirectoryWatch> {
    let directory = directory.try_clone()?;
    let snapshot = snapshot(&directory)?;
    let wakeup = make_wakeup(&directory);
    Ok(DirectoryWatch {
        directory,
        snapshot,
        wakeup,
    })
}

impl DirectoryWatch {
    /// Return changes that occur before `timeout` expires.
    ///
    /// The portable path rescans at most every 50 ms. The kqueue path waits
    /// for a descriptor-anchored vnode notification, then performs the same
    /// diff so all platforms expose identical names and change kinds.
    pub fn poll(&mut self, timeout: Duration) -> io::Result<Vec<Change>> {
        let changes = self.diff()?;
        if !changes.is_empty() || timeout.is_zero() {
            return Ok(changes);
        }

        match &self.wakeup {
            Wakeup::Poll => self.poll_by_snapshot(timeout),
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "dragonfly"
            ))]
            Wakeup::Kqueue(queue) => {
                wait_for_kqueue(*queue, timeout)?;
                self.diff()
            }
        }
    }

    fn poll_by_snapshot(&mut self, timeout: Duration) -> io::Result<Vec<Change>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(Vec::new());
            }
            thread::sleep(remaining.min(POLL_INTERVAL));
            let changes = self.diff()?;
            if !changes.is_empty() || Instant::now() >= deadline {
                return Ok(changes);
            }
        }
    }

    fn diff(&mut self) -> io::Result<Vec<Change>> {
        let next = snapshot(&self.directory)?;
        let mut changes = Vec::new();

        for (name, identity) in &next {
            match self.snapshot.get(name) {
                None => changes.push(Change {
                    kind: ChangeKind::Add,
                    name: name.clone(),
                }),
                Some(previous) if previous != identity => changes.push(Change {
                    kind: ChangeKind::Replace,
                    name: name.clone(),
                }),
                Some(_) => {}
            }
        }

        for name in self.snapshot.keys() {
            if !next.contains_key(name) {
                changes.push(Change {
                    kind: ChangeKind::Remove,
                    name: name.clone(),
                });
            }
        }

        self.snapshot = next;
        Ok(changes)
    }
}

fn snapshot(directory: &Dir) -> io::Result<BTreeMap<String, EntryIdentity>> {
    let mut entries = BTreeMap::new();
    for entry in directory.entries()? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        entries.insert(
            entry.file_name().to_string_lossy().into_owned(),
            EntryIdentity {
                kind,
                length: metadata.len(),
                modified: metadata.modified().ok(),
            },
        );
    }
    Ok(entries)
}

fn make_wakeup(directory: &Dir) -> Wakeup {
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    if let Ok(queue) = make_kqueue_wakeup(directory) {
        return Wakeup::Kqueue(queue);
    }

    let _ = directory;
    Wakeup::Poll
}

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn make_kqueue_wakeup(directory: &Dir) -> io::Result<std::os::raw::c_int> {
    use std::os::unix::io::AsRawFd;

    let queue = unsafe { libc::kqueue() };
    if queue == -1 {
        return Err(io::Error::last_os_error());
    }

    let mut event = libc::kevent {
        ident: directory.as_raw_fd() as libc::uintptr_t,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR,
        fflags: libc::NOTE_WRITE
            | libc::NOTE_EXTEND
            | libc::NOTE_ATTRIB
            | libc::NOTE_LINK
            | libc::NOTE_RENAME
            | libc::NOTE_DELETE
            | libc::NOTE_REVOKE,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let result = unsafe {
        libc::kevent(
            queue,
            &mut event,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    };
    if result == -1 {
        let error = io::Error::last_os_error();
        unsafe {
            libc::close(queue);
        }
        return Err(error);
    }
    Ok(queue)
}

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn wait_for_kqueue(queue: std::os::raw::c_int, timeout: Duration) -> io::Result<()> {
    let timeout = libc::timespec {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_nsec: timeout.subsec_nanos() as libc::c_long,
    };
    let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
    let result =
        unsafe { libc::kevent(queue, std::ptr::null(), 0, event.as_mut_ptr(), 1, &timeout) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        if let Wakeup::Kqueue(queue) = &self.wakeup {
            unsafe {
                libc::close(*queue);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_diff_reports_add_replace_and_remove() {
        let temporary = tempfile::tempdir().unwrap();
        let directory =
            Dir::open_ambient_dir(temporary.path(), cap_std::ambient_authority()).unwrap();
        let mut watch = watch_directory(&directory).unwrap();

        std::fs::write(temporary.path().join("entry"), "one").unwrap();
        assert_eq!(
            watch.poll(Duration::ZERO).unwrap(),
            vec![Change {
                kind: ChangeKind::Add,
                name: "entry".to_string(),
            }]
        );

        std::fs::write(temporary.path().join("entry"), "replacement").unwrap();
        assert_eq!(
            watch.poll(Duration::ZERO).unwrap(),
            vec![Change {
                kind: ChangeKind::Replace,
                name: "entry".to_string(),
            }]
        );

        std::fs::remove_file(temporary.path().join("entry")).unwrap();
        assert_eq!(
            watch.poll(Duration::ZERO).unwrap(),
            vec![Change {
                kind: ChangeKind::Remove,
                name: "entry".to_string(),
            }]
        );
    }
}
