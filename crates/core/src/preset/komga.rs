//! Flat `<Series>/<Book>.<ext>` — Komga reads only the leaf folder (§6.8.1).

use super::Preset;
use crate::types::Resolved;
use std::path::PathBuf;

pub struct Komga;

impl Preset for Komga {
    fn id(&self) -> &'static str {
        "komga"
    }

    fn destination(&self, resolved: &Resolved, ext: &str) -> PathBuf {
        todo!()
    }
}

/// Zero-pads to the digit count of `run`, floor 2 (D37).
pub fn pad(volume: u32, run: u32) -> String {
    todo!()
}

/// Fixes titles legal as titles and hostile as paths. Distinct titles must
/// never converge — that would silently merge two series. Truncation is at 255
/// **bytes** and must not split a UTF-8 sequence.
pub fn sanitize(component: &str) -> String {
    todo!()
}
