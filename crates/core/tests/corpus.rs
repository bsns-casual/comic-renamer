//! Task 3. No test touches the network: every manifest URL is a `file://`
//! pointing inside the test's own tempdir.

mod common;

use comic_renamer_core::corpus::{ensure_corpus, load_manifest, Corpus, Manifest};
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::resolve::{corpus_ladder, publisher_hint, Context};
use comic_renamer_core::types::{CorpusError, PublisherHint, Suggestion};
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

/// The ladder as the old `resolve` was called.
fn resolve(
    stem: &str,
    source_folder: &str,
    hint: &PublisherHint,
    corpus: &Corpus,
    overrides: &Overrides,
    lang: &str,
) -> Option<Vec<Suggestion>> {
    let ctx = Context { source_folder, hint: hint.clone(), overrides: Some(overrides), lang };
    corpus_ladder(stem, corpus, &ctx)
}

/// What the old `Ready` meant: the ladder settled on exactly one answer.
fn settled(found: &Option<Vec<Suggestion>>) -> Option<&Suggestion> {
    match found.as_deref() {
        Some([only]) => Some(only),
        _ => None,
    }
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
    let chosen = settled(&resolved).expect("shortest run settles on one record");
    // Image 2014 is the shortest English run; the suggestion carries its run.
    let image_2014 = corpus
        .lookup("velvet")
        .iter()
        .find(|r| r.language == "en" && r.publisher == "Image" && r.year_began == 2014)
        .expect("Image 2014 is in the real corpus");
    assert_eq!(chosen.series, "Velvet");
    assert_eq!(chosen.volume_count, Some(image_2014.run));

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
    assert_eq!(settled(&witcher).unwrap().series, "The Witcher Omnibus");
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

/// Reports how often the publisher hint fires across the real bundles.
/// Ignored: needs the corpus. Substring matching fired on 2,223 of 2,223
/// ground-truth rows, against D30's measured 7.4% resolution rate — a hint
/// that is always present is not a signal.
#[test]
#[ignore]
fn the_publisher_hint_does_not_fire_on_every_bundle() {
    let Ok(zip_path) = std::env::var("COMIC_RENAMER_TEST_ZIP") else {
        panic!("set COMIC_RENAMER_TEST_ZIP to a downloaded gcd-*.zip");
    };
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/manifest.json");
    let mut manifest = load_manifest(&repo).unwrap();
    manifest.url = format!("file://{zip_path}");
    let cache = TempDir::new().unwrap();
    let db = ensure_corpus(&manifest, cache.path()).unwrap();
    let corpus = Corpus::open(&db, &manifest).unwrap();

    let truth = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/humble-ground-truth.tsv");
    let text = std::fs::read_to_string(&truth).unwrap();
    let mut bundles: Vec<&str> = text
        .lines()
        .skip(1)
        .filter_map(|l| l.split('\t').next())
        .collect();
    bundles.sort_unstable();
    bundles.dedup();

    let hit: Vec<(&str, String)> = bundles
        .iter()
        .filter_map(|b| publisher_hint(b, &corpus).0.map(|p| (*b, p)))
        .collect();
    println!(
        "publisher hint fires on {}/{} bundles",
        hit.len(),
        bundles.len()
    );
    for (bundle, publisher) in &hit {
        println!("  {bundle}  ->  {publisher}");
    }
    assert!(
        hit.len() < bundles.len(),
        "the hint fires on every bundle, which is what substring matching did"
    );
    // Every hint that does fire must be a word of the folder, not a fragment.
    for (bundle, publisher) in &hit {
        let folder = bundle.to_lowercase();
        assert!(
            publisher
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .all(|w| folder.split(|c: char| !c.is_alphanumeric()).any(|f| f == w)),
            "{publisher:?} is not a word of {bundle:?}"
        );
    }
}

/// Does the publisher hint move rows toward or away from ground truth?
/// Ignored: needs the corpus. §6.3 step 2 lets the hint promote a row to
/// `ready`, so a wrong hint is a silent-wrongness path rather than noise.
#[test]
#[ignore]
fn the_publisher_hint_does_not_make_titles_worse() {
    let Ok(zip_path) = std::env::var("COMIC_RENAMER_TEST_ZIP") else {
        panic!("set COMIC_RENAMER_TEST_ZIP to a downloaded gcd-*.zip");
    };
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/manifest.json");
    let mut manifest = load_manifest(&repo).unwrap();
    manifest.url = format!("file://{zip_path}");
    let cache = TempDir::new().unwrap();
    let db = ensure_corpus(&manifest, cache.path()).unwrap();
    let corpus = Corpus::open(&db, &manifest).unwrap();
    let overrides = Overrides::default();

    let truth = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/humble-ground-truth.tsv");
    let text = std::fs::read_to_string(&truth).unwrap();

    // §7: normalise both sides or you chase phantoms. Humble writes `&` where
    // the corpus writes "and", and inserts `Vol 1:` mid-title.
    let key = |s: &str| -> String {
        s.to_lowercase()
            .replace('&', "and")
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect()
    };

    let mut changed = 0usize;
    let mut better = 0usize;
    let mut worse = 0usize;
    let mut demoted = 0usize;
    for line in text.lines().skip(1) {
        let mut cols = line.split('\t');
        let (Some(bundle), Some(title), Some(stem)) = (cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        let hint = publisher_hint(bundle, &corpus);
        if hint.0.is_none() {
            continue;
        }
        let with = resolve(stem, bundle, &hint, &corpus, &overrides, "en");
        let without = resolve(stem, bundle, &PublisherHint(None), &corpus, &overrides, "en");
        let with_name = settled(&with).map(|s| s.series.clone());
        let without_name = settled(&without).map(|s| s.series.clone());
        if with_name == without_name {
            continue;
        }
        changed += 1;
        let truth_key = key(title);
        let hit = |s: &Option<String>| s.as_ref().is_some_and(|s| truth_key.starts_with(&key(s)));
        // Several candidates is a safe outcome, not a wrong one. Only a
        // *different, single* title counts as worse -- that is the path where
        // a picker's default would be wrong.
        let asserted = with_name.is_some();
        match (hit(&with_name), hit(&without_name)) {
            (true, false) => better += 1,
            (false, true) if asserted => {
                worse += 1;
                println!(
                    "  WORSE  stem={stem}\n         truth={title}\n         with-hint={with_name:?}"
                );
            }
            (false, true) => demoted += 1,
            _ => {}
        }
    }
    println!(
        "hint changed {changed} titles: {better} better, {worse} wrong, {demoted} sent to review"
    );
    assert_eq!(worse, 0, "the hint asserted {worse} wrong titles");
}

/// Reports located and ready rates over the held-out set. Ignored: needs the
/// corpus. Task 14 owns the real gate; this is instrumentation, and it claims
/// no silent-false-positive number — §7 and D26 reserve that for the graded
/// ground-truth run.
#[test]
#[ignore]
fn report_coverage_over_the_heldout_set() {
    let Ok(zip_path) = std::env::var("COMIC_RENAMER_TEST_ZIP") else {
        panic!("set COMIC_RENAMER_TEST_ZIP to a downloaded gcd-*.zip");
    };
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/manifest.json");
    let mut manifest = load_manifest(&repo).unwrap();
    manifest.url = format!("file://{zip_path}");
    let cache = TempDir::new().unwrap();
    let db = ensure_corpus(&manifest, cache.path()).unwrap();
    let corpus = Corpus::open(&db, &manifest).unwrap();
    let overrides = Overrides::default();

    let names = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/heldout-filenames.txt");
    let text = std::fs::read_to_string(&names).unwrap();

    let (mut total, mut located, mut ready) = (0usize, 0usize, 0usize);
    let mut buckets: std::collections::BTreeMap<&str, usize> = Default::default();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let stem = line.rsplit_once('.').map_or(line, |(s, _)| s);
        // No folder hint: §6.3's "the no-publisher case is the common case".
        let r = resolve(stem, "", &PublisherHint(None), &corpus, &overrides, "en");
        total += 1;
        let bucket = match r.as_deref() {
            Some([_]) => {
                ready += 1;
                located += 1;
                "one suggestion"
            }
            Some([]) => {
                located += 1;
                "refused (language or volume)"
            }
            Some(_) => {
                located += 1;
                "several suggestions"
            }
            None => {
                if std::env::var("SHOW_FAILURES").is_ok() {
                    println!("  FAIL {stem}");
                }
                "nothing matched"
            }
        };
        *buckets.entry(bucket).or_default() += 1;
    }
    let pct = |n: usize| 100.0 * n as f64 / total as f64;
    println!("heldout: {total} names");
    println!("  located {:.1}%", pct(located));
    println!("  one suggestion {:.1}%", pct(ready));
    for (bucket, count) in &buckets {
        println!("  {bucket:<18} {:>5.1}%  ({count})", pct(*count));
    }
}
