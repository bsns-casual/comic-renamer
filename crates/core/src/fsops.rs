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
use std::path::{Path, PathBuf};

/// `st_flags` bit set by the kernel on a cloud placeholder whose data lives
/// only on the server (D25). Not exported by the `libc` crate.
const SF_DATALESS: u32 = 0x4000_0000;

/// `renamex_np` flag: fail with `EEXIST` rather than replacing the target.
const RENAME_EXCL: libc::c_uint = 0x0000_0004;

fn cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

/// `renamex_np(…, RENAME_EXCL)`. Errors `AlreadyExists` rather than clobbering.
pub fn rename_excl(src: &Path, dst: &Path) -> io::Result<()> {
    let (from, to) = (cstring(src)?, cstring(dst)?);
    // SAFETY: both pointers are valid NUL-terminated C strings that outlive
    // the call, and renamex_np reads them without retaining them.
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), RENAME_EXCL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `clonefile`, whose `EEXIST` is native. Flags `0`.
// shortcut: containment via realpath-per-component; CLONE_RESOLVE_BENEATH
// would move it into the kernel but libc exports no constant
pub fn clone_excl(src: &Path, dst: &Path) -> io::Result<()> {
    let (from, to) = (cstring(src)?, cstring(dst)?);
    // SAFETY: as above; clonefile takes two borrowed C strings and a flag word.
    let rc = unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Temp in the destination directory → `fsync` → compare size and SHA-256 →
/// `rename_excl` into place. **Never unlinks the source** — `apply` owns that,
/// after verification.
/// `dest_root` scopes the containment check. Walking every ancestor instead
/// refuses almost everything: on macOS `/var` is itself a symlink, so any
/// destination under a tempdir — or under plenty of real homes — trips it.
/// The components above the root are the user's own choice of root; only the
/// ones beneath it can let a row escape the tree.
pub fn stream_verified(src: &Path, dst: &Path, dest_root: &Path) -> io::Result<()> {
    let parent = dst
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    refuse_symlinked_components(dest_root, parent)?;

    let temp = parent.join(format!(
        ".comic-renamer.{}.{}.partial",
        std::process::id(),
        dst.file_name().map_or(0, |n| n.len())
    ));

    let result = stream_into(src, &temp).and_then(|()| rename_excl(&temp, dst));
    if result.is_err() {
        // Verified-then-renamed means a failure must leave nothing behind.
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn stream_into(src: &Path, temp: &Path) -> io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    // create_new is O_CREAT|O_EXCL: the temp name is ours or the call fails.
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);

    let (src_len, src_hash) = size_and_hash(src)?;
    let (tmp_len, tmp_hash) = size_and_hash(temp)?;
    if src_len != tmp_len || src_hash != tmp_hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "copy does not match the source",
        ));
    }
    Ok(())
}

/// Streams rather than reading the file in: sources here reach gigabytes.
fn size_and_hash(path: &Path) -> io::Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    use sha2::Digest;
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
    let digest = hasher.finalize();
    Ok((total, sha256_hex_of_digest(&digest)))
}

fn sha256_hex_of_digest(digest: &[u8]) -> String {
    use std::fmt::Write;
    digest.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// §6.11: containment is per component, never a string prefix. A symlinked
/// subdirectory passes `starts_with(dest_root)` and writes outside the tree.
fn refuse_symlinked_components(dest_root: &Path, dir: &Path) -> io::Result<()> {
    let relative = dir.strip_prefix(dest_root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("destination {} is outside {}", dir.display(), dest_root.display()),
        )
    })?;
    let mut current = dest_root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if let Ok(meta) = std::fs::symlink_metadata(&current) {
            if meta.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("destination passes through a symlink: {}", current.display()),
                ));
            }
        }
    }
    Ok(())
}

/// Reads `SF_DATALESS` in `st_flags`, never `st_blocks == 0`, which
/// misclassifies legitimate sparse files (invariant 8, D25).
pub fn is_dataless(path: &Path) -> io::Result<bool> {
    use std::os::macos::fs::MetadataExt as MacMetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    Ok(meta.st_flags() & SF_DATALESS != 0)
}

pub fn select_mechanism(src: &Path, dst: &Path, mode: Mode) -> io::Result<Mechanism> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        // Following one would move the file it points at, which may sit in a
        // library this run was never pointed at.
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source is a symlink: {}", src.display()),
        ));
    }
    let parent = dst
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    let same_volume = std::fs::metadata(parent)?.dev() == meta.dev();

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

/// Groups destination paths that are one path on a case-folding, normalising
/// filesystem (D2). NFC, case-fold and `'` versus `U+2019`.
pub fn collisions(destinations: &[&str]) -> Vec<Collision> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (index, destination) in destinations.iter().enumerate() {
        groups
            .entry(collision_key(destination))
            .or_default()
            .push(index.to_string());
    }
    groups
        .into_iter()
        .filter(|(_, rows)| rows.len() > 1)
        .map(|(key, row_ids)| Collision {
            key,
            row_ids,
            existing: None,
        })
        .collect()
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
        let existing = std::fs::read_dir(&current).ok()?.flatten().find_map(|e| {
            let found = e.file_name().to_string_lossy().into_owned();
            (collision_key(&found) == key).then_some(found)
        })?;
        if existing != name {
            // A case or normalisation variant of this component is already
            // there; the row would merge into it.
            return Some(current.join(existing));
        }
        current = current.join(existing);
    }
    // Every component matched literally, so the whole path is occupied.
    Some(current)
}
