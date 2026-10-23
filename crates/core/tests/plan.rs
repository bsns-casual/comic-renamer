//! Task 7. JSON Lines (§6.9): header object, then one row per line. A plan is
//! handed to assistants and attached to public PRs, so `read` trusts nothing.

use comic_renamer_core::plan::{checksum, read, write};
use comic_renamer_core::types::{
    Mechanism, Mode, PlanError, PlanHeader, PlanRow, SeriesRecord, SourceStat, Status,
};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn header() -> PlanHeader {
    PlanHeader {
        version: comic_renamer_core::plan::PLAN_VERSION,
        preset: "komga".into(),
        mode: Mode::Move,
        source_root: PathBuf::from("/src"),
        dest_root: PathBuf::from("/dest"),
        lang: "en".into(),
        required_bytes: 4096,
        download_bytes: 0,
        // Non-zero on purpose: with 0 here, `round_trips_every_field` passed
        // against a `write` that hard-coded the field, and the count is the
        // only way `apply` learns about files `scan` never planned.
        unmapped: 3,
        checksum: String::new(),
    }
}

fn row(id: &str, destination_preview: &str) -> PlanRow {
    PlanRow {
        id: id.into(),
        source: PathBuf::from(format!("/src/{id}.pdf")),
        destination_preview: PathBuf::from(destination_preview),
        extension: "pdf".into(),
        series: "Hellboy Omnibus".into(),
        volume: Some(1),
        subtitle: None,
        run: 4,
        status: Status::Mapped,
        mechanism: Mechanism::Rename,
        source_stat: SourceStat {
            size: 4096,
            mtime_ns: 1_700_000_000_000_000_000,
            inode: 42,
        },
        record: Some(SeriesRecord {
            name: "Hellboy Omnibus".into(),
            publisher: "Dark Horse".into(),
            language: "en".into(),
            year_began: 2018,
            run: 4,
        }),
    }
}

fn sample_plan() -> (PlanHeader, Vec<PlanRow>) {
    (
        header(),
        vec![
            row("a", "Hellboy Omnibus/Hellboy Omnibus Vol. 01.pdf"),
            row("b", "Hellboy Omnibus/Hellboy Omnibus Vol. 02.pdf"),
        ],
    )
}

fn in_dir(dir: &TempDir) -> PathBuf {
    dir.path().join("plan.jsonl")
}

/// Writes raw lines, for the hand-edited cases a writer cannot produce.
fn write_raw(dir: &TempDir, lines: &[String]) -> PathBuf {
    let path = in_dir(dir);
    std::fs::write(&path, lines.join("\n")).unwrap();
    path
}

fn lines_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A well-formed plan on disk, as raw lines ready to be tampered with.
fn written(dir: &TempDir) -> Vec<String> {
    let (h, rows) = sample_plan();
    let path = in_dir(dir);
    write(&h, &rows, &path).unwrap();
    lines_of(&path)
}

#[test]
fn round_trips_every_field() {
    let dir = TempDir::new().unwrap();
    let (mut h, rows) = sample_plan();
    let path = in_dir(&dir);
    write(&h, &rows, &path).unwrap();
    h.checksum = checksum(&h, &rows);
    assert_eq!(read(&path).unwrap(), (h, rows));
}

/// Named separately because it is the one field with no row to carry it: an
/// unmapped file never becomes a plan row, so a header that lost the count
/// would leave `apply` unable to say anything about those files at all.
#[test]
fn the_unmapped_count_survives_the_file() {
    let dir = TempDir::new().unwrap();
    let (h, rows) = sample_plan();
    assert_ne!(h.unmapped, 0, "a zero here would prove nothing");
    let path = in_dir(&dir);
    write(&h, &rows, &path).unwrap();
    assert_eq!(read(&path).unwrap().0.unmapped, h.unmapped);
}

#[test]
fn rejects_duplicate_ids_unknown_status_and_escaping_paths() {
    // Review Focus 5
    let dir = TempDir::new().unwrap();
    let mut l = written(&dir);
    l.push(l[1].clone()); // the same row id twice
    assert!(matches!(
        read(&write_raw(&dir, &l)),
        Err(PlanError::DuplicateRowId(_))
    ));

    let dir = TempDir::new().unwrap();
    let mut l = written(&dir);
    l[1] = l[1].replace(r#""status":"mapped""#, r#""status":"definitely-mapped""#);
    assert_ne!(l[1], written(&dir)[1], "the status was not rewritten");
    assert!(read(&write_raw(&dir, &l)).is_err());

    let dir = TempDir::new().unwrap();
    let mut l = written(&dir);
    l[1] = l[1].replace(
        r#""destination_preview":"Hellboy Omnibus/Hellboy Omnibus Vol. 01.pdf""#,
        r#""destination_preview":"../../etc/passwd""#,
    );
    assert!(matches!(
        read(&write_raw(&dir, &l)),
        Err(PlanError::OutsideDestRoot(_))
    ));
}

#[test]
fn one_bad_row_rejects_the_whole_file() {
    // Review Focus 5: never return the rows that still parse.
    let dir = TempDir::new().unwrap();
    let (h, _) = sample_plan();
    let rows: Vec<PlanRow> = (0..10)
        .map(|i| row(&format!("r{i}"), &format!("S/S Vol. {i:02}.pdf")))
        .collect();
    let path = in_dir(&dir);
    write(&h, &rows, &path).unwrap();
    let mut l = lines_of(&path);
    l[5] = r#"{"id":"r5","source":"/src/r5.pdf"}"#.to_string(); // missing fields
    assert!(read(&write_raw(&dir, &l)).is_err());
}

#[test]
fn an_unknown_field_is_an_error_not_a_silent_default() {
    let dir = TempDir::new().unwrap();
    let mut l = written(&dir);
    l[1] = l[1].replace(r#""id":"a""#, r#""id":"a","destinaton_preview":"typo""#);
    assert!(read(&write_raw(&dir, &l)).is_err());
}

#[test]
fn checksum_detects_an_edited_row() {
    let dir = TempDir::new().unwrap();
    let mut l = written(&dir);
    l[1] = l[1].replace(r#""series":"Hellboy Omnibus""#, r#""series":"Hellboy Omnibusz""#);
    assert!(matches!(
        read(&write_raw(&dir, &l)),
        Err(PlanError::ChecksumMismatch { .. })
    ));
}

#[test]
fn a_plan_with_no_header_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = write_raw(&dir, &[]);
    assert!(matches!(read(&path), Err(PlanError::MissingHeader)));
}

#[test]
fn the_checksum_does_not_cover_itself() {
    // Otherwise stamping it would invalidate it, and no plan could ever verify.
    let (h, rows) = sample_plan();
    let stamped = PlanHeader {
        checksum: "whatever".into(),
        ..h.clone()
    };
    assert_eq!(checksum(&h, &rows), checksum(&stamped, &rows));
}

#[test]
fn a_row_rebuilds_its_resolver_result() {
    // D14: apply recomputes the destination from metadata, so the round trip
    // back to Resolved is the thing that must not drift.
    let r = row("a", "S/S.pdf");
    let resolved = r.resolved();
    assert_eq!(resolved.series, r.series);
    assert_eq!(resolved.volume, r.volume);
    assert_eq!(resolved.run, r.run);
    assert_eq!(resolved.status, r.status);
}

#[test]
fn a_v1_header_is_refused_for_its_version_not_for_a_missing_field() {
    // A v1 header has no `unmapped`. The version has to be read before the
    // header is deserialised, or the user sees a serde field error instead of
    // being told to scan again.
    let dir = TempDir::new().unwrap();
    let v1 = r#"{"version":1,"preset":"komga","mode":"move","source_root":"/s","dest_root":"/d","lang":"en","required_bytes":0,"download_bytes":0,"checksum":""}"#;
    let path = write_raw(&dir, &[v1.to_string()]);
    assert!(matches!(
        read(&path),
        Err(PlanError::UnsupportedVersion { found: 1, .. })
    ));
}
