//! Shared test fixtures. Lives here because each `tests/*.rs` is its own
//! binary and cannot `use` a sibling.
//!
//! Task 8 adds the sparse-file generators and `TreeGuard` alongside these.

#![allow(dead_code)] // each test binary uses a different subset

use comic_renamer_core::corpus::{Corpus, Manifest, CORPUS_SCHEMA_VERSION};
use std::path::Path;
use tempfile::TempDir;

/// `(name, name_key, publisher, language, year_began, run)`.
pub type Row<'a> = (&'a str, &'a str, &'a str, &'a str, u16, u32);

pub fn write_db(path: &Path, rows: &[Row<'_>]) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
         INSERT INTO meta VALUES ('schema_version', '1');
         CREATE TABLE series (
             name        TEXT    NOT NULL,
             name_key    TEXT    NOT NULL,
             publisher   TEXT    NOT NULL,
             language    TEXT    NOT NULL,
             year_began  INTEGER NOT NULL,
             issue_count INTEGER NOT NULL
         );",
    )
    .unwrap();
    for (name, name_key, publisher, language, year, run) in rows {
        conn.execute(
            "INSERT INTO series VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![name, name_key, publisher, language, year, run],
        )
        .unwrap();
    }
}

/// A manifest that agrees with the file at `path`.
pub fn manifest_for(path: &Path, series_count: u64) -> Manifest {
    Manifest {
        url: format!("file://{}", path.display()),
        sha256: comic_renamer_core::corpus::sha256_hex(&std::fs::read(path).unwrap()),
        size_bytes: std::fs::metadata(path).unwrap().len(),
        series_count,
        schema_version: CORPUS_SCHEMA_VERSION,
        licence: "CC BY-SA 4.0".into(),
        dump_date: "2026-09-15".into(),
        attribution: "Grand Comics Database (https://www.comics.org)".into(),
        upstream_manifest: "https://example.invalid/gcd-data.json".into(),
        licence_url: None,
    }
}

/// The `TempDir` comes back with the corpus: `Corpus` holds an open handle on
/// a file that has to outlive it, so a fixture cannot be a bare value.
pub fn corpus(rows: &[Row<'_>]) -> (TempDir, Corpus) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gcd.sqlite");
    write_db(&path, rows);
    let manifest = manifest_for(&path, rows.len() as u64);
    let corpus = Corpus::open(&path, &manifest).unwrap();
    (dir, corpus)
}

// ---------------------------------------------------------------------------
// Task 8: fixtures and the unchanged-tree harness.
//
// Tests use generated files, never anyone's library (D12).
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// `seek` then write: a large `st_size` with **non-zero** `st_blocks`. Drives
/// the streaming path.
///
/// Not interchangeable with `sparse_truncate_only` — §7 keeps both because
/// they measure different things, and one cannot do both jobs.
pub fn sparse_seek_write(dir: &Path, name: &str, size: u64) -> PathBuf {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    file.seek(SeekFrom::Start(size - 1)).unwrap();
    file.write_all(b"\0").unwrap();
    file.sync_all().unwrap();
    path
}

/// `truncate` only: a large `st_size` with `st_blocks == 0`. Exists to prove
/// the dataless predicate does *not* fire on it (invariant 8, D25).
pub fn sparse_truncate_only(dir: &Path, name: &str, size: u64) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(size).unwrap();
    file.sync_all().unwrap();
    path
}

/// Size zero is never an error (invariant 3, D21) — the whole fixture suite is
/// built from these.
pub fn zero_byte(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::File::create(&path).unwrap();
    path
}

pub fn blocks(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().blocks()
}

/// `(len, mtime_ns, sha256 of the first 4 KiB)`, or `None` for a directory.
type Fingerprint = Option<(u64, i64, String)>;

fn fingerprint(path: &Path) -> Fingerprint {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_dir() {
        return None;
    }
    // A same-size rewrite has to be caught, so the content is sampled too.
    let head = std::fs::read(path).map(|b| b[..b.len().min(4096)].to_vec());
    Some((
        meta.len(),
        meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
        comic_renamer_core::corpus::sha256_hex(&head.unwrap_or_default()),
    ))
}

fn snapshot(roots: &[&Path]) -> BTreeMap<PathBuf, Fingerprint> {
    let mut out = BTreeMap::new();
    for root in roots {
        walk(root, &mut out);
    }
    out
}

fn walk(path: &Path, out: &mut BTreeMap<PathBuf, Fingerprint>) {
    out.insert(path.to_path_buf(), fingerprint(path));
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        walk(&entry.path(), out);
    }
}

/// Snapshots a tree and asserts on `Drop` that nothing changed.
///
/// **Unchanged is the default.** A safety review found every refusal test in
/// an earlier draft asserted only the `Err` variant, so rows 1-3 could execute
/// before row 4 refused and the suite stayed green. "The function returned
/// `Err`" and "no files moved" are not the same claim, and only this makes the
/// second one. A test that means to change something says which paths.
pub struct TreeGuard {
    roots: Vec<PathBuf>,
    before: BTreeMap<PathBuf, Fingerprint>,
    permitted: Vec<PathBuf>,
}

impl TreeGuard {
    pub fn new(roots: &[&Path]) -> TreeGuard {
        TreeGuard {
            roots: roots.iter().map(|p| p.to_path_buf()).collect(),
            before: snapshot(roots),
            permitted: Vec::new(),
        }
    }

    /// Opt in to a change, naming every path that may differ.
    pub fn expect_changed(mut self, paths: &[&Path]) -> TreeGuard {
        self.permitted = paths.iter().map(|p| p.to_path_buf()).collect();
        self
    }
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        // A panic inside Drop while already unwinding aborts the process and
        // hides the real failure.
        if std::thread::panicking() {
            return;
        }
        let roots: Vec<&Path> = self.roots.iter().map(PathBuf::as_path).collect();
        let after = snapshot(&roots);

        let mut unexpected: Vec<String> = Vec::new();
        let mut paths: Vec<&PathBuf> = self.before.keys().chain(after.keys()).collect();
        paths.sort();
        paths.dedup();
        for path in paths {
            if self.permitted.contains(path) {
                continue;
            }
            let (before, after) = (self.before.get(path), after.get(path));
            if before != after {
                unexpected.push(match (before, after) {
                    (None, Some(_)) => format!("appeared: {}", path.display()),
                    (Some(_), None) => format!("vanished: {}", path.display()),
                    _ => format!("modified: {}", path.display()),
                });
            }
        }
        assert!(
            unexpected.is_empty(),
            "unexpected change to the tree:\n  {}",
            unexpected.join("\n  ")
        );
    }
}
