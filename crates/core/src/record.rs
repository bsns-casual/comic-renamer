//! Where a run's undo record lives, and whether one is still being written.
//!
//! A record is held under `flock` for as long as its run writes it, so a
//! crashed run releases it by dying and a live one cannot be mistaken for
//! stale.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
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
/// that is a long or odd path is still one filename, and `Hellboy/` and
/// `hellboy/` (invariant 7) share a lock. Canonicalisation failure is an
/// error, never a fallback that could give one library two locks.
pub fn lock_name(dest_root: &Path) -> Result<String, io::Error> {
    let canonical = dest_root.canonicalize()?;
    let key = crate::normalize::collision_key(&canonical.to_string_lossy());
    Ok(format!("{}.lock", crate::corpus::sha256_hex(key.as_bytes())))
}

/// Whether some process still holds `path`'s write lock. Opens read-only, so
/// a read-only record answers "not live" instead of failing; only a genuine
/// `EWOULDBLOCK` means live.
pub fn is_live(path: &Path) -> io::Result<bool> {
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
