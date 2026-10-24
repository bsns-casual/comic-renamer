//! Where a run's undo record lives, and whether one is still being written.
//!
//! A record is held under `flock` for as long as its run writes it, so a
//! crashed run releases it by dying and a live one cannot be mistaken for
//! stale.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A record's basename: `.comic-renamer.<unix-secs>.<pid>.<seq>.record`. The
/// pid and per-process counter keep two runs in the same second apart.
pub fn basename(now: SystemTime) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!(".comic-renamer.{secs}.{}.{seq}.record", std::process::id())
}

/// The lock filename for a library: a hash of its canonical root, so a root
/// that is a long or odd path is still one filename. Canonicalisation failure
/// is an error, never a fallback that could give one library two locks.
pub fn lock_name(dest_root: &Path) -> Result<String, io::Error> {
    // Canonicalisation is the whole normalisation: macOS returns the on-disk
    // form, so every spelling of one directory (case, NFC/NFD, symlinks)
    // collapses to one string. Case is deliberately not folded: on a
    // case-sensitive volume `Hellboy` and `hellboy` are two real libraries.
    let canonical = dest_root.canonicalize()?;
    Ok(format!(
        "{}.lock",
        crate::corpus::sha256_hex(canonical.to_string_lossy().as_bytes())
    ))
}

/// Whether some process still holds `path`'s write lock. Opens read-only, so
/// a read-only record answers "not live" instead of failing; only a genuine
/// `EWOULDBLOCK` means live.
pub fn is_live(path: &Path) -> io::Result<bool> {
    // When not held, this briefly takes the lock itself, so a `hold` starting
    // at that instant gets `WouldBlock`: a narrow window, and the failure is a
    // refused run, not data loss.
    let file = File::open(path)?;
    Ok(!crate::fsops::flock_nb(&file)?)
}

/// Open `path` for writing and hold its lock until the returned file drops.
pub fn hold(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    if !crate::fsops::flock_nb(&file)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "record is already held",
        ));
    }
    Ok(file)
}

/// `(secs, pid, seq)` from a name `basename` produced, or `None` for anything
/// else. Numeric, because a lexical sort puts `.9.` after `.10.`.
fn parse_name(name: &str) -> Option<(u64, u32, u64)> {
    let rest = name.strip_prefix(".comic-renamer.")?.strip_suffix(".record")?;
    let mut parts = rest.split('.');
    let key = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(key)
}

/// Every record in `dir`, newest first. Only names `basename` could have made
/// count; the content is never read, so a record `summarise` refuses is still
/// listed and a stray file cannot abort a sweep.
pub fn list(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(key) = entry.file_name().to_str().and_then(parse_name) {
            found.push((key, entry.path()));
        }
    }
    found.sort_by_key(|(key, _)| std::cmp::Reverse(*key));
    Ok(found.into_iter().map(|(_, path)| path).collect())
}

/// Delete every record in `dir` other than `keep` that was made for exactly
/// this `source_root` and `dest_root` and that no run holds. Returns how many.
///
/// **The caller must hold the library lock for `dest_root`**, and must call this
/// only after it has created `keep`. Both matter because this deletes the only
/// record of where a user's files went.
///
/// - With the lock held, no other run for this library can be writing a
///   record, so `is_live` can only be answering about one abandoned by a
///   crashed run, whose `flock` the kernel has already released. Without the
///   lock, `is_live` then `remove_file` is check-then-act (invariant 6's
///   forbidden shape) aimed at the file that can undo a whole run.
/// - A `keep` that was never created matches nothing, so every record for the
///   pair would go. A run that wrote no record must not call this at all.
///
/// A record `summarise` cannot read is left alone: this cannot tell whose it
/// is.
pub fn supersede(
    dir: &Path,
    keep: &Path,
    source_root: &Path,
    dest_root: &Path,
) -> io::Result<usize> {
    let mut removed = 0;
    for path in list(dir)? {
        if path.file_name() == keep.file_name() {
            continue;
        }
        let Ok(summary) = crate::undo::summarise(&path) else {
            continue;
        };
        if summary.source_root != source_root || summary.dest_root != dest_root {
            continue;
        }
        if is_live(&path)? {
            continue;
        }
        std::fs::remove_file(&path)?;
        removed += 1;
    }
    Ok(removed)
}
