//! The pinned corpus. Never committed (invariant 4); fetched and hash-verified.

use crate::normalize::key_variants;
use crate::types::{CorpusError, SeriesRecord};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The corpus schema this build reads.
pub const SCHEMA_VERSION: u32 = 1;

/// The cache filename. `ensure_corpus` installs here and `Corpus::open` reads it.
pub const CACHE_FILENAME: &str = "gcd.sqlite";

/// Mirrors `corpus/manifest.json`, which `scripts/check-manifest.py` guards.
/// `deny_unknown_fields` so a typo in the file is an error, not a silent default.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Manifest {
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub series_count: u64,
    pub schema_version: u32,
    pub licence: String,
    pub dump_date: String,
    pub attribution: String,
    pub upstream_manifest: String,
    pub licence_url: Option<String>,
}

pub fn load_manifest(path: &Path) -> Result<Manifest, CorpusError> {
    let text = std::fs::read_to_string(path)?;
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|e| CorpusError::Manifest(e.to_string()))?;
    // D3/D9: every viable corpus source is share-alike, so a manifest that
    // claims otherwise is either wrong or pointing somewhere it should not.
    if !manifest.licence.starts_with("CC BY-SA") {
        return Err(CorpusError::Licence(manifest.licence));
    }
    if manifest.sha256.len() != 64 || !manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CorpusError::Manifest(format!(
            "sha256 {:?} is not 64 hex characters",
            manifest.sha256
        )));
    }
    Ok(manifest)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Verifies size and SHA-256 **before** moving into the cache, so a failed
/// fetch leaves nothing behind.
///
/// `file://` URLs are read from disk. That keeps the whole verify-then-install
/// path testable without a network, and lets a user prime an air-gapped cache.
pub fn ensure_corpus(manifest: &Manifest, cache_dir: &Path) -> Result<PathBuf, CorpusError> {
    let target = cache_dir.join(CACHE_FILENAME);
    if let Ok(cached) = std::fs::read(&target) {
        if cached.len() as u64 == manifest.size_bytes
            && sha256_hex(&cached) == manifest.sha256
        {
            return Ok(target);
        }
    }

    // shortcut: the pinned release URL is a .zip and this installs the bytes
    // verbatim, so a real fetch against corpus/manifest.json still needs an
    // unzip step between fetch and verify. No archive crate is in the pinned
    // dependency set; Task 14 is the first thing that needs it.
    let body = fetch(&manifest.url)?;

    if body.len() as u64 != manifest.size_bytes {
        return Err(CorpusError::SizeMismatch {
            expected: manifest.size_bytes,
            actual: body.len() as u64,
        });
    }
    let actual = sha256_hex(&body);
    if actual != manifest.sha256 {
        return Err(CorpusError::HashMismatch {
            expected: manifest.sha256.clone(),
            actual,
        });
    }

    // Verified before anything lands: write beside the target, then rename, so
    // an interrupted write cannot leave a half corpus under the real name.
    std::fs::create_dir_all(cache_dir)?;
    let staging = cache_dir.join(format!("{CACHE_FILENAME}.partial"));
    std::fs::write(&staging, &body)?;
    std::fs::rename(&staging, &target)?;
    Ok(target)
}

fn fetch(url: &str) -> Result<Vec<u8>, CorpusError> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(Into::into);
    }
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| CorpusError::Fetch(e.to_string()))?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|e| CorpusError::Fetch(e.to_string()))
}

pub struct Corpus {
    #[allow(dead_code)] // held so the index's backing file stays open
    conn: rusqlite::Connection,
    // shortcut: clones per key variant; switch to Vec<SeriesRecord> +
    // HashMap<String, Vec<u32>> if startup exceeds a second
    index: HashMap<String, Vec<SeriesRecord>>,
    /// Distinct publishers, for `publisher_hint` to match a folder name against.
    publishers: Vec<String>,
}

impl Corpus {
    /// Validates `series_count`, `size_bytes` and `schema_version` against the
    /// manifest, so a truncation test cannot pass by luck of the cut point.
    pub fn open(db: &Path, manifest: &Manifest) -> Result<Corpus, CorpusError> {
        let size = std::fs::metadata(db)?.len();
        if size != manifest.size_bytes {
            return Err(CorpusError::SizeMismatch {
                expected: manifest.size_bytes,
                actual: size,
            });
        }

        let conn = rusqlite::Connection::open(db)?;

        let schema: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if schema != manifest.schema_version || schema != SCHEMA_VERSION {
            return Err(CorpusError::SchemaVersion {
                expected: SCHEMA_VERSION,
                actual: schema,
            });
        }

        // sqlite integers are i64; rusqlite has no FromSql for u64.
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM series", [], |r| r.get(0))?;
        let count = count.max(0) as u64;
        if count != manifest.series_count {
            return Err(CorpusError::CountMismatch {
                expected: manifest.series_count,
                actual: count,
            });
        }

        // §6.1: one combined index over squash(name), name_key and the
        // punctuation-expansion variants. Indexing them separately resolved
        // `witcheromnibus` to the series *Witch*.
        let mut index: HashMap<String, Vec<SeriesRecord>> = HashMap::new();
        let mut statement = conn.prepare(
            "SELECT name, name_key, publisher, language, year_began, issue_count FROM series",
        )?;
        let rows = statement.query_map([], |r| {
            Ok((
                SeriesRecord {
                    name: r.get(0)?,
                    publisher: r.get(2)?,
                    language: r.get(3)?,
                    year_began: r.get(4)?,
                    run: r.get(5)?,
                },
                r.get::<_, String>(1)?,
            ))
        })?;
        for row in rows {
            let (record, name_key) = row?;
            let mut keys = key_variants(&record.name);
            keys.insert(name_key);
            for key in keys {
                if key.is_empty() {
                    continue;
                }
                index.entry(key).or_default().push(record.clone());
            }
        }
        drop(statement);

        let mut publishers: Vec<String> = conn
            .prepare("SELECT DISTINCT publisher FROM series")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        publishers.sort();

        Ok(Corpus {
            conn,
            index,
            publishers,
        })
    }

    pub fn lookup(&self, key: &str) -> &[SeriesRecord] {
        self.index.get(key).map_or(&[], Vec::as_slice)
    }

    pub fn publishers(&self) -> &[String] {
        &self.publishers
    }
}
