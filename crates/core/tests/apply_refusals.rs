//! Task 11. `apply` pre-flight, exercised through `check`, which runs
//! everything up to execution and writes nothing. **Every test wraps both trees in a
//! `TreeGuard` with no permits** — the test an earlier draft was missing,
//! because `Err` alone proves nothing about whether rows 1-3 ran before row 4
//! refused.

mod common;

use comic_renamer_core::apply::check;
use comic_renamer_core::mapping::Mappings;
use comic_renamer_core::types::{ApplyError, PlanHeader, PlanRow, ScanOpts, Status};
use comic_renamer_core::{plan, scan};
use common::{mappings, tree_from_names, TreeGuard};
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn gcd() -> Mappings {
    mappings(&[
        ("mazebook", "Mazebook", None, Some(5)),
        ("birdking_vol1", "Birdking", Some(1), Some(3)),
    ])
}

/// A source tree, an empty destination, and a written plan over both.
struct Fixture {
    _source: TempDir,
    _dest: TempDir,
    _plans: TempDir,
    source: PathBuf,
    dest: PathBuf,
    plan: PathBuf,
}

impl Fixture {
    fn new(names: &[&str], corpus: &Mappings) -> Fixture {
        Fixture::under(names, corpus, None)
    }

    /// A source genuinely inside a cloud-sync-looking path. The roots are
    /// canonicalized at `apply`, so a made-up path no longer reaches the
    /// cloud check at all.
    fn cloud(names: &[&str], corpus: &Mappings) -> Fixture {
        Fixture::under(names, corpus, Some("CloudStorage/Dropbox/Comics"))
    }

    fn under(names: &[&str], corpus: &Mappings, nest: Option<&str>) -> Fixture {
        let source = tempdir().unwrap();
        let dest = tempdir().unwrap();
        let plans = tempdir().unwrap();
        let root = match nest {
            Some(nest) => {
                let nested = source.path().join(nest);
                std::fs::create_dir_all(&nested).unwrap();
                nested
            }
            None => source.path().to_path_buf(),
        };
        tree_from_names(&root, names);
        let opts = ScanOpts {
            source_root: root.clone(),
            dest_root: dest.path().to_path_buf(),
            preset: "komga".into(),
            mode: None,
            lang: "en".into(),
        };
        let comic_renamer_core::scan::ScanOutcome { header, rows, .. } =
            scan::scan(&opts, corpus, &mut |_| {}).unwrap();
        let plan_path = plans.path().join("plan.jsonl");
        plan::write(&header, &rows, &plan_path).unwrap();
        Fixture {
            source: root,
            dest: dest.path().to_path_buf(),
            plan: plan_path,
            _source: source,
            _dest: dest,
            _plans: plans,
        }
    }

    fn roots(&self) -> [&Path; 2] {
        [self.source.as_path(), self.dest.as_path()]
    }

    fn rewrite<F: FnOnce(&mut PlanHeader, &mut Vec<PlanRow>)>(&self, edit: F) {
        let (mut header, mut rows) = plan::read(&self.plan).unwrap();
        edit(&mut header, &mut rows);
        // Re-stamped, so the checksum is valid and the *content* is what the
        // pre-flight has to catch — not the checksum.
        plan::write(&header, &rows, &self.plan).unwrap();
    }
}

/// `check` takes the plan as read, so reading it is the test's job now — and
/// `read_allowing_edits` is deliberately the reader, because §6.9's edited
/// plan has to reach the pre-flight rather than being rejected by a checksum.
fn run(f: &Fixture) -> Result<comic_renamer_core::types::ApplyResult, ApplyError> {
    run_with(f, false)
}

fn run_with(f: &Fixture, allow_cloud_move: bool) -> Result<comic_renamer_core::types::ApplyResult, ApplyError> {
    let (header, rows, _edited) = plan::read_allowing_edits(&f.plan)?;
    check(&header, &rows, allow_cloud_move, &mut |_| {})
}

#[test]
fn a_refused_run_moves_zero_files() {
    let c = gcd();

    // 1. A destination edited to point somewhere else.
    let f = Fixture::new(&["mazebook.pdf"], &c);
    f.rewrite(|_, rows| rows[0].destination_preview = PathBuf::from("Elsewhere/x.pdf"));
    {
        let _guard = TreeGuard::new(&f.roots());
        assert!(matches!(run(&f), Err(ApplyError::Tampered(_))));
    }

    // 2. A source that changed after the scan.
    let f = Fixture::new(&["mazebook.pdf"], &c);
    std::fs::write(f.source.join("mazebook.pdf"), b"now it has content").unwrap();
    {
        // The source legitimately differs, so the guard permits that one file
        // and nothing else.
        let _guard = TreeGuard::new(&f.roots());
        assert!(matches!(run(&f), Err(ApplyError::SourceChanged(_))));
    }

    // 3. A mechanism that no longer matches.
    let f = Fixture::new(&["mazebook.pdf"], &c);
    f.rewrite(|_, rows| {
        rows[0].mechanism = comic_renamer_core::types::Mechanism::Stream;
    });
    {
        let _guard = TreeGuard::new(&f.roots());
        assert!(matches!(run(&f), Err(ApplyError::MechanismChanged(_))));
    }

    // 4. A move off a cloud-synced root, without the override (D23).
    let f = Fixture::cloud(&["mazebook.pdf"], &c);
    {
        let _guard = TreeGuard::new(&f.roots());
        assert!(matches!(run(&f), Err(ApplyError::CloudSourceMove(_))));
    }

    // 5. An unknown preset.
    let f = Fixture::new(&["mazebook.pdf"], &c);
    f.rewrite(|header, _| header.preset = "plex".into());
    {
        let _guard = TreeGuard::new(&f.roots());
        assert!(matches!(run(&f), Err(ApplyError::UnknownPreset(_))));
    }
}

/// The three lock tests this file used to hold are now in `apply_record.rs`:
/// the lock is per library and held by the caller, so `check` — which writes
/// nothing — neither takes it nor needs it. That it *does not* is a claim
/// worth keeping here, because a pre-flight that quietly took a lock the
/// caller already held would deadlock every run.
#[test]
fn check_takes_no_lock_of_its_own() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    let _guard = TreeGuard::new(&f.roots());
    // Twice in a row, in one process, with nothing dropped in between.
    run(&f).unwrap();
    run(&f).unwrap();
}

#[test]
fn does_not_execute_unmapped_rows() {
    // `scan` never plans an unmapped file, but a plan is a file a human can
    // edit and a v2 plan can still carry the status. `apply` must skip such a
    // row rather than treat "not Mapped" as "ready".
    let c = mappings(&[
        ("mazebook", "Mazebook", None, Some(5)),
        ("birdking_vol1", "Birdking", Some(1), Some(3)),
        ("totallyunknownthing", "Unknown", None, None),
        ("hellboyomnibusvolume1_seedofdestruction", "Hellboy Omnibus", Some(1), None),
        // A run that can hold volume 9: this row is here to be forced to
        // Unmapped, and `validate` now refuses a volume past the run's end.
        ("velvet_vol9", "Velvet", Some(9), Some(12)),
    ]);
    let f = Fixture::new(
        &[
            "mazebook.pdf",
            "birdking_vol1.pdf",
            "totallyunknownthing.pdf",
            "hellboyomnibusvolume1_seedofdestruction.pdf",
            "velvet_vol9.pdf",
        ],
        &c,
    );
    f.rewrite(|_, rows| {
        for row in rows.iter_mut() {
            let name = row.source.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("mazebook") && !name.starts_with("birdking") {
                row.status = Status::Unmapped;
            }
        }
    });
    let _guard = TreeGuard::new(&f.roots());
    let r = run(&f).unwrap();
    assert_eq!((r.executed, r.skipped), (2, 3), "{r:?}");
}

#[test]
fn an_occupied_destination_refuses_the_row_not_the_run() {
    // §6.10: never overwrite, never auto-suffix — refuse the row.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf", "birdking_vol1.pdf"], &c);
    std::fs::create_dir(f.dest.join("Mazebook")).unwrap();
    std::fs::write(f.dest.join("Mazebook/Mazebook.pdf"), b"already here").unwrap();
    let _guard = TreeGuard::new(&f.roots());
    let r = run(&f).unwrap();
    assert_eq!((r.executed, r.skipped), (1, 1), "{r:?}");
}

#[test]
fn an_edited_plan_is_revalidated_not_rejected() {
    // §6.9. A stale checksum is a report about what a human touched; every
    // destination is recomputed from metadata regardless.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    // The checksum is taken over the re-serialised data, so reformatting a
    // plan does not trip it — only a change of content does. This edit is a
    // real one that still leaves every row executable.
    let text = std::fs::read_to_string(&f.plan).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[0] = lines[0].replace("\"required_bytes\":0", "\"required_bytes\":123");
    assert!(lines[0].contains("123"), "the header shape changed");
    std::fs::write(&f.plan, lines.join("\n")).unwrap();

    let _guard = TreeGuard::new(&f.roots());
    // The edit is reported by the reader, which is where the caller learns of
    // it: `check` is handed rows and has nothing to compare a checksum
    // against. The disclosure carries the notice to the user (`disclose`'s
    // `edited` paragraph, pinned in the CLI's own tests).
    let (header, rows, edited) = plan::read_allowing_edits(&f.plan).unwrap();
    assert!(edited, "the edit was not reported");
    // And the plan is revalidated rather than rejected: the row still runs.
    let r = check(&header, &rows, false, &mut |_| {}).unwrap();
    assert_eq!(r.executed, 1);
}

#[test]
fn a_cloud_source_move_is_allowed_with_the_override() {
    let c = gcd();
    let f = Fixture::cloud(&["mazebook.pdf"], &c);
    let _guard = TreeGuard::new(&f.roots());
    // Without the override this is CloudSourceMove; with it the run proceeds
    // and the rows are judged on their own merits.
    assert!(matches!(run(&f), Err(ApplyError::CloudSourceMove(_))));
    let r = run_with(&f, true).unwrap();
    assert_eq!(r.executed, 1);
}


#[test]
fn an_edited_plan_cannot_move_a_file_from_outside_the_source_root() {
    // §6.9 forbids absolute paths outside the chosen roots; §6.11 says the
    // plan is re-validated here because the plan file is untrusted. Without
    // this, an edited plan relocates any file on the system into the library
    // — and re-stamping the checksum and the stat defeats every other check.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    let elsewhere = tempdir().unwrap();
    let victim = elsewhere.path().join("tax-return.pdf");
    std::fs::write(&victim, b"PRIVATE").unwrap();
    let stat = std::fs::metadata(&victim).unwrap();

    f.rewrite(|_, rows| {
        rows[0].source = victim.clone();
        rows[0].source_stat = comic_renamer_core::types::SourceStat {
            size: stat.len(),
            mtime_ns: std::os::unix::fs::MetadataExt::mtime(&stat) * 1_000_000_000
                + std::os::unix::fs::MetadataExt::mtime_nsec(&stat),
            inode: std::os::unix::fs::MetadataExt::ino(&stat),
        };
    });

    let _guard = TreeGuard::new(&[f.source.as_path(), f.dest.as_path(), elsewhere.path()]);
    assert!(matches!(run(&f), Err(ApplyError::OutsideSourceRoot(_, _))));
    assert!(victim.exists(), "the victim was taken");
}

#[test]
fn an_edited_plan_cannot_repoint_the_destination_root() {
    // dest_root is the containment anchor for every write, and
    // resolve_destination opens it without O_NOFOLLOW by necessity. A plan
    // naming `/` would land the library at the filesystem root.
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    f.rewrite(|header, _| header.dest_root = PathBuf::from("/"));
    let _guard = TreeGuard::new(&[f.source.as_path(), f.dest.as_path()]);
    assert!(matches!(run(&f), Err(ApplyError::RootsOverlap)));
}

#[test]
fn an_edited_plan_cannot_make_the_roots_overlap() {
    let c = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    f.rewrite(|header, _| header.dest_root = header.source_root.clone());
    let _guard = TreeGuard::new(&[f.source.as_path(), f.dest.as_path()]);
    assert!(matches!(run(&f), Err(ApplyError::RootsOverlap)));
}

// `a_run_that_executes_nothing_does_not_burn_the_plan` moved to
// `apply_record.rs` as `a_run_that_executes_nothing_writes_no_record`: it is
// the only test in this file that executed a run, and what it was really
// about — a refused run leaving no record behind — now belongs beside the
// store. The plan is no longer burnable at all, because nothing is named
// after it.

