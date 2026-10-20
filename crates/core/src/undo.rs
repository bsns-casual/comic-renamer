//! Append-only. A `pending` row lands before the operation is attempted and a
//! `done` row after; `reverse` reads the last status per id. In-place status
//! rewriting is impossible anyway — "pending" is 7 bytes and "done" is 4.
//! RFC 4180 quoting, doubled embedded quotes.

use crate::types::{Mechanism, Mode, UndoError};
use std::io;
use std::path::Path;

pub struct UndoWriter {
    file: std::fs::File,
    seq: u64,
}

impl UndoWriter {
    /// `O_EXCL`: re-running a plan must not truncate the previous run's
    /// manifest, the only record of where those files went.
    pub fn create(path: &Path) -> io::Result<UndoWriter> {
        todo!()
    }

    /// Returns the row's sequence number, for the matching `record_done`.
    pub fn record_pending(
        &mut self,
        src: &Path,
        dst: &Path,
        mechanism: Mechanism,
        mode: Mode,
    ) -> io::Result<u64> {
        todo!()
    }

    pub fn record_done(&mut self, seq: u64) -> io::Result<()> {
        todo!()
    }
}

/// `reverse` of a copy **deletes the destination**; of a move it **moves it
/// back**. Backwards clobbers a kept original. Strips the `'` formula-defusal
/// prefix, refuses a reoccupied original, and is idempotent.
pub fn reverse(manifest: &Path) -> Result<(), UndoError> {
    todo!()
}
