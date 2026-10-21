//! The pinned corpus. Never committed (invariant 4); fetched and hash-verified.

use crate::normalize::key_variants;
use crate::types::{CorpusError, SeriesRecord};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The corpus schema this build reads. The extract publishes its own version
/// in `meta.schema_version`; it leaves `PRAGMA user_version` at 0, so reading
/// the pragma rejects every real corpus.
pub const CORPUS_SCHEMA_VERSION: u32 = 1;

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

    let body = fetch(&manifest.url, manifest.size_bytes)?;

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

    // Verified before anything lands: extract beside the target, then rename,
    // so an interrupted write cannot leave a half corpus under the real name.
    std::fs::create_dir_all(cache_dir)?;
    let staging = cache_dir.join(format!("{CACHE_FILENAME}.partial"));
    let extracted = extract_database(&body, &staging);
    if extracted.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    extracted?;
    std::fs::rename(&staging, &target)?;
    Ok(target)
}

/// Pulls the single `.sqlite` member out of the release archive.
///
/// The entry's own name is never used to build a path. A zip from the network
/// can name a member `../../etc/passwd`, and joining that onto a directory is
/// the zip-slip write primitive; writing to a path we chose makes it moot.
fn extract_database(archive: &[u8], out: &Path) -> Result<(), CorpusError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|e| CorpusError::Archive(e.to_string()))?;

    let index = (0..zip.len())
        .find(|&i| {
            zip.by_index(i)
                .map(|entry| entry.name().ends_with(".sqlite"))
                .unwrap_or(false)
        })
        .ok_or_else(|| CorpusError::Archive("archive holds no .sqlite member".into()))?;

    let mut member = zip
        .by_index(index)
        .map_err(|e| CorpusError::Archive(e.to_string()))?;
    let mut file = std::fs::File::create(out)?;
    std::io::copy(&mut member, &mut file)?;
    file.sync_all()?;
    Ok(())
}

/// `expected` bounds the read. ureq defaults to a 10 MiB body limit, which
/// silently refuses a 33.7 MiB corpus — every test uses `file://`, so only a
/// real fetch found it. Bounding it to the pinned size is also the safer
/// shape: a redirect cannot stream more than the manifest accounts for.
fn fetch(url: &str, expected: u64) -> Result<Vec<u8>, CorpusError> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(Into::into);
    }
    // One spare byte, so an over-long body overshoots the size check rather
    // than being silently truncated to exactly the expected length.
    let limit = expected.saturating_add(1) as usize;
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| CorpusError::Fetch(e.to_string()))?;
    response
        .body_mut()
        .with_config()
        .limit(limit as u64)
        .read_to_vec()
        .map_err(|e| CorpusError::Fetch(e.to_string()))
}

pub struct Corpus {
    #[allow(dead_code)] // held so the index's backing file stays open
    conn: rusqlite::Connection,
    // shortcut: clones per key variant; switch to Vec<SeriesRecord> +
    // HashMap<String, Vec<u32>> if startup exceeds a second. Measured on the
    // real 229,836-series release: 0.69s release-build for verify, extract,
    // index and two resolutions together, so it is well inside budget.
    index: HashMap<String, Vec<SeriesRecord>>,
    /// Distinct publishers, for `publisher_hint` to match a folder name against.
    publishers: Vec<String>,
}

impl Corpus {
    /// Validates the **database**: that it carries the columns we read, and
    /// that its row count matches the manifest, so a short corpus cannot pass
    /// by luck of where it was cut.
    ///
    /// Size and SHA-256 are deliberately not checked here. They pin the
    /// *download* — a 33.7 MB zip against a 97.7 MB database — and
    /// `ensure_corpus` is where they belong. Comparing the database against
    /// `size_bytes` rejects every real corpus.
    pub fn open(db: &Path, manifest: &Manifest) -> Result<Corpus, CorpusError> {
        let conn = rusqlite::Connection::open(db)?;

        // The real extract ships
        // `series(id, name, name_key, year_began, year_ended, publisher,
        // language, issue_count)`: two columns more than we read, in another
        // order. Selecting by name tolerates both, and `prepare` failing is
        // the schema check — a missing column errors here, before any work.
        let mut statement = conn.prepare(
            "SELECT name, name_key, publisher, language, year_began, issue_count FROM series",
        )?;

        // The extract carries its own version in `meta`, alongside its licence
        // and attribution. `PRAGMA user_version` is 0 on the real corpus.
        let schema: u32 = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if schema != CORPUS_SCHEMA_VERSION || schema != manifest.schema_version {
            return Err(CorpusError::SchemaVersion {
                expected: CORPUS_SCHEMA_VERSION,
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
        let rows = statement.query_map([], |r| {
            Ok((
                // publisher, language and year_began are nullable in the
                // schema. This release has no NULLs, but a later dump may.
                SeriesRecord {
                    name: r.get(0)?,
                    publisher: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    language: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    year_began: r.get::<_, Option<i64>>(4)?.unwrap_or(0).clamp(0, 65535) as u16,
                    run: r.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0) as u32,
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
