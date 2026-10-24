//! Where a run's undo record lives, and whether one is still being written.
//!
//! A record is held under `flock` for as long as its run writes it, so a
//! crashed run releases it by dying and a live one cannot be mistaken for
//! stale.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A record's basename: `.comic-renamer.<unix-nanos>.<pid>.<seq>.record`. The
/// stamp is nanoseconds because `list` orders by it and "the newest run" is
/// what `undo` picks: at one-second resolution two runs finishing in the same
/// second ordered by pid, which is not time. The pid and per-process counter
/// still keep two names from one instant apart.
pub fn basename(now: SystemTime) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    format!(".comic-renamer.{nanos}.{}.{seq}.record", std::process::id())
}

/// The lock filename for a library: a hash of its canonical root, so a root
/// that is a long or odd path is still one filename. Canonicalisation failure
/// is an error, never a fallback that could give one library two locks.
pub fn lock_name(dest_root: &Path) -> Result<String, io::Error> {
    // Canonicalisation is the whole normalisation: macOS returns the on-disk
    // form, so every spelling of one directory (case, NFC/NFD, symlinks)
    // collapses to one string. Case is deliberately not folded: on a
    // case-sensitive volume `Hellboy` and `hellboy` are two real libraries.
    Ok(lock_name_of(&dest_root.canonicalize()?))
}

/// As `lock_name`, for a root that is **already canonical**.
///
/// `apply::Lock::library` needs the name of the file it locks and the root it
/// then reports to come from one resolution. Taking them from two
/// `canonicalize` calls let a root whose resolution changed between them lock
/// one library while naming another, with no error: measured at 65 of 300
/// attempts against a symlinked root being repointed, each of which ran to
/// completion with the library it wrote left unlocked throughout.
pub fn lock_name_of(canonical: &Path) -> String {
    format!(
        "{}.lock",
        crate::corpus::sha256_hex(canonical.to_string_lossy().as_bytes())
    )
}

/// Whether some process still holds `path`'s write lock. Opens read-only, so
/// a read-only record answers "not live" instead of failing; only a genuine
/// `EWOULDBLOCK` means live.
///
/// A record that is **gone** is held by nobody, so that is `false` and not an
/// error. `menu::decline` deletes records with no library lock, so one can
/// vanish between any listing and this call — and the callers that then
/// propagated the `NotFound` turned someone else's tidy-up into a failed run.
pub fn is_live(path: &Path) -> io::Result<bool> {
    // When not held, this briefly takes the lock itself, so a `hold` starting
    // at that instant gets `WouldBlock`: a narrow window, and the failure is a
    // refused run, not data loss.
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    Ok(!crate::fsops::flock_nb(&file)?)
}

/// Whether some run is writing a record in `dir` right now. Used to tell
/// "nothing to undo" from "a run is mid-flight", which are different answers.
///
/// **An unanswerable liveness question counts as _not_ live here, and that is
/// the opposite default from `menu::waiting`'s identical-looking expression.**
/// The two ask opposite questions, so the same default lands on opposite
/// sides:
///
/// - `waiting` asks "should I offer this record?", and not offering is the
///   safe answer — so an unreadable record is treated as live and filtered out.
/// - this asks "is a run in progress?", and the answer is used to *refuse*. A
///   `true` here from one record at mode `000` makes `undo` refuse with "a run
///   is in progress" while `undo --list` says there are no runs, for as long as
///   that file exists and with nothing the user can do about it from inside the
///   tool.
///
/// So do not unify the two for consistency: the expressions match and the
/// meanings do not.
pub fn any_live(dir: &Path) -> io::Result<bool> {
    Ok(list(dir)?.iter().any(|path| is_live(path).unwrap_or(false)))
}

/// `remove_file`, where a record that is already gone is success.
///
/// `true` when this call removed it. Every deletion of a record goes through
/// here because the end state all of them want is "that record is not there",
/// and `menu::decline` deletes without the library lock — so a `NotFound` is
/// another process having reached the same end state first, not a failure.
/// Propagating it made a run that had placed every row exit non-zero and skip
/// its own `N placed` line.
pub fn forget(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
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

/// `(nanos, pid, seq)` from a name `basename` produced, or `None` for anything
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
///
/// So is an **interrupted** record that still has rows to put back. A new run
/// over the same pair does not make the old one accounted for, and the menu
/// promises in so many words that `l` keeps it ("this stays here until you
/// deal with it"): crash, re-scan the same bundle, re-apply, and the rows the
/// crashed run had already placed would stop being reversible. The
/// rows-to-put-back half is what keeps that from becoming unbounded growth —
/// a run abandoned before it created anything has nothing to lose, so a
/// bundle that fails on its first row does not leave one record per attempt.
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
        if summary.interrupted && summary.reversible > 0 {
            continue;
        }
        if is_live(&path)? {
            continue;
        }
        if forget(&path)? {
            removed += 1;
        }
    }
    Ok(removed)
}
