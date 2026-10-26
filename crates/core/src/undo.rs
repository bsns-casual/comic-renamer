//! Append-only undo manifest and `reverse`.
//!
//! Written to `docs/specs/2026-10-21-undo-and-reverse-design.md`, which exists
//! because two earlier implementations of this file failed adversarial review.
//! The invariants it states are the ones to preserve; the comments below name
//! which bug each one replaces, because every one of them was a
//! reasonable-looking shortcut.

use crate::fsops::{
    ensure_root, prune_empty_dirs, resolve_at, sha256_of, stat_at, stream_verified, Entry,
};
use crate::preset::preset_by_id;
use crate::types::{
    Aside, Mechanism, Mode, Resolved, Status as MapStatus, UndoError, UndoReport,
};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// Everything a preset needs to rebuild a destination travels with the row, so
/// a record reverses without the plan that produced it. The roots row carries
/// the preset id and the planned count in the last two columns.
pub const COLUMNS: &str = "seq,status,mode,mechanism,source_name,dest_relative,\
    size,inode,mtime_ns,series,volume,issue,subtitle,run,extension,preset,planned";

/// Trailing columns after `mtime_ns` on rows that do not carry the pending
/// fields: series, volume, issue, subtitle, run, extension, preset, planned.
const PAD: &str = ",,,,,,,,";

/// What a record says about its run, read without reversing anything.
#[derive(Clone, Debug)]
pub struct Summary {
    pub source_root: PathBuf,
    pub dest_root: PathBuf,
    pub preset: String,
    pub planned: u32,
    /// Rows the reverse can identify at the destination, which is the only
    /// thing that makes a row reversible: `Row::expected` is the authority on
    /// that, and this counts what it answers for.
    ///
    /// Derived rather than paralleled, because a second list of statuses
    /// disagreed with the first. It counted `placed` and `done` and so missed
    /// `pending` + `rename` — a crash inside move mode's one-syscall window,
    /// the default mode — which `expected` identifies exactly, since a rename
    /// moves the inode and the recorded source identity still describes the
    /// destination. Three gates read this number (`pick_record`,
    /// `menu::waiting`, and D57's interrupted exception in
    /// `record::supersede`), so one wrong count meant `undo` said nothing was
    /// restored, the menu drew no offer, and the next `apply` of the same
    /// bundle deleted the record — while `undo <run-id>` put the file back.
    pub reversible: u32,
    /// This record does not account for everything its run planned, so the
    /// library it names may be inconsistent right now. `done` is the only
    /// status that says a row finished, and `planned` is how many rows the run
    /// was about to attempt — see `summarise` for which states reach this.
    pub interrupted: bool,
    pub started: std::time::SystemTime,
}

/// `(size, inode, mtime_ns)`. All three, because size and inode alone survive
/// an in-place edit — and the duplicate branch would then delete the only
/// *corrected* copy of a file the user had fixed.
type Identity = (u64, u64, i64);

fn identity_of(meta: &std::fs::Metadata) -> Identity {
    (
        meta.len(),
        meta.ino(),
        meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
    )
}

pub struct UndoWriter {
    file: std::fs::File,
    seq: u64,
}

impl UndoWriter {
    /// `O_EXCL`: a record's name is its own, and the open that creates it must
    /// never land on a name some other run is already writing. (The *lock*
    /// uses `flock` instead — see `apply::Lock` and `record::hold`.)
    ///
    /// Carries the preset id and the planned count a later `reverse` needs.
    /// The roots row's `mtime_ns` column holds the start time.
    pub fn create(
        path: &Path,
        source_root: &Path,
        dest_root: &Path,
        preset: &str,
        planned: u32,
    ) -> io::Result<UndoWriter> {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        writeln!(file, "{COLUMNS}")?;
        writeln!(
            file,
            "0,roots,,,{},{},,,{started},,,,,,,{},{planned}",
            path_field(source_root),
            path_field(dest_root),
            field(preset),
        )?;
        file.sync_all()?;
        // `sync_all` makes the contents durable, not the name.
        sync_parent(path)?;
        Ok(UndoWriter { file, seq: 0 })
    }

    /// About to attempt. Carries the source's identity, which is all that is
    /// knowable before the operation, and the fields a preset needs to rebuild
    /// `dest_relative`. Text fields take the same always-on `'` prefix as
    /// paths, so a series named `=cmd` is inert in a spreadsheet.
    #[allow(clippy::too_many_arguments)]
    pub fn record_pending(
        &mut self,
        source_name: &Path,
        dest_relative: &Path,
        mechanism: Mechanism,
        mode: Mode,
        source: &Path,
        series: &str,
        volume: Option<u32>,
        issue: Option<u32>,
        subtitle: Option<&str>,
        run: u32,
        extension: &str,
    ) -> io::Result<u64> {
        self.seq += 1;
        let meta = std::fs::symlink_metadata(source)?;
        let (size, inode, mtime) = identity_of(&meta);
        writeln!(
            self.file,
            "{},pending,{},{},{},{},{size},{inode},{mtime},{},{},{},{},{run},{},,",
            self.seq,
            field(mode_name(mode)),
            field(mechanism_name(mechanism)),
            path_field(source_name),
            path_field(dest_relative),
            field(&format!("'{series}")),
            volume.map(|v| v.to_string()).unwrap_or_default(),
            issue.map(|v| v.to_string()).unwrap_or_default(),
            subtitle.map(|t| field(&format!("'{t}"))).unwrap_or_default(),
            field(&format!("'{extension}")),
        )?;
        self.file.sync_all()
            .map(|()| self.seq)
    }

    /// Attempted, created nothing. Without it a bare `pending` is ambiguous
    /// between "crashed mid-operation" and "refused at the syscall", and
    /// `reverse` guessed between them by matching on size — which is what
    /// reopened the two worst findings.
    pub fn record_failed(&mut self, seq: u64) -> io::Result<()> {
        writeln!(self.file, "{seq},failed,,,,,,,{PAD}")?;
        self.file.sync_all()
    }

    /// A new inode exists at the destination and is verified. Written by
    /// clone and stream in **both** modes, before any unlink.
    ///
    /// Never written with an empty identity: the same bytes would mean both
    /// "this status carries none" and "the stat failed".
    pub fn record_placed(&mut self, seq: u64, identity: Identity) -> io::Result<()> {
        let (size, inode, mtime) = identity;
        writeln!(self.file, "{seq},placed,,,,,{size},{inode},{mtime}{PAD}")?;
        self.file.sync_all()
    }

    pub fn record_done(&mut self, seq: u64) -> io::Result<()> {
        writeln!(self.file, "{seq},done,,,,,,,{PAD}")?;
        self.file.sync_all()
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::File::open(parent)?.sync_all()
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Move => "move",
        Mode::Copy => "copy",
    }
}

fn mechanism_name(mechanism: Mechanism) -> &'static str {
    match mechanism {
        Mechanism::Rename => "rename",
        Mechanism::Clone => "clone",
        Mechanism::Stream => "stream",
    }
}

/// RFC 4180: quote when the value holds a comma, a quote or a newline, and
/// double any embedded quote. The GCD lists *Hellboy, the Corpse and the Iron
/// Shoes*, so the comma case is not hypothetical.
fn field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Paths carry a leading `'` **always**, not only when they begin with
/// `= + - @`. A conditional prefix cannot be undone: a path that genuinely
/// starts with `'` would lose it on the way back.
fn path_field(path: &Path) -> String {
    field(&format!("'{}", path.to_string_lossy()))
}

fn unprefix(value: &str) -> &str {
    value.strip_prefix('\'').unwrap_or(value)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Status {
    Pending,
    Failed,
    Placed,
    Done,
}

#[derive(Clone, Debug)]
struct Row {
    seq: u64,
    status: Status,
    mode: Mode,
    mechanism: Mechanism,
    source_name: PathBuf,
    dest_relative: PathBuf,
    source_identity: Option<Identity>,
    placed_identity: Option<Identity>,
    /// Never absent: a `pending` row that carries no rebuildable fields is
    /// reported as unreadable at parse time rather than becoming a row the
    /// reverse has to refuse later. The two reach the user identically — both
    /// land in `UndoReport::refused` — and only one of them needs a branch
    /// nothing this build can write will ever take.
    fields: Fields,
}

#[derive(Clone, Debug)]
struct Fields {
    series: String,
    volume: Option<u32>,
    issue: Option<u32>,
    subtitle: Option<String>,
    run: u32,
    extension: String,
}

impl Row {
    /// The identity the destination must have for this row to own it.
    ///
    /// Exact, or `None` and the row is refused. No arm that widens the match:
    /// the size-only fallback an earlier version used fired far more often
    /// than its author intended, because comics from one bundle are
    /// byte-identical.
    fn expected(&self) -> Option<Identity> {
        match (self.status, self.mechanism) {
            (Status::Placed | Status::Done, _) => self.placed_identity.or(
                // A rename moves the inode, so the source's identity still
                // describes the destination exactly.
                match self.mechanism {
                    Mechanism::Rename => self.source_identity,
                    _ => None,
                },
            ),
            // Interrupted mid-operation. Only a rename can still be identified.
            (Status::Pending, Mechanism::Rename) => self.source_identity,
            (Status::Pending, _) => None,
            (Status::Failed, _) => None,
        }
    }
}

/// Reads a record's own summary: no plan, no filesystem beyond the record.
pub fn summarise(manifest: &Path) -> Result<Summary, UndoError> {
    let m = read_manifest(manifest)?;
    let started = match m.started {
        Some(t) => t,
        None => {
            let meta = std::fs::metadata(manifest)?;
            // Birth time is when the record was created. `modified()` is the
            // last append, which is when the run *ended*: the wrong end, kept
            // only for filesystems with no birth time.
            meta.created().or_else(|_| meta.modified())?
        }
    };
    // From `expected()`, not from a list of statuses beside it: see
    // `Summary::reversible`.
    let reversible = m.rows.iter().filter(|r| r.expected().is_some()).count() as u32;
    // `interrupted` means "this record does not account for everything it
    // planned", which is not the same as "some row is `pending`". `apply`
    // abandons the run on the first row that fails, and that row is written
    // `failed`, or left at `placed` when its destination landed and the unlink
    // failed — **neither leaves a `pending` row**. Reading only `pending`
    // therefore called an aborted run a completed one, and the menu offered to
    // forget the only record of a half-migrated library.
    //
    // Two signals, each reaching a state the other cannot see:
    //
    // - A row that is not `done` is a row that did not finish: `pending` (the
    //   process died mid-operation), `failed` (refused at the syscall), or
    //   `placed` with no `done` (the copy landed, the unlink did not). The
    //   last of those is why the count alone is not enough — it can be the
    //   final planned row, and then `reversible == planned`.
    // - Fewer finished rows than `planned`: a crash during a `record_pending`
    //   write loses that row's line entirely, so no row is left to be
    //   not-`done`. This is also the only signal that survives a torn record,
    //   whose unreadable lines `Summary` does not carry.
    //
    // The first is independent of `planned`, which a hand-edited or
    // short roots row can leave at 0.
    let done = m.rows.iter().filter(|r| r.status == Status::Done).count() as u32;
    let interrupted = done < m.planned || m.rows.iter().any(|r| r.status != Status::Done);
    Ok(Summary {
        source_root: m.source_root,
        dest_root: m.dest_root,
        preset: m.preset,
        planned: m.planned,
        reversible,
        interrupted,
        started,
    })
}

/// Reverses a run from its record alone. Roots and preset id come from the
/// record; every destination is recomputed through the preset and must equal
/// the one the row names, so an edited record cannot name an arbitrary path
/// (invariant 2).
///
/// Every row is independent, including rows that fail to parse: aborting on the
/// first problem left every earlier successful row stranded, and re-running hit
/// the same row forever.
///
/// `apply::reverse_run` is the entry point: it holds the library lock and
/// deletes the record once nothing was refused.
pub fn reverse(manifest: &Path) -> Result<UndoReport, UndoError> {
    let m = read_manifest(manifest)?;
    let preset = preset_by_id(&m.preset).ok_or_else(|| UndoError::Parse {
        line: 0,
        message: format!("unknown preset {:?}", m.preset),
    })?;

    // D58's aside directory, named for the run whose reverse fills it: the run
    // id is the record's own file name, which is what `undo --list` prints and
    // `undo <run-id>` takes. One directory per run, so two runs' reverses
    // cannot collide inside it.
    let run_id = manifest.file_name().map(Path::new).ok_or_else(|| UndoError::Parse {
        line: 0,
        message: "this record has no file name to name an aside directory after".into(),
    })?;
    // `file_name` cannot yield a separator or `..`, so this holds by
    // construction. Checked anyway: it is the one component of the aside path
    // that does not come from this file, and a path reaching the library under
    // a name the tool did not choose is the shape of every finding this
    // branch exists to close.
    single_component(run_id).map_err(|message| UndoError::Parse { line: 0, message })?;
    let aside = Path::new(ASIDE).join(run_id);

    let mut report = UndoReport::default();
    let mut moved_aside = 0usize;
    report.refused.extend(m.unreadable);

    // The bundle folder is where the files go back to, and a user who deleted
    // it once it emptied has not made them unrestorable. Only with the library
    // still there: a record whose destination is gone has nothing to anchor.
    // A failure here is left for the canonicalisation below to report.
    if m.dest_root.is_dir() && !m.source_root.exists() {
        let _ = ensure_root(&m.source_root);
    }
    let (source_root, dest_root) = match (
        std::fs::canonicalize(&m.source_root),
        std::fs::canonicalize(&m.dest_root),
    ) {
        (Ok(s), Ok(d)) => (s, d),
        (s, d) => {
            // Nothing can be anchored, so nothing is touched; each row says why.
            let why = [(&m.source_root, s), (&m.dest_root, d)]
                .into_iter()
                .filter_map(|(p, r)| r.err().map(|e| format!("{}: {e}", p.display())))
                .collect::<Vec<_>>()
                .join("; ");
            report
                .refused
                .extend(m.rows.iter().map(|r| format!("row {}: root unavailable ({why})", r.seq)));
            return Ok(report);
        }
    };
    // Equal roots would let a content check hash one inode against itself and
    // agree, deleting the only copy.
    if source_root.starts_with(&dest_root) || dest_root.starts_with(&source_root) {
        return Err(UndoError::RootsOverlap);
    }

    for row in m.rows.into_iter().rev() {
        let f = &row.fields;
        let resolved = Resolved {
            series: f.series.clone(),
            volume: f.volume,
            issue: f.issue,
            subtitle: f.subtitle.clone(),
            run: f.run,
            status: MapStatus::Mapped,
            record: None,
        };
        let own = [preset.destination(&resolved, &f.extension)];
        match reverse_one(&row, &source_root, &dest_root, &own, &aside) {
            // Still a reverse: the library no longer holds the file and the
            // bundle does. It also left a file behind, which the report has
            // to name — nothing else will.
            Ok(Reversed::MovedAside) => {
                report.restored += 1;
                moved_aside += 1;
            }
            Ok(Reversed::Restored) => report.restored += 1,
            Ok(Reversed::Nothing) => report.skipped += 1,
            Err(why) => report.refused.push(format!("row {}: {why}", row.seq)),
        }
    }
    if moved_aside > 0 {
        let directory = dest_root.join(&aside);
        // What the directory holds, not what this pass put there: a reverse
        // that refused some rows and was run again adds to a directory the
        // earlier pass started. A read that fails still has `moved_aside`,
        // which is a true lower bound, so no reverse reports a directory
        // without a count.
        let files = std::fs::read_dir(&directory)
            .map(Iterator::count)
            .unwrap_or(moved_aside);
        report.aside = Some(Aside { directory, files });
    }
    Ok(report)
}

fn reverse_one(
    row: &Row,
    source_root: &Path,
    dest_root: &Path,
    recomputed: &[PathBuf],
    aside: &Path,
) -> Result<Reversed, String> {
    if row.status == Status::Failed {
        return Ok(Reversed::Nothing);
    }
    // Invariant 2 applied to reverse: a destination is only recognised if the
    // preset still produces it. Without this a hand-written manifest names an
    // arbitrary relative path.
    if !recomputed.iter().any(|p| p == &row.dest_relative) {
        return Err(format!(
            "{} is not a destination this plan produces",
            row.dest_relative.display()
        ));
    }
    plain_relative(&row.dest_relative)?;
    let source_name = single_component(&row.source_name)?;

    // Resolved once and held: the identity check and whatever acts on it go
    // through the same directory descriptor, so a component swapped between
    // the two cannot redirect the second.
    let entry = match resolve_at(dest_root, &row.dest_relative) {
        Ok(entry) => entry,
        // The parent is gone — pruned by an earlier successful row in this
        // same run, usually. That is "nothing to do", not a refusal: treating
        // it as one left `refused` non-empty forever, so the manifest was
        // never renamed aside and `O_EXCL` killed the plan permanently.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Reversed::Nothing),
        Err(e) => {
            return Err(format!(
                "cannot resolve {}: {e}",
                row.dest_relative.display()
            ))
        }
    };
    let actual = match entry.identity() {
        Ok(actual) => actual,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Reversed::Nothing),
        Err(e) => {
            // `Path::exists()` answers false on EACCES, so a permission
            // problem used to read as "already reversed".
            return Err(format!(
                "cannot examine {}: {e}",
                row.dest_relative.display()
            ));
        }
    };
    let expected = row
        .expected()
        .ok_or_else(|| "no exact identity was recorded for this row".to_string())?;
    if actual != expected {
        return Err(format!(
            "{} is not the file this run put there",
            row.dest_relative.display()
        ));
    }

    let source = stat_at(source_root, source_name);
    match row.mode {
        Mode::Copy => {
            // Invariant 6. Copy a bundle in, delete the bundle folder, undo:
            // removing the destination would destroy the last copy, and an
            // earlier version did exactly that and reported success.
            match &source {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Err(format!(
                    "the original {} is gone; removing {} would destroy the last copy",
                    source_name.display(),
                    row.dest_relative.display()
                )),
                Err(e) => Err(format!("cannot examine the original: {e}")),
                Ok(actual) if row.source_identity != Some(*actual) => Err(format!(
                    "the original {} is not what this run copied from",
                    source_name.display()
                )),
                Ok(_) => move_aside_with_a_copy_at_the_source(
                    &entry,
                    source_root,
                    source_name,
                    dest_root,
                    &row.dest_relative,
                    aside,
                ),
            }
        }
        Mode::Move => match &source {
            // The duplicate a failed unlink leaves. Gated on the status
            // *and* on the source's identity: the status says a copy was
            // made, but only the identity says the thing now sitting at the
            // source is the original it was made from. Without that check,
            // re-extracting the bundle — or dropping in a truncated file —
            // makes `reverse` delete the verified library copy and report
            // success. Invariant 6.
            Ok(actual) if row.status == Status::Placed
                && row.source_identity == Some(*actual) =>
            {
                move_aside_with_a_copy_at_the_source(
                    &entry,
                    source_root,
                    source_name,
                    dest_root,
                    &row.dest_relative,
                    aside,
                )
            }
            Ok(actual) if row.source_identity == Some(*actual) => Ok(Reversed::Nothing),
            Ok(_) => Err(format!("{} is occupied", source_name.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // The common case is a user who deleted the emptied bundle
                // folder; `ensure_parent` is a no-op for a single component.
                ensure_root(source_root).map_err(|e| e.to_string())?;
                let restore = entry.rename_into(source_root, Path::new(source_name));
                match restore {
                    Ok(()) => {
                        prune_empty_dirs(dest_root, &row.dest_relative)
                            .map_err(|e| e.to_string())?;
                        Ok(Reversed::Restored)
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        Err(format!("{} is occupied", source_name.display()))
                    }
                    // A cross-volume move was executed by streaming, so it is
                    // reversed by streaming. `rename` fails EXDEV on exactly
                    // the runs where the move was most expensive.
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                        // shortcut: `stream_verified` re-opens the library
                        // file by path, so this row's intermediate components
                        // are resolved a second time rather than through the
                        // descriptor `resolve_at` already returned. Closing
                        // that needs an fd-taking variant.
                        //
                        // State the limit exactly, because the unlink below is
                        // deliberately ungated and this is its whole defence.
                        // The re-check establishes that the file this unlink
                        // removes is still the one the row recorded, so the
                        // *removal* is of the right file. It does not
                        // establish that the bytes now at the source came from
                        // that file: a component swapped inside the library
                        // between `resolve_at` and the open above would have
                        // the copy read a stranger, and the recorded file
                        // would still be removed, leaving the source holding
                        // foreign bytes. That needs a same-user process racing
                        // a directory swap inside the library during a
                        // cross-volume undo.
                        let from = dest_root.join(&row.dest_relative);
                        stream_verified(&from, source_root, Path::new(source_name))
                            .map_err(|e| e.to_string())?;
                        // Re-read through the handle before deleting: the
                        // copy was verified against whatever was read, which
                        // is not the same as against what this row recorded.
                        match entry.identity() {
                            Ok(still) if still == expected => {}
                            _ => {
                                return Err(format!(
                                    "{} changed while it was being restored; it was left in place",
                                    row.dest_relative.display()
                                ))
                            }
                        }
                        // Deliberately *not*
                        // `move_aside_with_a_copy_at_the_source`. The copy at
                        // the source is one this run streamed back a moment
                        // ago, not one the record claims was always there, and
                        // `stream_verified` compared its SHA-256 against the
                        // bytes it read. Hashing the source again would
                        // re-read the payload for a comparison already made
                        // against those same bytes — see the limit stated
                        // above for what "those same bytes" does and does not
                        // cover. The asymmetry is the point: the gate exists
                        // for sources a record vouches for, and this one the
                        // tool wrote itself.
                        //
                        // **And this one still unlinks** (D58). The two
                        // record-authorised sites rename into the aside
                        // directory because the agreement that licenses them
                        // is about the *source*, which nothing locks, so it
                        // can be stale by the time they act. Here the
                        // agreement is about the destination:
                        // `entry.identity()` was just re-read through the
                        // pinned descriptor, so the file this unlinks is the
                        // one the row recorded, and the copy at the source is
                        // one this run wrote rather than one the record
                        // vouches for.
                        //
                        // Stated no more strongly than that. It bounds
                        // *which* file goes; it does not establish that the
                        // same bytes now exist in two places, and the
                        // `shortcut` above is why — `stream_verified`
                        // re-opens the library file by path, so a component
                        // swapped inside the library during the stream has
                        // the copy read a stranger while the recorded file is
                        // still removed. Moving aside here would make that
                        // residual recoverable, at the price of a second copy
                        // of the whole payload in the library on every
                        // cross-volume undo; D58 weighed the two and kept the
                        // unlink. Closing the residual itself needs the
                        // fd-taking variant the shortcut names.
                        match entry.remove() {
                            Ok(()) => {
                                prune_empty_dirs(dest_root, &row.dest_relative)
                                    .map_err(|e| e.to_string())?;
                                Ok(Reversed::Restored)
                            }
                            // Reported for what it is. The previous version
                            // called this "occupied", pointing the user at
                            // deleting the copy they had just recovered.
                            Err(e) => Err(format!(
                                "a duplicate remains at {}; the original was restored ({e})",
                                row.dest_relative.display()
                            )),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(format!("cannot examine the original: {e}")),
        },
    }
}

/// What one row's reverse did.
///
/// `MovedAside` is still a reverse — the library no longer holds the file and
/// the bundle does — but it also left a file behind, in a directory nothing
/// else on the system will ever mention. Counted separately for that reason.
enum Reversed {
    /// Already reversed, or never executed. Nothing to do.
    Nothing,
    /// The file is back at its source.
    Restored,
    /// The library's copy was renamed into the aside directory (D58).
    MovedAside,
}

/// Where a reverse puts a library file it is authorised to remove (D58).
///
/// Under `dest_root`, so the rename is one `renameat` on one filesystem:
/// atomic, and incapable of half-completing the way a cross-volume
/// copy-then-unlink can. Dot-prefixed so a library scanner skips it.
///
/// **Nothing ever empties it.** Removing the files is a keystroke the user
/// makes. An automatic purge would put back the destructive path D58 removes,
/// and every sweep this project has written for its own convenience has had
/// to be argued out of deleting something.
const ASIDE: &str = ".comic-renamer-aside";

/// The one gate on every library removal whose authority comes from a record.
///
/// The identity check above compares the destination against the size, inode
/// and mtime the record carries — numbers whoever wrote the record chose, and
/// chose the file they are compared against. A reviewer executed that: a
/// hand-written record deleted a file no run had placed, and the report said
/// `restored: 1, refused: []`. A better identity check cannot close it,
/// because the record is still both sides of the comparison.
///
/// So the filesystem decides instead. The destination moves only when a
/// regular file still sits at the recorded source and holds the same bytes,
/// which makes it a de-duplication rather than a deletion. Someone who plants
/// a record cannot make the tool displace anything they did not already hold
/// a copy of.
///
/// Both sites that act on record-supplied source data call this —
/// `Mode::Copy`, and `Mode::Move`'s `placed` branch clearing the duplicate a
/// failed unlink left. Guarding one and not the other only moves the finding
/// to the other door.
///
/// The source is hashed first: it is the record-claimed side, so it is where a
/// refusal is expected, and reading it cannot hydrate a library file that is
/// about to be refused anyway.
///
/// **What this check is for, since D58.** It decides whether the reverse is
/// *correct*, and no longer whether it is *safe*. It used to decide both, and
/// could not: an adversarial review showed the check-then-act, because the
/// source is hashed by path and nothing holds it afterwards. A writer
/// truncating the source between the two hashes got the library copy unlinked
/// anyway — `1 restored · 0 refused`, exit 0, and a 400 MB payload in neither
/// place, on the first attempt. Narrowing the window does not close it: a
/// truncation between a re-hash and the unlink loses the same bytes, and the
/// bundle folder has no lock by accepted design, so any content check on it is
/// racy by construction. So the destructive step went instead (D58). A stale
/// or forged authorisation now relocates a file inside the user's own library
/// rather than destroying it.
fn move_aside_with_a_copy_at_the_source(
    entry: &Entry,
    source_root: &Path,
    source_name: &Path,
    dest_root: &Path,
    dest_relative: &Path,
    aside: &Path,
) -> Result<Reversed, String> {
    let source = sha256_of(&source_root.join(source_name))
        .map_err(|e| format!("cannot read the original {}: {e}", source_name.display()))?;
    // Reading a cloud placeholder here hydrates it, which costs the download
    // of a file that is then moved. D23 already accepts that cost on the way
    // in, and the alternative is a hash the record supplies.
    let placed = entry
        .sha256()
        .map_err(|e| format!("cannot read {}: {e}", dest_relative.display()))?;
    if source != placed {
        return Err(format!(
            "the original {} does not hold the contents of {}; it was left in place",
            source_name.display(),
            dest_relative.display()
        ));
    }
    // The leaf, and nothing else. `dest_relative` came from the record, and
    // nothing here reconstructs a path it supplied: the shape of the aside
    // path is this module's, and this is one component inside it. Two rows
    // sharing a leaf within one run therefore collide, which `RENAME_EXCL`
    // reports rather than overwriting (invariant 6).
    let leaf = dest_relative
        .file_name()
        .ok_or_else(|| format!("{} names no file", dest_relative.display()))?;
    let target = aside.join(leaf);
    // Created here rather than up front: a reverse that moves nothing aside —
    // every move-mode run — must not leave an empty directory in a library.
    ensure_root(&dest_root.join(aside))
        .map_err(|e| format!("cannot make room at {}: {e}", aside.display()))?;
    // `Entry::rename_into` is `renameatx_np(RENAME_EXCL)` through the entry's
    // own pinned parent descriptor — the same one the identity check and the
    // hash above used, so the act and the checks stay on one resolution.
    entry.rename_into(dest_root, &target).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            format!(
                "{} is already set aside at {}; {} was left in the library",
                Path::new(leaf).display(),
                target.display(),
                dest_relative.display()
            )
        } else {
            format!("cannot move {} aside: {e}", dest_relative.display())
        }
    })?;
    prune_empty_dirs(dest_root, dest_relative).map_err(|e| e.to_string())?;
    Ok(Reversed::MovedAside)
}

fn plain_relative(path: &Path) -> Result<(), String> {
    if path.components().all(|c| matches!(c, Component::Normal(_))) && path.components().count() > 0
    {
        Ok(())
    } else {
        Err(format!("{} is not a plain relative path", path.display()))
    }
}

fn single_component(path: &Path) -> Result<&Path, String> {
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(path),
        _ => Err(format!("{} is not a single filename", path.display())),
    }
}

fn pending_fields(columns: &[String]) -> Option<Fields> {
    let get = |i: usize| columns.get(i).map(String::as_str);
    let number = |i: usize| match get(i) {
        Some("") => Some(None),
        Some(v) => v.parse().ok().map(Some),
        None => None,
    };
    Some(Fields {
        series: unprefix(get(9)?).to_string(),
        volume: number(10)?,
        issue: number(11)?,
        subtitle: match get(12)? {
            "" => None,
            t => Some(unprefix(t).to_string()),
        },
        run: get(13)?.parse().ok()?,
        extension: unprefix(get(14)?).to_string(),
    })
}

/// Roots, preset id, planned count and start time, then the rows.
struct Manifest {
    source_root: PathBuf,
    dest_root: PathBuf,
    preset: String,
    planned: u32,
    started: Option<std::time::SystemTime>,
    rows: Vec<Row>,
    unreadable: Vec<String>,
}

fn read_manifest(manifest: &Path) -> Result<Manifest, UndoError> {
    let text = std::fs::read_to_string(manifest)?;
    let (records, torn) = parse_records(&text);

    let mut roots: Option<(PathBuf, PathBuf)> = None;
    let mut preset = String::new();
    let mut planned = 0u32;
    let mut started = None;
    let mut rows: Vec<Row> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    if let Some(problem) = torn {
        unreadable.push(problem);
    }

    for (index, columns) in records.iter().enumerate() {
        let line = index + 1;
        // Skip the column header only if that is what it is. Skipping
        // records[0] unconditionally loses the first data row of a manifest
        // that has no header — a hand-edited file, which is exactly the
        // input invariant 1 exists for.
        //
        // A header that is present and wrong was written by another build, and
        // its columns mean something else.
        if index == 0 && columns.first().map(String::as_str) == Some("seq") {
            if columns.join(",") != COLUMNS {
                return Err(UndoError::Parse {
                    line: 1,
                    message: "this record was written by an older build and has no \
                              fields to rebuild destinations from"
                        .into(),
                });
            }
            continue;
        }
        if columns.iter().all(|c| c.is_empty()) {
            continue;
        }
        // Short rows, unparseable sequences and unknown vocabulary are all
        // per-row problems. Only a missing or duplicated roots record can
        // refuse the whole file, because without it nothing can be anchored.
        if columns.len() < 9 {
            unreadable.push(format!("line {line}: expected nine columns"));
            continue;
        }
        let identity = match (
            columns[6].parse::<u64>(),
            columns[7].parse::<u64>(),
            columns[8].parse::<i64>(),
        ) {
            (Ok(size), Ok(inode), Ok(mtime)) => Some((size, inode, mtime)),
            _ => None,
        };
        if columns[1] == "roots" {
            if roots.is_some() {
                return Err(UndoError::NoRoot);
            }
            roots = Some((
                PathBuf::from(unprefix(&columns[4])),
                PathBuf::from(unprefix(&columns[5])),
            ));
            // Indices into COLUMNS. Inserting a column ahead of these moves
            // them, which reads as `unknown preset ""` — pinned by
            // `the_column_list_is_read_at_the_indices_this_code_uses`.
            preset = columns.get(15).cloned().unwrap_or_default();
            planned = columns.get(16).and_then(|c| c.parse().ok()).unwrap_or(0);
            started = columns[8]
                .parse::<u64>()
                .ok()
                .map(|n| std::time::UNIX_EPOCH + std::time::Duration::from_nanos(n));
            continue;
        }
        let Ok(seq) = columns[0].parse::<u64>() else {
            unreadable.push(format!("line {line}: sequence {:?} is not a number", columns[0]));
            continue;
        };
        match columns[1].as_str() {
            "pending" if rows.iter().any(|r| r.seq == seq) => {
                unreadable.push(format!("line {line}: duplicate row {seq}"));
            }
            "pending" => {
                let Some(mode) = parse_mode(&columns[2]) else {
                    unreadable.push(format!("line {line}: unknown mode {:?}", columns[2]));
                    continue;
                };
                let Some(mechanism) = parse_mechanism(&columns[3]) else {
                    unreadable.push(format!(
                        "line {line}: unknown mechanism {:?}",
                        columns[3]
                    ));
                    continue;
                };
                // A row with nothing to rebuild its destination from cannot be
                // reversed at all, so it is reported here rather than carried
                // as a row whose every later check has to allow for it.
                let Some(fields) = pending_fields(columns) else {
                    unreadable.push(format!(
                        "line {line}: row {seq} carries no fields to rebuild its destination"
                    ));
                    continue;
                };
                rows.push(Row {
                    seq,
                    status: Status::Pending,
                    mode,
                    mechanism,
                    source_name: PathBuf::from(unprefix(&columns[4])),
                    dest_relative: PathBuf::from(unprefix(&columns[5])),
                    source_identity: identity,
                    placed_identity: None,
                    fields,
                });
            }
            // Last status wins per seq; a status row with no pending is
            // ignored rather than fatal.
            "failed" => orphan(&mut rows, seq, Status::Failed, None, line, &mut unreadable),
            "placed" => orphan(&mut rows, seq, Status::Placed, identity, line, &mut unreadable),
            "done" => orphan(&mut rows, seq, Status::Done, None, line, &mut unreadable),
            other => unreadable.push(format!("line {line}: unknown status {other:?}")),
        }
    }

    let (source_root, dest_root) = roots.ok_or(UndoError::NoRoot)?;
    Ok(Manifest { source_root, dest_root, preset, planned, started, rows, unreadable })
}

/// Last status wins per `seq`. A status row whose `seq` has no `pending` is
/// reported rather than ignored: silence let a manifest lose rows while the
/// user was told the undo was clean.
fn orphan(
    rows: &mut [Row],
    seq: u64,
    status: Status,
    identity: Option<Identity>,
    line: usize,
    unreadable: &mut Vec<String>,
) {
    match rows.iter_mut().find(|r| r.seq == seq) {
        Some(row) => {
            row.status = status;
            if identity.is_some() {
                row.placed_identity = identity;
            }
        }
        None => unreadable.push(format!("line {line}: status for unknown row {seq}")),
    }
}

fn parse_mode(value: &str) -> Option<Mode> {
    match value {
        "move" => Some(Mode::Move),
        "copy" => Some(Mode::Copy),
        _ => None,
    }
}

fn parse_mechanism(value: &str) -> Option<Mechanism> {
    match value {
        "rename" => Some(Mechanism::Rename),
        "clone" => Some(Mechanism::Clone),
        "stream" => Some(Mechanism::Stream),
        _ => None,
    }
}

/// RFC 4180 **records**, not lines. `str::lines` splits inside a quoted
/// field, so a filename containing a newline — legal on APFS, and written
/// correctly by the writer — made an entire run unreadable.
///
/// A trailing fragment is returned separately: a torn final record is the
/// crash this file exists to survive, not a reason to abandon the records
/// before it.
fn parse_records(text: &str) -> (Vec<Vec<String>>, Option<String>) {
    let mut records = Vec::new();
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            '\n' if !quoted => {
                fields.push(std::mem::take(&mut current));
                records.push(std::mem::take(&mut fields));
            }
            '\r' if !quoted => {}
            _ => current.push(c),
        }
    }
    if !quoted && (!current.is_empty() || !fields.is_empty()) {
        // A complete record that simply lacks a trailing newline. Dropping
        // it loses a `placed`, which downgrades a clone row to bare
        // `pending` and makes `expected()` refuse it forever.
        fields.push(current);
        records.push(fields);
        return (records, None);
    }
    if quoted || !current.is_empty() || !fields.is_empty() {
        fields.push(current);
        let tail = fields.join(",");
        // An unterminated quote absorbs the rest of the file into one field,
        // so every record after it is lost — not "one incomplete final
        // record", which is what the report used to say while three valid
        // rows stayed in the library. Count what was swallowed.
        let lost = tail.matches('\n').count();
        let message = if quoted {
            format!(
                "unterminated quote from record {}: {lost} further line(s) could not be read",
                records.len() + 1
            )
        } else {
            format!("incomplete final record: {tail}")
        };
        return (records, Some(message));
    }
    (records, None)
}
