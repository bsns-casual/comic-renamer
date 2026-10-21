//! Append-only. A `pending` row lands before the operation is attempted and a
//! `done` row after; `reverse` reads the last status per id. In-place status
//! rewriting is impossible anyway — "pending" is 7 bytes and "done" is 4.
//! RFC 4180 quoting, doubled embedded quotes.

use crate::fsops::{rename_excl, stream_verified};
use crate::types::{Mechanism, Mode, UndoError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct UndoWriter {
    file: std::fs::File,
    seq: u64,
}

impl UndoWriter {
    /// `O_EXCL`: re-running a plan must not truncate the previous run's
    /// manifest, the only record of where those files went.
    pub fn create(path: &Path) -> io::Result<UndoWriter> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        writeln!(file, "seq,status,mode,mechanism,source,destination")?;
        file.sync_all()?;
        Ok(UndoWriter { file, seq: 0 })
    }

    /// Returns the row's sequence number, for the matching `record_done`.
    ///
    /// Flushed and `fsync`ed before returning: the caller attempts the
    /// operation next, and a manifest entry that is still in a buffer when
    /// the power goes is a file nobody can find again.
    pub fn record_pending(
        &mut self,
        src: &Path,
        dst: &Path,
        mechanism: Mechanism,
        mode: Mode,
    ) -> io::Result<u64> {
        self.seq += 1;
        let row = format!(
            "{},pending,{},{},{},{}",
            self.seq,
            field(mode_name(mode)),
            field(mechanism_name(mechanism)),
            path_field(src),
            path_field(dst),
        );
        writeln!(self.file, "{row}")?;
        self.file.sync_all()?;
        Ok(self.seq)
    }

    pub fn record_done(&mut self, seq: u64) -> io::Result<()> {
        writeln!(self.file, "{seq},done,,,,")?;
        self.file.sync_all()?;
        Ok(())
    }
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

#[derive(Clone, Debug)]
struct Entry {
    mode: Mode,
    #[allow(dead_code)] // recorded for diagnostics; reverse decides by mode
    mechanism: Mechanism,
    source: PathBuf,
    destination: PathBuf,
}

/// `reverse` of a copy **deletes the destination**; of a move it **moves it
/// back**. Backwards clobbers a kept original. Strips the `'` formula-defusal
/// prefix, refuses a reoccupied original, and is idempotent.
pub fn reverse(manifest: &Path) -> Result<(), UndoError> {
    let entries = read_manifest(manifest)?;

    // Last first: a run's effects unwind in the order they were applied.
    for entry in entries.into_iter().rev() {
        // A `pending` row with no `done` is an operation that may or may not
        // have completed — the crash case. Ask the filesystem rather than the
        // manifest, which is exactly why `pending` is written first.
        if !entry.destination.exists() {
            continue;
        }
        match entry.mode {
            // The original was kept, so there is nothing to put back.
            Mode::Copy => std::fs::remove_file(&entry.destination)?,
            Mode::Move => {
                if entry.source.exists() {
                    // Already reversed, or something else moved in. Either
                    // way this must not overwrite it.
                    return Err(UndoError::Reoccupied(entry.source.clone()));
                }
                let parent = entry.source.parent().unwrap_or(Path::new("/"));
                let name = Path::new(
                    entry
                        .source
                        .file_name()
                        .ok_or_else(|| UndoError::Reoccupied(entry.source.clone()))?,
                );
                match rename_excl(&entry.destination, parent, name) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        return Err(UndoError::Reoccupied(entry.source.clone()))
                    }
                    // A cross-volume move was executed by streaming, so it
                    // has to be reversed by streaming. Reversing it with
                    // `rename` fails EXDEV on exactly the runs where the move
                    // was most expensive, leaving the user no way back.
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                        // Same order as the forward direction: the copy is
                        // verified and in place before the original is gone.
                        stream_verified(&entry.destination, parent, name)?;
                        std::fs::remove_file(&entry.destination)?;
                    }
                    Err(e) => return Err(UndoError::Io(e)),
                }
            }
        }
    }
    Ok(())
}

fn read_manifest(manifest: &Path) -> Result<Vec<Entry>, UndoError> {
    let text = std::fs::read_to_string(manifest)?;
    let mut pending: Vec<Entry> = Vec::new();
    let mut done: Vec<u64> = Vec::new();

    for (index, line) in text.lines().enumerate().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let columns = split_csv(line);
        let fail = |message: &str| UndoError::Parse {
            line: index + 1,
            message: message.to_string(),
        };
        if columns.len() < 6 {
            return Err(fail("expected six columns"));
        }
        let seq: u64 = columns[0].parse().map_err(|_| fail("sequence is not a number"))?;
        match columns[1].as_str() {
            "done" => done.push(seq),
            "pending" => pending.push(Entry {
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
            }),
            other => return Err(fail(&format!("unknown status {other:?}"))),
        }
    }
    // `done` rows are the crash-safety record and the human-readable log, but
    // they do not drive the decision: a `pending` row with no `done` is an
    // operation that may or may not have completed, so `reverse` asks the
    // filesystem either way. That is precisely why `pending` is written and
    // fsynced *before* the operation is attempted.
    let _ = done;
    Ok(pending)
}

/// Every path was written with a leading `'`.
fn unprefix(value: &str) -> &str {
    value.strip_prefix('\'').unwrap_or(value)
}

fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}
