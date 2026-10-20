//! Shared test fixtures. Lives here because each `tests/*.rs` is its own
//! binary and cannot `use` a sibling.
//!
//! Task 8 adds the sparse-file generators and `TreeGuard` alongside these.

#![allow(dead_code)] // each test binary uses a different subset

use comic_renamer_core::corpus::{Corpus, Manifest, SCHEMA_VERSION};
use std::path::Path;
use tempfile::TempDir;

/// `(name, name_key, publisher, language, year_began, run)`.
pub type Row<'a> = (&'a str, &'a str, &'a str, &'a str, u16, u32);

pub fn write_db(path: &Path, rows: &[Row<'_>]) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(&format!(
        "PRAGMA user_version = {SCHEMA_VERSION};
         CREATE TABLE series (
             name        TEXT    NOT NULL,
             name_key    TEXT    NOT NULL,
             publisher   TEXT    NOT NULL,
             language    TEXT    NOT NULL,
             year_began  INTEGER NOT NULL,
             issue_count INTEGER NOT NULL
         );"
    ))
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
        schema_version: SCHEMA_VERSION,
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
