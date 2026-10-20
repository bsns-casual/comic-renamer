//! JSON Lines: header object, then one row per line (§6.9). TSV is ruled out —
//! tabs occur in titles and a leading `=` is formula injection.

use crate::types::{PlanError, PlanHeader, PlanRow};
use std::path::Path;

pub fn write(header: &PlanHeader, rows: &[PlanRow], path: &Path) -> Result<(), PlanError> {
    todo!()
}

/// Validates the whole file and **never** returns a partial row set: one bad
/// row rejects everything. An edited plan is reported as edited, not rejected —
/// `apply` revalidates rather than trusting it.
pub fn read(path: &Path) -> Result<(PlanHeader, Vec<PlanRow>), PlanError> {
    todo!()
}

/// Over the header and every row, so `apply` can tell what a human changed.
pub fn checksum(header: &PlanHeader, rows: &[PlanRow]) -> String {
    todo!()
}
