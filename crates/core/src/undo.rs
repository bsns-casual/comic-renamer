//! Append-only. A `pending` row lands before the operation is attempted and a
//! `done` row after; `reverse` decides from the filesystem, not the status.
//! In-place status rewriting is impossible anyway — "pending" is 7 bytes and
//! "done" is 4. RFC 4180 quoting, doubled embedded quotes.
//!
//! Rewritten after an adversarial review found six structural faults. Each
//! one is named at the code that fixes it, because every one of them was a
//! reasonable-looking shortcut.

use crate::fsops::{rename_excl, stream_verified};
use crate::types::{Mechanism, Mode, UndoError, UndoReport};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const COLUMNS: &str = "seq,status,mode,mechanism,source,destination,size,inode";

pub struct UndoWriter {
    file: std::fs::File,
    seq: u64,
}

impl UndoWriter {
    /// `O_EXCL`: re-running a plan must not truncate the previous run's
    /// manifest, the only record of where those files went.
    ///
    /// `dest_root` is recorded because `reverse` acts destructively on paths
    /// it reads back, and a manifest is as untrusted as a plan (§6.11). A
    /// hand-written two-line manifest used to make `reverse` delete any file
    /// on the system.
    pub fn create(path: &Path, dest_root: &Path) -> io::Result<UndoWriter> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        writeln!(file, "{COLUMNS}")?;
        writeln!(file, "0,root,,,,{},,", path_field(dest_root))?;
        file.sync_all()?;
        // `sync_all` makes the contents durable, not the name. Without an
        // fsync on the containing directory a power loss can leave the
        // manifest's own directory entry missing.
        sync_parent(path)?;
        Ok(UndoWriter { file, seq: 0 })
    }

    /// Returns the row's sequence number, for the matching `record_done`.
    ///
    /// Flushed and `fsync`ed before returning: the caller attempts the
    /// operation next, and a manifest entry still in a buffer when the power
    /// goes is a file nobody can find again.
    pub fn record_pending(
        &mut self,
        src: &Path,
        dst: &Path,
        mechanism: Mechanism,
        mode: Mode,
    ) -> io::Result<u64> {
        self.seq += 1;
        // The source's identity, which is all that is knowable before the
        // operation. A rename preserves the inode, so for that mechanism this
        // alone identifies the destination afterwards.
        let (size, inode) = match std::fs::symlink_metadata(src) {
            Ok(meta) => (meta.len().to_string(), meta.ino().to_string()),
            Err(_) => (String::new(), String::new()),
        };
        writeln!(
            self.file,
            "{},pending,{},{},{},{},{size},{inode}",
            self.seq,
            field(mode_name(mode)),
            field(mechanism_name(mechanism)),
            path_field(src),
            path_field(dst),
        )?;
        self.file.sync_all()?;
        Ok(self.seq)
    }

    /// The destination's identity, now that it exists. `reverse` refuses any
    /// destination that does not match — without it, `reverse` deleted or
    /// relocated whatever happened to be at the path, including a file the
    /// tool never created.
    pub fn record_done(&mut self, seq: u64, destination: &Path) -> io::Result<()> {
        let (size, inode) = match std::fs::symlink_metadata(destination) {
            Ok(meta) => (meta.len().to_string(), meta.ino().to_string()),
            Err(_) => (String::new(), String::new()),
        };
        writeln!(self.file, "{seq},done,,,,,{size},{inode}")?;
        self.file.sync_all()?;
        Ok(())
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

/// `<manifest>.lock`. Task 11 says the lock is taken by `undo` too; it was
/// not, so an `apply` and an `undo` of one plan could interleave.
struct UndoLock {
    path: PathBuf,
}

impl UndoLock {
    fn acquire(manifest: &Path) -> Result<UndoLock, UndoError> {
        let mut name = manifest.file_name().unwrap_or_default().to_os_string();
        name.push(".lock");
        let path = manifest.with_file_name(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(UndoLock { path }),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(UndoError::Locked),
            Err(e) => Err(UndoError::Io(e)),
        }
    }
}

impl Drop for UndoLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Clone, Debug)]
struct Entry {
    seq: u64,
    mode: Mode,
    mechanism: Mechanism,
    source: PathBuf,
    destination: PathBuf,
    /// The source as it was before the operation.
    source_identity: Option<(u64, u64)>,
    /// The destination as it was after, when a `done` row was reached.
    destination_identity: Option<(u64, u64)>,
}

impl Entry {
    /// Is the file at the destination the one this row put there?
    ///
    /// A `done` row carries the destination's own identity and is exact. A
    /// `pending`-only row is the crash case: a rename preserves the inode, so
    /// the source's identity still answers it; a clone or a stream changes
    /// the inode, leaving only the size.
    fn owns(&self, actual: (u64, u64)) -> bool {
        if let Some(recorded) = self.destination_identity {
            return actual == recorded;
        }
        match (self.source_identity, self.mechanism) {
            (Some(recorded), Mechanism::Rename) => actual == recorded,
            (Some((size, _)), _) => actual.0 == size,
            (None, _) => false,
        }
    }
}

/// `reverse` of a copy **deletes the destination**; of a move it **moves it
/// back**. Backwards clobbers a kept original. Strips the `'` prefix, refuses
/// a reoccupied original, and is idempotent.
///
/// Every row is independent. A row it cannot reverse is reported and the rest
/// continue — aborting the manifest on the first problem left every earlier,
/// successful row stranded, and re-running hit the same row forever.
pub fn reverse(manifest: &Path) -> Result<UndoReport, UndoError> {
    let _lock = UndoLock::acquire(manifest)?;
    let (root, entries, torn) = read_manifest(manifest)?;
    let mut report = UndoReport::default();
    if let Some(tail) = torn {
        // A torn final line is the crash this manifest exists for. It used to
        // abandon every completed row in the file.
        report
            .refused
            .push(format!("ignored an incomplete final record: {tail}"));
    }

    // Newest first: a run's effects unwind in the order they were applied.
    for entry in entries.into_iter().rev() {
        match reverse_one(&entry, &root) {
            Ok(true) => report.restored += 1,
            Ok(false) => report.skipped += 1,
            Err(why) => report.refused.push(format!("row {}: {why}", entry.seq)),
        }
    }
    Ok(report)
}

fn reverse_one(entry: &Entry, root: &Path) -> Result<bool, String> {
    if !entry.destination.starts_with(root) {
        return Err(format!(
            "destination {} is outside the manifest's root",
            entry.destination.display()
        ));
    }

    // `Path::exists()` answers false on EACCES, so a permission problem used
    // to read as "already reversed" and `reverse` reported success.
    let meta = match std::fs::symlink_metadata(&entry.destination) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("cannot examine {}: {e}", entry.destination.display())),
    };

    if !entry.owns((meta.len(), meta.ino())) {
        return Err(format!(
            "{} is not the file this run put there",
            entry.destination.display()
        ));
    }

    match entry.mode {
        // The original was kept, so there is nothing to put back.
        Mode::Copy => {
            std::fs::remove_file(&entry.destination).map_err(|e| e.to_string())?;
            Ok(true)
        }
        Mode::Move => {
            let parent = entry
                .source
                .parent()
                .ok_or_else(|| "source has no parent".to_string())?;
            let name = Path::new(
                entry
                    .source
                    .file_name()
                    .ok_or_else(|| "source has no filename".to_string())?,
            );

            if let Ok(existing) = std::fs::symlink_metadata(&entry.source) {
                // The move's own copy is back already, so this is idempotent
                // rather than a conflict.
                if entry.owns((existing.len(), existing.ino())) {
                    return Ok(false);
                }
                // Both the destination and the original exist. For a move
                // that means the unlink failed after a verified copy, leaving
                // a duplicate; removing the copy is provably safe and is the
                // only way to clear it. Anything else is a squatter.
                if entry.mechanism != Mechanism::Rename
                    && entry.source_identity == Some((existing.len(), existing.ino()))
                {
                    std::fs::remove_file(&entry.destination).map_err(|e| e.to_string())?;
                    return Ok(true);
                }
                return Err(format!("{} is occupied", entry.source.display()));
            }

            match rename_excl(&entry.destination, parent, name) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    Err(format!("{} is occupied", entry.source.display()))
                }
                // A cross-volume move was executed by streaming, so it has to
                // be reversed by streaming. Reversing it with `rename` failed
                // EXDEV on exactly the runs where the move was most expensive.
                Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                    stream_verified(&entry.destination, parent, name).map_err(|e| e.to_string())?;
                    std::fs::remove_file(&entry.destination).map_err(|e| e.to_string())?;
                    Ok(true)
                }
                Err(e) => Err(e.to_string()),
            }
        }
    }
}

type Manifest = (PathBuf, Vec<Entry>, Option<String>);

fn read_manifest(manifest: &Path) -> Result<Manifest, UndoError> {
    let text = std::fs::read_to_string(manifest)?;
    let (records, torn) = parse_records(&text);

    let mut root: Option<PathBuf> = None;
    let mut pending: Vec<Entry> = Vec::new();
    let mut done: Vec<(u64, Option<(u64, u64)>)> = Vec::new();

    for (index, columns) in records.iter().enumerate().skip(1) {
        if columns.iter().all(|c| c.is_empty()) {
            continue;
        }
        let fail = |message: &str| UndoError::Parse {
            line: index + 1,
            message: message.to_string(),
        };
        if columns.len() < 8 {
            return Err(fail("expected eight columns"));
        }
        let identity = match (columns[6].parse::<u64>(), columns[7].parse::<u64>()) {
            (Ok(size), Ok(inode)) => Some((size, inode)),
            _ => None,
        };
        match columns[1].as_str() {
            "root" => root = Some(PathBuf::from(unprefix(&columns[5]))),
            "done" => done.push((
                columns[0].parse().map_err(|_| fail("sequence is not a number"))?,
                identity,
            )),
            "pending" => pending.push(Entry {
                seq: columns[0].parse().map_err(|_| fail("sequence is not a number"))?,
                mode: match columns[2].as_str() {
                    "move" => Mode::Move,
                    "copy" => Mode::Copy,
                    other => return Err(fail(&format!("unknown mode {other:?}"))),
                },
                mechanism: match columns[3].as_str() {
                    "rename" => Mechanism::Rename,
                    "clone" => Mechanism::Clone,
                    "stream" => Mechanism::Stream,
                    other => return Err(fail(&format!("unknown mechanism {other:?}"))),
                },
                source: PathBuf::from(unprefix(&columns[4])),
                destination: PathBuf::from(unprefix(&columns[5])),
                source_identity: identity,
                destination_identity: None,
            }),
            other => return Err(fail(&format!("unknown status {other:?}"))),
        }
    }

    for entry in &mut pending {
        if let Some((_, identity)) = done.iter().find(|(seq, _)| *seq == entry.seq) {
            entry.destination_identity = *identity;
        }
    }
    Ok((root.ok_or(UndoError::NoRoot)?, pending, torn))
}

/// RFC 4180 **records**, not lines. `str::lines` splits inside a quoted
/// field, so a filename containing a newline — legal on APFS, and written
/// correctly by the writer — made the whole run unreadable.
///
/// Returns the complete records and, separately, any trailing fragment: a
/// torn final line is the crash this file exists to survive, not a reason to
/// abandon every record before it.
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
    // Anything left was not terminated by a newline.
    if quoted || !current.is_empty() || !fields.is_empty() {
        fields.push(current);
        let tail = fields.join(",");
        return (records, Some(tail));
    }
    (records, None)
}
