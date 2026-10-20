//! Task 3. No test touches the network: every manifest URL is a `file://`
//! pointing inside the test's own tempdir.

mod common;

use comic_renamer_core::corpus::{ensure_corpus, load_manifest, Corpus, Manifest};
use comic_renamer_core::types::CorpusError;
use common::{manifest_for, write_db};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn manifest_json(licence: &str) -> String {
    format!(
        r#"{{
  "schemaVersion": 1,
  "dumpDate": "2026-09-15",
  "url": "https://example.invalid/gcd-2026-09-15.zip",
  "sha256": "{zeros}",
  "sizeBytes": 1,
  "seriesCount": 1,
  "upstreamManifest": "https://example.invalid/gcd-data.json",
  "attribution": "Grand Comics Database (https://www.comics.org)",
  "licence": "{licence}",
  "licenceUrl": "https://creativecommons.org/licenses/by-sa/4.0/"
}}"#,
        zeros = "0".repeat(64),
    )
}

fn manifest_with_licence(licence: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("manifest.json");
    std::fs::write(&path, manifest_json(licence)).unwrap();
    (dir, path)
}

/// A cache dir holding a source file the manifest points at, pinned to a hash
/// the file does not have.
fn manifest_with_wrong_hash() -> (TempDir, Manifest) {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("source.bin");
    let body = b"not the corpus";
    std::fs::write(&source, body).unwrap();
    let mut manifest = manifest_for(&source, 1);
    manifest.sha256 = "0".repeat(64);
    manifest.size_bytes = body.len() as u64;
    (dir, manifest)
}

fn tiny_corpus(dir: &TempDir, rows: &[common::Row<'_>]) -> Corpus {
    let path = dir.path().join("gcd.sqlite");
    write_db(&path, rows);
    let manifest = manifest_for(&path, rows.len() as u64);
    Corpus::open(&path, &manifest).unwrap()
}

#[test]
fn rejects_hash_mismatch_and_leaves_no_cache() {
    // Review Focus 4
    let (dir, m) = manifest_with_wrong_hash();
    assert!(matches!(
        ensure_corpus(&m, dir.path()),
        Err(CorpusError::HashMismatch { .. })
    ));
    assert!(!dir.path().join("gcd.sqlite").exists());
}

#[test]
fn installs_a_corpus_whose_hash_matches() {
    // The other half of the pair: verification must not refuse everything.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("source.bin");
    std::fs::write(&source, b"pretend corpus").unwrap();
    let m = manifest_for(&source, 1);
    let cache = TempDir::new().unwrap();
    let installed = ensure_corpus(&m, cache.path()).unwrap();
    assert_eq!(installed, cache.path().join("gcd.sqlite"));
    assert_eq!(std::fs::read(&installed).unwrap(), b"pretend corpus");
}

#[test]
fn rejects_a_corpus_whose_row_count_disagrees() {
    // Review Focus 4. The manifest agrees on size and schema, so only the
    // count can fire — otherwise the test passes by luck of the cut point.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gcd.sqlite");
    write_db(&path, &[("A", "a", "Image", "en", 2004, 1)]);
    let manifest = manifest_for(&path, 229_836);
    assert!(matches!(
        Corpus::open(&path, &manifest),
        Err(CorpusError::CountMismatch { .. })
    ));
}

#[test]
fn indexes_squash_name_key_and_punctuation_variants() {
    let dir = TempDir::new().unwrap();
    let c = tiny_corpus(&dir, &[("Hack/Slash", "hackslash", "Image", "en", 2004, 6)]);
    assert_eq!(c.lookup("hackslash").len(), 1);
    assert_eq!(c.lookup("hackandslash").len(), 1);
    assert!(c.lookup("nosuchseries").is_empty());
}

#[test]
fn manifest_requires_sharealike() {
    let (_d, path) = manifest_with_licence("MIT");
    assert!(load_manifest(&path).is_err());
    let (_d2, ok) = manifest_with_licence("CC BY-SA 4.0");
    assert!(load_manifest(&ok).is_ok());
}

#[test]
fn the_repo_manifest_parses() {
    // The struct and corpus/manifest.json must not drift apart;
    // scripts/check-manifest.py guards the file, nothing guarded the struct.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/manifest.json");
    let loaded = load_manifest(&repo);
    assert!(loaded.is_ok(), "{:?}", loaded.err());
}
