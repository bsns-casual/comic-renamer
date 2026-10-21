//! Task 3. No test touches the network: every manifest URL is a `file://`
//! pointing inside the test's own tempdir.

mod common;

use comic_renamer_core::corpus::{ensure_corpus, load_manifest, Corpus, Manifest};
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::resolve::resolve;
use comic_renamer_core::types::{Basis, CorpusError, PublisherHint, Status};
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

/// A release-shaped archive: one deflated `.sqlite` member, as the real one
/// has. `entry_name` is a parameter so zip-slip can be exercised.
fn zip_containing(dir: &Path, entry_name: &str, body: &[u8]) -> PathBuf {
    use std::io::Write;
    let path = dir.join("release.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    writer
        .start_file(entry_name, zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(body).unwrap();
    writer.finish().unwrap();
    path
}

#[test]
fn installs_a_corpus_whose_hash_matches() {
    // The other half of the pair: verification must not refuse everything.
    let dir = TempDir::new().unwrap();
    let source = zip_containing(dir.path(), "gcd.sqlite", b"pretend corpus");
    let m = manifest_for(&source, 1);
    let cache = TempDir::new().unwrap();
    let installed = ensure_corpus(&m, cache.path()).unwrap();
    assert_eq!(installed, cache.path().join("gcd.sqlite"));
    // The database, not the archive: the release ships a 33.7 MB zip holding
    // a 97.7 MB database, so installing the bytes verbatim installs a zip.
    assert_eq!(std::fs::read(&installed).unwrap(), b"pretend corpus");
}

#[test]
fn an_archive_without_a_database_is_refused() {
    let dir = TempDir::new().unwrap();
    let source = zip_containing(dir.path(), "README.txt", b"nothing useful");
    let m = manifest_for(&source, 1);
    let cache = TempDir::new().unwrap();
    assert!(matches!(
        ensure_corpus(&m, cache.path()),
        Err(CorpusError::Archive(_))
    ));
    assert!(!cache.path().join("gcd.sqlite").exists());
    assert!(!cache.path().join("gcd.sqlite.partial").exists());
}

#[test]
fn a_zip_slip_entry_name_cannot_steer_the_write() {
    // A member named ../../escape.sqlite must still land at the cache path.
    let dir = TempDir::new().unwrap();
    let source = zip_containing(dir.path(), "../../escape.sqlite", b"payload");
    let m = manifest_for(&source, 1);
    let cache = TempDir::new().unwrap();
    let nested = cache.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();

    let installed = ensure_corpus(&m, &nested).unwrap();
    assert_eq!(installed, nested.join("gcd.sqlite"));
    assert!(!cache.path().join("escape.sqlite").exists());
}

/// End-to-end against the real release. Ignored because it needs the 33.7 MB
/// artifact; point `COMIC_RENAMER_TEST_ZIP` at a downloaded copy and run with
/// `--ignored` to exercise ensure_corpus and Corpus::open on the real thing.
#[test]
#[ignore]
fn the_real_release_installs_and_opens() {
    let Ok(zip_path) = std::env::var("COMIC_RENAMER_TEST_ZIP") else {
        panic!("set COMIC_RENAMER_TEST_ZIP to a downloaded gcd-*.zip");
    };
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/manifest.json");
    let mut manifest = load_manifest(&repo).unwrap();
    manifest.url = format!("file://{zip_path}");

    let cache = TempDir::new().unwrap();
    let db = ensure_corpus(&manifest, cache.path()).expect("real release must install");
    let corpus = Corpus::open(&db, &manifest).expect("real corpus must open");

    // Nine records, one title, five languages — the case §6.3 is built
    // around, and bigger than the spec's three-record example.
    assert_eq!(corpus.lookup("velvet").len(), 9);
    // The punctuation-expansion variant the index is supposed to carry (D16).
    assert!(!corpus.lookup("hackandslash").is_empty());

    // The ladder on real data: the language filter takes nine records to the
    // three English ones, then shortest feasible run takes Image 2014.
    let overrides = Overrides::default();
    let resolved = resolve(
        "velvet_vol1",
        "",
        &PublisherHint(None),
        &corpus,
        &overrides,
        "en",
    );
    assert_eq!(resolved.status, Status::Ready(Basis::ShortestRun));
    let record = resolved.record.unwrap();
    assert_eq!(
        (record.name.as_str(), record.publisher.as_str(), record.year_began),
        ("Velvet", "Image", 2014)
    );

    // The boundary-guard case the kill test found: a naive longest-prefix
    // match resolves this to the series *Witch*.
    let witcher = resolve(
        "witcheromnibus",
        "",
        &PublisherHint(None),
        &corpus,
        &overrides,
        "en",
    );
    assert_eq!(witcher.series, "The Witcher Omnibus");
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

/// The schema the real extract ships, recorded from a live run in
/// docs/research/2026-10-19-kill-test-result.md:
/// `series(id, name, name_key, year_began, year_ended, publisher, language,
/// issue_count)` — two columns more than we read, in a different order, and
/// with no `PRAGMA user_version` set.
fn write_upstream_shaped_db(path: &Path, rows: usize) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
         INSERT INTO meta VALUES ('schema_version', '1');
         CREATE TABLE series (
             id          INTEGER PRIMARY KEY,
             name        TEXT    NOT NULL,
             name_key    TEXT    NOT NULL,
             year_began  INTEGER,
             year_ended  INTEGER,
             publisher   TEXT,
             language    TEXT,
             issue_count INTEGER
         );
         CREATE TABLE issues (id INTEGER PRIMARY KEY, series_id INTEGER);",
    )
    .unwrap();
    for i in 0..rows {
        conn.execute(
            "INSERT INTO series (name, name_key, year_began, year_ended, publisher, language, issue_count)
             VALUES (?1, ?2, 2004, NULL, 'Image', 'en', 6)",
            rusqlite::params![format!("Series {i}"), format!("series{i}")],
        )
        .unwrap();
    }
}

#[test]
fn opens_the_schema_the_real_extract_ships() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("gcd.sqlite");
    write_upstream_shaped_db(&db, 3);

    // The manifest pins the *zip*: 33.7 MB against a 97.7 MB database. A
    // Corpus::open that compares the db file against sizeBytes rejects every
    // real corpus, and the local fixtures hid it by making the two equal.
    let mut manifest = manifest_for(&db, 3);
    manifest.size_bytes = 35_341_426;
    manifest.schema_version = 1;

    let corpus = Corpus::open(&db, &manifest).expect("real-shaped corpus must open");
    assert_eq!(corpus.lookup("series1").len(), 1);
    assert_eq!(corpus.publishers(), ["Image"]);
}

#[test]
fn rejects_a_database_missing_a_column_we_read() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("gcd.sqlite");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE series (id INTEGER, name TEXT, name_key TEXT);")
        .unwrap();
    drop(conn);
    let manifest = manifest_for(&db, 0);
    assert!(Corpus::open(&db, &manifest).is_err());
}
