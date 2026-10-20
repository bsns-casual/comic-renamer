//! Presets are the only code that constructs paths (invariant 1).

pub mod komga;

use crate::types::Resolved;
use std::path::PathBuf;

pub trait Preset {
    fn id(&self) -> &'static str;

    /// Relative to `dest_root`. `apply` joins; `plan::read` validates that the
    /// joined path stays beneath `dest_root`.
    fn destination(&self, resolved: &Resolved, ext: &str) -> PathBuf;
}

/// `apply` resolves `PlanHeader.preset` through this.
pub fn preset_by_id(id: &str) -> Option<Box<dyn Preset>> {
    todo!()
}
