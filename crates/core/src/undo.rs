//! Append-only undo manifest and `reverse`.
//!
//! Written to `docs/specs/2026-10-21-undo-and-reverse-design.md`, which exists
//! because two earlier implementations of this file failed adversarial review.
//! The invariants it states are the ones to preserve; the comments below name
//! which bug each one replaces, because every one of them was a
//! reasonable-looking shortcut.

use crate::fsops::{
    ensure_root, prune_empty_dirs, resolve_at, stat_at, stream_verified,
};
use crate::types::{Mechanism, Mode, UndoError, UndoReport};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const COLUMNS: &str = "seq,status,mode,mechanism,source_name,dest_relative,size,inode,mtime_ns";

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
    /// `O_EXCL`: re-running a plan must not truncate the previous run's
    /// manifest, the only record of where those files went. (The *lock* uses
    /// `flock` instead — see `apply::Lock`.)
    pub fn create(path: &Path, source_root: &Path, dest_root: &Path) -> io::Result<UndoWriter> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        writeln!(file, "{COLUMNS}")?;
        writeln!(
            file,
            "0,roots,,,{},{},,,",
            path_field(source_root),
            path_field(dest_root)
        )?;
        file.sync_all()?;
        // `sync_all` makes the contents durable, not the name.
        sync_parent(path)?;
        Ok(UndoWriter { file, seq: 0 })
    }

    /// About to attempt. Carries the source's identity, which is all that is
    /// knowable before the operation.
    pub fn record_pending(
        &mut self,
        source_name: &Path,
        dest_relative: &Path,
        mechanism: Mechanism,
        mode: Mode,
        source: &Path,
    ) -> io::Result<u64> {
        self.seq += 1;
        let meta = std::fs::symlink_metadata(source)?;
        let (size, inode, mtime) = identity_of(&meta);
        writeln!(
            self.file,
            "{},pending,{},{},{},{},{size},{inode},{mtime}",
            self.seq,
            field(mode_name(mode)),
            field(mechanism_name(mechanism)),
            path_field(source_name),
            path_field(dest_relative),
        )?;
        self.file.sync_all()
            .map(|()| self.seq)
    }

    /// Attempted, created nothing. Without it a bare `pending` is ambiguous
    /// between "crashed mid-operation" and "refused at the syscall", and
    /// `reverse` guessed between them by matching on size — which is what
    /// reopened the two worst findings.
    pub fn record_failed(&mut self, seq: u64) -> io::Result<()> {
        writeln!(self.file, "{seq},failed,,,,,,,")?;
        self.file.sync_all()
    }

    /// A new inode exists at the destination and is verified. Written by
    /// clone and stream in **both** modes, before any unlink.
    ///
    /// Never written with an empty identity: the same bytes would mean both
    /// "this status carries none" and "the stat failed".
    pub fn record_placed(&mut self, seq: u64, identity: Identity) -> io::Result<()> {
        let (size, inode, mtime) = identity;
        writeln!(self.file, "{seq},placed,,,,,{size},{inode},{mtime}")?;
        self.file.sync_all()
    }

    pub fn record_done(&mut self, seq: u64) -> io::Result<()> {
        writeln!(self.file, "{seq},done,,,,,,,")?;
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

/// Reverses a run. Takes the **plan**, deriving the manifest beside it, so it
/// holds the same lock `apply` does and the plan's roots anchor every path
/// the manifest names (D38).
///
/// Every row is independent, including rows that fail to parse: aborting the
/// manifest on the first problem left every earlier successful row stranded,
/// and re-running hit the same row forever.
/// `apply::undo` is the entry point: it takes the plan, holds the lock,
/// canonicalises the roots and recomputes the destinations. This is the
/// manifest-level operation it calls.
pub fn reverse(
    manifest: &Path,
    source_root: &Path,
    dest_root: &Path,
    recomputed: &[PathBuf],
) -> Result<UndoReport, UndoError> {
    let (recorded_source, recorded_dest, rows, unreadable) = read_manifest(manifest)?;

    // The manifest's own roots are a cross-check, not the anchor. Exact
    // identity buys nothing if whoever edits the manifest also supplies the
    // roots those identities are checked against.
    if recorded_source != source_root || recorded_dest != dest_root {
        return Err(UndoError::NoRoot);
    }

    let mut report = UndoReport::default();
    // Every row the parser could not read is a refusal, not a silence. A
    // dropped row means a file stays in the library while the report says
    // the undo was clean.
    report.refused.extend(unreadable);

    for row in rows.into_iter().rev() {
        match reverse_one(&row, source_root, dest_root, recomputed) {
            Ok(true) => report.restored += 1,
            Ok(false) => report.skipped += 1,
            Err(why) => report.refused.push(format!("row {}: {why}", row.seq)),
        }
    }
    Ok(report)
}

fn reverse_one(
    row: &Row,
    source_root: &Path,
    dest_root: &Path,
    recomputed: &[PathBuf],
) -> Result<bool, String> {
    if row.status == Status::Failed {
        return Ok(false);
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
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(format!(
                "cannot resolve {}: {e}",
                row.dest_relative.display()
            ))
        }
    };
    let actual = match entry.identity() {
        Ok(actual) => actual,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
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
                Ok(_) => {
                    entry.remove().map_err(|e| e.to_string())?;
                    prune_empty_dirs(dest_root, &row.dest_relative).map_err(|e| e.to_string())?;
                    Ok(true)
                }
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
                entry.remove().map_err(|e| e.to_string())?;
                prune_empty_dirs(dest_root, &row.dest_relative).map_err(|e| e.to_string())?;
                Ok(true)
            }
            Ok(actual) if row.source_identity == Some(*actual) => Ok(false),
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
                        Ok(true)
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        Err(format!("{} is occupied", source_name.display()))
                    }
                    // A cross-volume move was executed by streaming, so it is
                    // reversed by streaming. `rename` fails EXDEV on exactly
                    // the runs where the move was most expensive.
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                        // shortcut: stream_verified re-opens the source by
                        // path, so the intermediate components are resolved
                        // a second time. Closing that needs an fd-taking
                        // variant; the destination is re-checked below
                        // instead, which is the half that could lose data.
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
                        match entry.remove() {
                            Ok(()) => {
                                prune_empty_dirs(dest_root, &row.dest_relative)
                                    .map_err(|e| e.to_string())?;
                                Ok(true)
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

type Parsed = (PathBuf, PathBuf, Vec<Row>, Vec<String>);

fn read_manifest(manifest: &Path) -> Result<Parsed, UndoError> {
    let text = std::fs::read_to_string(manifest)?;
    let (records, torn) = parse_records(&text);

    let mut roots: Option<(PathBuf, PathBuf)> = None;
    let mut rows: Vec<Row> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    if let Some(problem) = torn {
        unreadable.push(problem);
    }

    for (index, columns) in records.iter().enumerate().skip(1) {
        let line = index + 1;
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
            continue;
        }
        let Ok(seq) = columns[0].parse::<u64>() else {
            unreadable.push(format!("line {line}: sequence {:?} is not a number", columns[0]));
            continue;
        };
        match columns[1].as_str() {
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
                rows.push(Row {
                    seq,
                    status: Status::Pending,
                    mode,
                    mechanism,
                    source_name: PathBuf::from(unprefix(&columns[4])),
                    dest_relative: PathBuf::from(unprefix(&columns[5])),
                    source_identity: identity,
                    placed_identity: None,
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
    Ok((source_root, dest_root, rows, unreadable))
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
