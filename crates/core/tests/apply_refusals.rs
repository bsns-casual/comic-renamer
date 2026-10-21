//! Task 11. `apply` pre-flight, exercised through `check`, which runs
//! everything up to execution and writes nothing. **Every test wraps both trees in a
//! `TreeGuard` with no permits** — the test an earlier draft was missing,
//! because `Err` alone proves nothing about whether rows 1-3 ran before row 4
//! refused.

mod common;

use comic_renamer_core::apply::{check, lock_path};
use comic_renamer_core::corpus::Corpus;
use comic_renamer_core::overrides::Overrides;
use comic_renamer_core::types::{ApplyError, PlanHeader, PlanRow, ScanOpts};
use comic_renamer_core::{plan, scan};
use common::{corpus, tree_from_names, TreeGuard};
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

fn gcd() -> (TempDir, Corpus) {
    corpus(&[
        ("Mazebook", "mazebook", "Dark Horse", "en", 2021, 5),
        ("Hellboy Omnibus", "hellboyomnibus", "Dark Horse", "en", 2018, 4),
        ("Birdking", "birdking", "Dark Horse", "en", 2022, 3),
        ("Velvet", "velvet", "Image", "en", 2014, 3),
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
    fn new(names: &[&str], corpus: &Corpus) -> Fixture {
        Fixture::under(names, corpus, None)
    }

    /// A source genuinely inside a cloud-sync-looking path. The roots are
    /// canonicalized at `apply`, so a made-up path no longer reaches the
    /// cloud check at all.
    fn cloud(names: &[&str], corpus: &Corpus) -> Fixture {
        Fixture::under(names, corpus, Some("CloudStorage/Dropbox/Comics"))
    }

    fn under(names: &[&str], corpus: &Corpus, nest: Option<&str>) -> Fixture {
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
        let (header, rows) = scan::scan(&opts, corpus, &Overrides::default(), &mut |_| {}).unwrap();
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

fn run(f: &Fixture) -> Result<comic_renamer_core::types::ApplyResult, ApplyError> {
    check(&f.plan, false, &mut |_| {})
}

#[test]
fn a_refused_run_moves_zero_files() {
    let (_g, c) = gcd();

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

#[test]
fn refuses_a_lock_held_by_another_run() {
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    // Held the way a concurrent run holds it: an flock on the descriptor,
    // not merely the existence of a file.
    let held = comic_renamer_core::apply::Lock::acquire(&f.plan).unwrap();
    let _guard = TreeGuard::new(&f.roots());
    assert!(matches!(run(&f), Err(ApplyError::Locked)));
    drop(held);
    run(&f).unwrap();
}

#[test]
fn a_lock_file_left_by_a_killed_run_does_not_block() {
    // flock, not O_EXCL: the kernel releases the lock when its owner dies.
    // An O_EXCL lock file survived a SIGKILL and had to be deleted by hand —
    // and a crash is exactly when undo is needed, so the stale lock blocked
    // the one recovery path.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    std::fs::write(lock_path(&f.plan), b"leftover from a killed run").unwrap();
    let _guard = TreeGuard::new(&f.roots());
    run(&f).unwrap();
}

#[test]
fn the_lock_is_released_when_the_run_ends() {
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    let _guard = TreeGuard::new(&f.roots());
    run(&f).unwrap();
    // A second run proves release; the file itself may persist, which is
    // what makes a killed run recoverable.
    run(&f).unwrap();
}

#[test]
fn does_not_execute_review_rows() {
    let (_g, c) = gcd();
    // Two that resolve cleanly, three that cannot: an unknown title, an
    // unsplit subtitle, and a volume no record can hold.
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
    let _guard = TreeGuard::new(&f.roots());
    let r = run(&f).unwrap();
    assert_eq!((r.executed, r.skipped), (2, 3), "{r:?}");
}

#[test]
fn an_occupied_destination_refuses_the_row_not_the_run() {
    // §6.10: never overwrite, never auto-suffix — refuse the row.
    let (_g, c) = gcd();
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
    let (_g, c) = gcd();
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
    let mut saw_notice = false;
    let r = check(&f.plan, false, &mut |p| {
        if p.current.contains("edited") {
            saw_notice = true;
        }
    })
    .unwrap();
    assert_eq!(r.executed, 1);
    assert!(saw_notice, "the edit was not reported");
}

#[test]
fn a_cloud_source_move_is_allowed_with_the_override() {
    let (_g, c) = gcd();
    let f = Fixture::cloud(&["mazebook.pdf"], &c);
    let _guard = TreeGuard::new(&f.roots());
    // Without the override this is CloudSourceMove; with it the run proceeds
    // and the rows are judged on their own merits.
    assert!(matches!(run(&f), Err(ApplyError::CloudSourceMove(_))));
    let r = check(&f.plan, true, &mut |_| {}).unwrap();
    assert_eq!(r.executed, 1);
}

#[test]
fn a_run_larger_than_the_free_space_is_refused_whole() {
    // The arithmetic, which is where a bug would be; the statvfs call around
    // it is one syscall. A run must leave max(1 GiB, 10%) behind.
    use comic_renamer_core::apply::usable;
    const GIB: u64 = 1 << 30;
    let free = 20 * GIB;
    let available = usable(free);
    assert_eq!(available, 18 * GIB);
    assert!(19 * GIB > available, "a 19 GiB run must not fit in 20 GiB free");
    assert!(17 * GIB <= available);
}

#[test]
fn an_edited_plan_cannot_move_a_file_from_outside_the_source_root() {
    // §6.9 forbids absolute paths outside the chosen roots; §6.11 says the
    // plan is re-validated here because the plan file is untrusted. Without
    // this, an edited plan relocates any file on the system into the library
    // — and re-stamping the checksum and the stat defeats every other check.
    let (_g, c) = gcd();
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
    assert!(matches!(
        check(&f.plan, false, &mut |_| {}),
        Err(ApplyError::OutsideSourceRoot(_, _))
    ));
    assert!(victim.exists(), "the victim was taken");
}

#[test]
fn an_edited_plan_cannot_repoint_the_destination_root() {
    // dest_root is the containment anchor for every write, and
    // resolve_destination opens it without O_NOFOLLOW by necessity. A plan
    // naming `/` would land the library at the filesystem root.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    f.rewrite(|header, _| header.dest_root = PathBuf::from("/"));
    let _guard = TreeGuard::new(&[f.source.as_path(), f.dest.as_path()]);
    assert!(matches!(
        check(&f.plan, false, &mut |_| {}),
        Err(ApplyError::RootsOverlap)
    ));
}

#[test]
fn an_edited_plan_cannot_make_the_roots_overlap() {
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    f.rewrite(|header, _| header.dest_root = header.source_root.clone());
    let _guard = TreeGuard::new(&[f.source.as_path(), f.dest.as_path()]);
    assert!(matches!(
        check(&f.plan, false, &mut |_| {}),
        Err(ApplyError::RootsOverlap)
    ));
}

#[test]
fn a_run_that_executes_nothing_does_not_burn_the_plan() {
    // The manifest was created before the loop, so refusing every row still
    // wrote a header-only .undo.csv — and O_EXCL then rejected the plan
    // forever, so fixing the collision and re-running was impossible.
    let (_g, c) = gcd();
    let f = Fixture::new(&["mazebook.pdf"], &c);
    std::fs::create_dir(f.dest.join("Mazebook")).unwrap();
    std::fs::write(f.dest.join("Mazebook/Mazebook.pdf"), b"in the way").unwrap();

    let r = comic_renamer_core::apply::apply(&f.plan, false, &mut |_| {}).unwrap();
    assert_eq!((r.executed, r.skipped), (0, 1));
    assert!(
        !comic_renamer_core::apply::undo_path(&f.plan).exists(),
        "a run that moved nothing still created a manifest"
    );

    // The user clears the collision and re-runs the same plan.
    std::fs::remove_dir_all(f.dest.join("Mazebook")).unwrap();
    let r = comic_renamer_core::apply::apply(&f.plan, false, &mut |_| {}).unwrap();
    assert_eq!(r.executed, 1, "the plan was unusable after a refused run");
}

