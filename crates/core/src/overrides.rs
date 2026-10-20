//! Layer B: the only place a wrong *title* is correctable, and the floor under
//! the unmatched tail (§6.2.1). Rule data never holds anything path-shaped
//! (invariant 1).

use crate::types::OverrideError;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrideEntry {
    pub series: String,
    pub volume: Option<u32>,
    pub subtitle: Option<String>,
    /// Required: a bare filename key recurs across publishers. Matched by
    /// `squash(bundle)` being a substring of `squash(source_folder_name)`.
    pub bundle: String,
}

#[derive(Clone, Debug, Default)]
pub struct Overrides {
    entries: Vec<(String, OverrideEntry)>,
}

impl Overrides {
    pub fn load(path: &Path) -> Result<Overrides, OverrideError> {
        todo!()
    }

    pub fn lookup(&self, filename: &str, source_folder: &str) -> Option<&OverrideEntry> {
        todo!()
    }
}
