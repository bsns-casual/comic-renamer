//! No-clobber is enforced at the syscall, never by a prior check (invariant 6).
//! `rename(2)` overwrites silently; a check-then-rename is a data-loss bug.

use crate::normalize::collision_key;
use crate::preset::Preset;
use crate::types::{Collision, Mechanism, Mode, PlanRow};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// `st_flags` bit set by the kernel on a cloud placeholder whose data lives
/// only on the server (D25). Not exported by the `libc` crate.
const SF_DATALESS: u32 = 0x4000_0000;

/// `renamex_np` flag: fail with `EEXIST` rather than replacing the target.
const RENAME_EXCL: libc::c_uint = 0x0000_0004;

pub(crate) fn cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

/// `clonefile` flag: refuse to follow a symbolic link anywhere in the path.
/// §6.11 says to prefer the kernel's own containment over our path arithmetic.
/// `libc` does not export it; the value is from `<sys/clonefile.h>`.
const CLONE_NOFOLLOW_ANY: u32 = 0x0008;

/// A verified destination: a directory handle that is provably reached from
/// `dest_root` without traversing a symlink, plus the leaf name within it.
///
/// Holding the handle rather than the path is what closes the race. Once the
/// walk has succeeded, swapping a component for a symlink cannot redirect a
/// write through this fd.
struct Destination {
    parent: std::os::fd::OwnedFd,
    leaf: CString,
}

/// §6.11: containment per component, never a string prefix. A symlinked
/// subdirectory passes `starts_with(dest_root)` and writes outside the tree.
///
/// Components *above* `dest_root` are followed: the root is the user's own
/// choice, and on macOS `/var` is itself a symlink, so refusing those refuses
/// almost every real destination. Components *beneath* it are opened
/// `O_NOFOLLOW`, so a symlinked series folder fails with `ELOOP`.
fn resolve_destination(dest_root: &Path, relative: &Path) -> io::Result<Destination> {
    use std::os::fd::{FromRawFd, OwnedFd};

    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => components.push(name),
            // `strip_prefix` and `components()` do not resolve `..`, so a
            // lexical containment check accepts `root/../sibling`.
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("destination {} is not a plain relative path", relative.display()),
                ))
            }
        }
    }
    let Some((leaf, directories)) = components.split_last() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination is empty",
        ));
    };

    let root = cstring(dest_root)?;
    // SAFETY: `root` is a valid NUL-terminated path for the duration of the
    // call; the returned descriptor is immediately adopted by OwnedFd.
    let raw = unsafe { libc::open(root.as_ptr(), libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut current = unsafe { OwnedFd::from_raw_fd(raw) };

    for name in directories {
        let component = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "component holds a NUL"))?;
        // SAFETY: `current` is an open directory descriptor we own, and
        // `component` outlives the call.
        let raw = unsafe {
            libc::openat(
                std::os::fd::AsRawFd::as_raw_fd(&current),
                component.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        current = unsafe { OwnedFd::from_raw_fd(raw) };
    }

    Ok(Destination {
        parent: current,
        leaf: CString::new(leaf.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "leaf holds a NUL"))?,
    })
}

/// A source must never be a symlink: following one moves the file it points
/// at, which may sit in a library this run was never aimed at.
///
/// Checked inside every primitive, not in the selector. Nothing in the types
/// enforces a call order, and the clone-to-stream fallback is a second door.
fn refuse_symlink_source(src: &Path) -> io::Result<std::fs::Metadata> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source is a symlink: {}", src.display()),
        ));
    }
    Ok(meta)
}

/// Is anything at all sitting at this leaf, symlink included?
fn occupied(destination: &Destination) -> bool {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: both the descriptor and the name are valid for the call, and
    // `stat` is only read when fstatat reports success.
    let rc = unsafe {
        libc::fstatat(
            std::os::fd::AsRawFd::as_raw_fd(&destination.parent),
            destination.leaf.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    rc == 0
}

/// `renameatx_np(…, RENAME_EXCL)`. Errors `AlreadyExists` rather than clobbering.
pub fn rename_excl(src: &Path, dest_root: &Path, relative: &Path) -> io::Result<()> {
    refuse_symlink_source(src)?;
    let destination = resolve_destination(dest_root, relative)?;
    let from = cstring(src)?;
    // SAFETY: both paths are valid NUL-terminated C strings that outlive the
    // call, and `parent` is an open directory descriptor we own.
    let rc = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            from.as_ptr(),
            std::os::fd::AsRawFd::as_raw_fd(&destination.parent),
            destination.leaf.as_ptr(),
            RENAME_EXCL,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A destination resolved **once**, held open, so a caller can stat it and
/// then act on it through the same directory descriptor.
///
/// `stat_at` followed by `remove_at` resolves twice, and a component can be
/// swapped between the two walks — the check and the act would then see
/// different directories. macOS has no `funlinkat`, so the name is still
/// re-looked-up inside the parent at unlink time; holding the parent is what
/// closes the part that can be closed.
pub struct Entry {
    destination: Destination,
}

/// One walk. Every subsequent operation goes through the descriptor it holds.
pub fn resolve_at(root: &Path, relative: &Path) -> io::Result<Entry> {
    Ok(Entry {
        destination: resolve_destination(root, relative)?,
    })
}

impl Entry {
    /// `(size, inode, mtime_ns)` of the leaf, without following a symlink.
    ///
    /// `fstatat`, not `openat`. Opening the file to stat it blocks forever on
    /// a FIFO — and `reverse` holds the plan lock while it hangs, so the one
    /// recovery path for the library becomes unavailable — and it fails
    /// outright on a file the user cannot read, which a stat should answer.
    pub fn identity(&self) -> io::Result<(u64, u64, i64)> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `parent` is an open directory we own, `leaf` outlives the
        // call, and `stat` is only read once fstatat reports success.
        let rc = unsafe {
            libc::fstatat(
                std::os::fd::AsRawFd::as_raw_fd(&self.destination.parent),
                self.destination.leaf.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstatat returned 0, so the struct is initialised.
        let stat = unsafe { stat.assume_init() };
        Ok((
            stat.st_size.max(0) as u64,
            stat.st_ino,
            stat.st_mtime as i64 * 1_000_000_000 + stat.st_mtime_nsec as i64,
        ))
    }

    pub fn remove(&self) -> io::Result<()> {
        // SAFETY: as above.
        let rc = unsafe {
            libc::unlinkat(
                std::os::fd::AsRawFd::as_raw_fd(&self.destination.parent),
                self.destination.leaf.as_ptr(),
                0,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Renames out of this entry's own directory, no-clobber.
    pub fn rename_into(&self, to_root: &Path, to_relative: &Path) -> io::Result<()> {
        let to = resolve_destination(to_root, to_relative)?;
        // SAFETY: two open directory descriptors we own, two names that
        // outlive the call.
        let rc = unsafe {
            libc::renameatx_np(
                std::os::fd::AsRawFd::as_raw_fd(&self.destination.parent),
                self.destination.leaf.as_ptr(),
                std::os::fd::AsRawFd::as_raw_fd(&to.parent),
                to.leaf.as_ptr(),
                RENAME_EXCL,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// Convenience for a one-shot identity read where nothing destructive follows.
pub fn stat_at(root: &Path, relative: &Path) -> io::Result<(u64, u64, i64)> {
    resolve_at(root, relative)?.identity()
}

/// `renameatx_np` with a walked descriptor on **both** sides.
///
/// `rename_excl` resolves only its destination and passes `AT_FDCWD` for the
/// source, which is adequate when the caller has already established the
/// source's containment. `undo` has not, so it uses this.
pub fn rename_excl_at(
    from_root: &Path,
    from_relative: &Path,
    to_root: &Path,
    to_relative: &Path,
) -> io::Result<()> {
    let from = resolve_destination(from_root, from_relative)?;
    let to = resolve_destination(to_root, to_relative)?;
    // SAFETY: two open directory descriptors we own, two names that outlive
    // the call.
    let rc = unsafe {
        libc::renameatx_np(
            std::os::fd::AsRawFd::as_raw_fd(&from.parent),
            from.leaf.as_ptr(),
            std::os::fd::AsRawFd::as_raw_fd(&to.parent),
            to.leaf.as_ptr(),
            RENAME_EXCL,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Creates the root itself when it is absent.
///
/// `ensure_parent` pops the leaf, so for a single-component relative path it
/// has nothing to do — and `undo`'s restore target is exactly that. The
/// common case is a user who deleted the emptied bundle folder.
pub fn ensure_root(root: &Path) -> io::Result<()> {
    if root.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(root)
}

/// Removes now-empty directories from `relative`'s parent up to `root`,
/// deepest first. `rmdir` refuses a non-empty directory, so this cannot take
/// one that still holds books.
pub fn prune_empty_dirs(root: &Path, relative: &Path) -> io::Result<()> {
    let mut components: Vec<_> = relative.components().collect();
    components.pop();
    while !components.is_empty() {
        let partial: PathBuf = components.iter().collect();
        let Ok(destination) = resolve_destination(root, &partial) else {
            return Ok(());
        };
        // SAFETY: an open directory we own and a name that outlives the call.
        let rc = unsafe {
            libc::unlinkat(
                std::os::fd::AsRawFd::as_raw_fd(&destination.parent),
                destination.leaf.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        };
        if rc != 0 {
            // ENOTEMPTY and anything else: stop, leave the rest alone.
            return Ok(());
        }
        components.pop();
    }
    Ok(())
}

/// `clonefile`, whose `EEXIST` is native, with `CLONE_NOFOLLOW_ANY` so the
/// kernel refuses a symlink anywhere in the path.
///
/// Without that flag `clonefile` follows a dangling symlink at the
/// destination, creates the target and returns `Ok` — the one mechanism of
/// the three for which no-clobber did not hold.
pub fn clone_excl(src: &Path, dest_root: &Path, relative: &Path) -> io::Result<()> {
    refuse_symlink_source(src)?;
    let destination = resolve_destination(dest_root, relative)?;
    if occupied(&destination) {
        // Additive, not a substitute: `clonefileat` still enforces EEXIST for
        // a regular file. This closes the symlink case the syscall does not.
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", relative.display()),
        ));
    }
    // `clonefileat` would walk the absolute source path, and
    // CLONE_NOFOLLOW_ANY then trips on `/var`, which is a symlink on macOS.
    // `fclonefileat` takes a source descriptor, so the flag constrains only
    // the destination — which is a single leaf under a verified directory.
    let source = open_nofollow(src)?;
    // SAFETY: `source` is an open descriptor we own, `parent` likewise, and
    // `leaf` outlives the call.
    let rc = unsafe {
        libc::fclonefileat(
            std::os::fd::AsRawFd::as_raw_fd(&source),
            std::os::fd::AsRawFd::as_raw_fd(&destination.parent),
            destination.leaf.as_ptr(),
            CLONE_NOFOLLOW_ANY,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Opens a file refusing to follow a final symlink. Paired with
/// `refuse_symlink_source` so the check and the open cannot disagree.
fn open_nofollow(path: &Path) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let name = cstring(path)?;
    // SAFETY: `name` is a valid NUL-terminated path for the call; the
    // descriptor is adopted immediately.
    let raw = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) })
}

/// Temp in the destination directory → `fsync` → compare size and SHA-256 →
/// rename into place. **Never unlinks the source** — `apply` owns that, after
/// verification.
pub fn stream_verified(src: &Path, dest_root: &Path, relative: &Path) -> io::Result<()> {
    refuse_symlink_source(src)?;
    let destination = resolve_destination(dest_root, relative)?;

    // Occupancy first. Copying before this costs a full read and write of the
    // payload — 31.2 GiB on the reference bundle — to reach a refusal that
    // merging into an existing library produces every time.
    if occupied(&destination) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", relative.display()),
        ));
    }

    let (temp, mut file) = Temp::create(&destination)?;

    let mut input = std::fs::File::open(src)?;
    io::copy(&mut input, &mut file)?;
    file.sync_all()?;


    // Hashed through the descriptor the copy was read from, not by
    // re-opening the path: otherwise the verify reads the payload twice and
    // confirms a name rather than the bytes that were written.
    let (src_len, src_hash) = hash_open_file(&mut input)?;
    let (tmp_len, tmp_hash) = hash_open_file(&mut file)?;
    if src_len != tmp_len || src_hash != tmp_hash {
        // `temp` unlinks itself on drop.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "copy does not match the source",
        ));
    }
    drop(file);

    let parent = std::os::fd::AsRawFd::as_raw_fd(&destination.parent);
    // SAFETY: one open directory descriptor, two names that outlive the call.
    let rc = unsafe {
        libc::renameatx_np(
            parent,
            temp.name.as_ptr(),
            parent,
            destination.leaf.as_ptr(),
            RENAME_EXCL,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    temp.keep();
    Ok(())
}

/// A temp file this code created, unlinked on drop unless it was renamed away.
///
/// Everything goes through the parent descriptor. An earlier version derived
/// the temp name from the destination filename's *length* and cleaned up
/// unconditionally, so a crafted destination made the error path delete a file
/// the function had never created.
struct Temp {
    parent: std::os::fd::RawFd,
    name: CString,
    armed: bool,
}

impl Temp {
    fn create(destination: &Destination) -> io::Result<(Temp, std::fs::File)> {
        use std::os::fd::FromRawFd;
        // Unique per process and per call, so two rows in one directory never
        // fight over a name — including under the parallel apply that the
        // single-threaded shortcut defers.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = CString::new(format!(
            ".comic-renamer.{}.{seq}.partial",
            std::process::id()
        ))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "temp name holds a NUL"))?;

        let parent = std::os::fd::AsRawFd::as_raw_fd(&destination.parent);
        // O_EXCL: the name is ours or this fails. O_NOFOLLOW: never write
        // through a planted link. O_RDWR, not O_WRONLY, because the verify
        // step reads back through this same descriptor.
        // SAFETY: `parent` is open and `name` outlives the call.
        let raw = unsafe {
            libc::openat(
                parent,
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { std::fs::File::from_raw_fd(raw) };
        Ok((
            Temp {
                parent,
                name,
                armed: true,
            },
            file,
        ))
    }

    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        if self.armed {
            // SAFETY: `parent` is still open for this scope and `name` is valid.
            unsafe { libc::unlinkat(self.parent, self.name.as_ptr(), 0) };
        }
    }
}

/// Hashes what was actually written, through the descriptor we wrote with,
/// so no path is re-resolved between writing and verifying.
fn hash_open_file(file: &mut std::fs::File) -> io::Result<(u64, String)> {
    use sha2::Digest;
    use std::io::Seek;
    file.rewind()?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut hasher = sha2::Sha256::new();
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((total, hex(&hasher.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Reads `SF_DATALESS` in `st_flags`, never `st_blocks == 0`, which
/// misclassifies legitimate sparse files (invariant 8, D25).
pub fn is_dataless(path: &Path) -> io::Result<bool> {
    use std::os::macos::fs::MetadataExt as MacMetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    Ok(meta.st_flags() & SF_DATALESS != 0)
}

pub fn select_mechanism(src: &Path, dest_root: &Path, mode: Mode) -> io::Result<Mechanism> {
    let meta = refuse_symlink_source(src)?;
    // Stat the root, not the destination's parent: the series folder does not
    // exist yet on a fresh library, and stat'ing it returned NotFound for
    // every row — forcing apply to mkdir before it could refuse anything.
    let same_volume = std::fs::metadata(dest_root)?.dev() == meta.dev();

    Ok(match (mode, same_volume) {
        // Atomic, zero space, and it cannot truncate a multi-gigabyte PDF.
        (Mode::Move, true) => Mechanism::Rename,
        (Mode::Move, false) => Mechanism::Stream,
        // shortcut: clonefile is APFS and same-volume only. This returns
        // Clone and apply falls back to Stream on error — that *is* the
        // runtime probe §6.10 asks for, rather than assuming APFS.
        (Mode::Copy, true) => Mechanism::Clone,
        (Mode::Copy, false) => Mechanism::Stream,
    })
}

/// **Recomputes** every destination through the preset — comparing
/// `destination_preview` would let a hand-edited plan defeat detection.
pub fn find_collisions(rows: &[PlanRow], dest_root: &Path, preset: &dyn Preset) -> Vec<Collision> {
    let destinations: Vec<(String, PathBuf)> = rows
        .iter()
        .map(|row| {
            (
                row.id.clone(),
                preset.destination(&row.resolved(), &row.extension),
            )
        })
        .collect();

    let mut found = Vec::new();

    // Two rows landing on one path.
    let mut by_path: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, destination) in &destinations {
        by_path
            .entry(collision_key(&destination.to_string_lossy()))
            .or_default()
            .push(id.clone());
    }
    found.extend(
        by_path
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(key, row_ids)| Collision {
                key,
                row_ids,
                existing: None,
            }),
    );

    // Two rows whose series folders are one directory on a folding volume and
    // two on a case-sensitive one (invariant 7). Grouping on the whole path
    // misses these, because the leaves differ.
    let mut by_folder: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for (id, destination) in &destinations {
        if let Some(first) = destination.components().next() {
            let literal = first.as_os_str().to_string_lossy().into_owned();
            by_folder
                .entry(collision_key(&literal))
                .or_default()
                .entry(literal)
                .or_default()
                .push(id.clone());
        }
    }
    found.extend(
        by_folder
            .into_iter()
            // One key, several spellings: the rows disagree about the folder.
            .filter(|(_, spellings)| spellings.len() > 1)
            .map(|(key, spellings)| Collision {
                key,
                row_ids: spellings.into_values().flatten().collect(),
                existing: None,
            }),
    );

    // Two rows over one underlying file. Under `--move` the first executes and
    // the second is left pointing at nothing.
    let mut by_inode: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for row in rows {
        by_inode
            .entry(row.source_stat.inode)
            .or_default()
            .push(row.id.clone());
    }
    found.extend(
        by_inode
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(inode, row_ids)| Collision {
                key: format!("inode:{inode}"),
                row_ids,
                existing: None,
            }),
    );

    // Something already at the destination. Merging into a library that holds
    // `Hellboy/` is the common case, not the rare one (§6.10).
    for (id, destination) in &destinations {
        if let Some(existing) = existing_conflict(dest_root, destination) {
            found.push(Collision {
                key: collision_key(&destination.to_string_lossy()),
                row_ids: vec![id.clone()],
                existing: Some(existing),
            });
        }
    }

    found
}

/// Walks the destination a component at a time, comparing by collision key
/// rather than by literal name: `Hellboy/` and `hellboy/` are one directory on
/// APFS and two series on a Linux server.
fn existing_conflict(dest_root: &Path, relative: &Path) -> Option<PathBuf> {
    let mut current = dest_root.to_path_buf();
    for component in relative.components() {
        let name = component.as_os_str().to_string_lossy().into_owned();
        let key = collision_key(&name);

        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            // Nothing is here, so nothing can conflict.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            // Unreadable, or not a directory at all — a plain file named
            // `Hellboy` sitting where a series folder belongs. Treating that
            // as "no conflict" makes the plan promise a row that `apply`
            // cannot execute.
            Err(_) => return Some(current),
        };

        // Every match, not the first: with both `Hellboy/` and `hellboy/`
        // present, descending into whichever `read_dir` yielded first could
        // miss the book under the other spelling, nondeterministically.
        let matches: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|found| collision_key(found) == key)
            .collect();

        match matches.as_slice() {
            [] => return None,
            [only] if *only == name => current = current.join(only),
            // A case or normalisation variant is already there, or the
            // destination holds several spellings of one name at once.
            _ => return Some(current.join(&matches[0])),
        }
    }
    // Every component matched literally, so the whole path is occupied.
    Some(current)
}

/// Creates the directories a destination needs, one component at a time,
/// refusing to traverse a symlink — the same discipline as
/// `resolve_destination`, which cannot create anything.
///
/// Called per row at execution, never during pre-flight: a run that refuses
/// every row should not leave a tree of empty folders behind.
pub fn ensure_parent(dest_root: &Path, relative: &Path) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let mut components: Vec<_> = relative.components().collect();
    components.pop(); // the leaf is a file, not a directory
    if components.is_empty() {
        return Ok(());
    }

    let root = cstring(dest_root)?;
    // SAFETY: `root` is valid for the call and the descriptor is adopted.
    let raw = unsafe { libc::open(root.as_ptr(), libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut current = unsafe { OwnedFd::from_raw_fd(raw) };

    for component in components {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a plain relative path", relative.display()),
            ));
        };
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "component holds a NUL"))?;

        // SAFETY: `current` is an open directory we own; `name` outlives both
        // calls. EEXIST is expected and handled by the openat that follows.
        unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o755) };
        let raw = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        current = unsafe { OwnedFd::from_raw_fd(raw) };
    }
    Ok(())
}
