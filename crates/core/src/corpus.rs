//! The pinned corpus. Never committed (invariant 4); fetched and hash-verified.

use crate::types::{CorpusError, SeriesRecord};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub series_count: u64,
    pub schema_version: u32,
    pub licence: String,
}

pub fn load_manifest(path: &Path) -> Result<Manifest, CorpusError> {
    todo!()
}

/// Verifies size and SHA-256 **before** moving into the cache, so a failed
/// fetch leaves nothing behind.
pub fn ensure_corpus(manifest: &Manifest, cache_dir: &Path) -> Result<PathBuf, CorpusError> {
    todo!()
}

pub struct Corpus {
    conn: rusqlite::Connection,
    // shortcut: clones per key variant; switch to Vec<SeriesRecord> +
    // HashMap<String, Vec<u32>> if startup exceeds a second
    index: HashMap<String, Vec<SeriesRecord>>,
}

impl Corpus {
    /// Validates `series_count`, `size_bytes` and `schema_version` against the
    /// manifest, so a truncation test cannot pass by luck of the cut point.
    pub fn open(db: &Path, manifest: &Manifest) -> Result<Corpus, CorpusError> {
        todo!()
    }

    pub fn lookup(&self, key: &str) -> &[SeriesRecord] {
        todo!()
    }
}
