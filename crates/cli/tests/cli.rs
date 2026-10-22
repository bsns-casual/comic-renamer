//! The command surface, end to end against the real binary.
//!
//! Plans are built here rather than scanned: `scan` needs the corpus, which
//! means a network fetch, and none of what this file checks is about
//! resolution. `apply` and `undo` are driven through the binary because the
//! disclosure, the prompt and the exit code are the behaviour under test and
//! none of them exist in the library.

use comic_renamer_core::preset::preset_by_id;
use comic_renamer_core::types::{
    Basis, Mechanism, Mode, PlanHeader, PlanRow, Resolved, SourceStat, Status,
};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const EXE: &str = env!("CARGO_BIN_EXE_comic-renamer");

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A source tree, a destination tree and a plan that joins them.
struct Fixture {
    _dir: TempDir,
    source: PathBuf,
    dest: PathBuf,
    plan: PathBuf,
    /// Where row 0 lands, absolute.
    placed: PathBuf,
}

/// One zero-byte file (invariant 3) resolved to `Hellboy Vol. 01`.
fn fixture(mode: Mode) -> Fixture {
    let dir = TempDir::new().unwrap();
    // Canonical from the start: on macOS a TempDir sits under `/var`, which is
    // a symlink, and `apply` canonicalises both roots before comparing them to
    // each row's source.
    let root = dir.path().canonicalize().unwrap();
    let source = root.join("bundle");
    let dest = root.join("library");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&dest).unwrap();

    let file = source.join("hellboy_vol1.pdf");
    std::fs::File::create(&file).unwrap();
    let meta = std::fs::metadata(&file).unwrap();

    let resolved = Resolved {
        series: "Hellboy".into(),
        volume: Some(1),
        subtitle: None,
        run: 10,
        status: Status::Ready(Basis::Unambiguous),
        record: None,
    };
    let relative = preset_by_id("komga").unwrap().destination(&resolved, "pdf");

    // Same volume, both trees being in one TempDir.
    let mechanism = match mode {
        Mode::Move => Mechanism::Rename,
        Mode::Copy => Mechanism::Clone,
    };
    let plan = write_plan(&root, &source, &dest, mode, mechanism, &file);
    let _ = meta;

    Fixture {
        placed: dest.join(&relative),
        _dir: dir,
        source,
        dest,
        plan,
    }
}

/// One row, resolved to `Hellboy Vol. 01`, written beside `beside`.
fn write_plan(
    beside: &Path,
    source_root: &Path,
    dest_root: &Path,
    mode: Mode,
    mechanism: Mechanism,
    file: &Path,
) -> PathBuf {
    let meta = std::fs::metadata(file).unwrap();
    let resolved = Resolved {
        series: "Hellboy".into(),
        volume: Some(1),
        subtitle: None,
        run: 10,
        status: Status::Ready(Basis::Unambiguous),
        record: None,
    };
    let relative = preset_by_id("komga").unwrap().destination(&resolved, "pdf");

    let row = PlanRow {
        id: "r0".into(),
        source: file.to_path_buf(),
        destination_preview: relative,
        extension: "pdf".into(),
        series: resolved.series.clone(),
        volume: resolved.volume,
        subtitle: None,
        run: resolved.run,
        status: resolved.status.clone(),
        mechanism,
        source_stat: SourceStat {
            size: meta.len(),
            mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            inode: meta.ino(),
        },
        record: None,
    };

    let header = PlanHeader {
        version: 1,
        preset: "komga".into(),
        mode,
        source_root: source_root.to_path_buf(),
        dest_root: dest_root.to_path_buf(),
        lang: "en".into(),
        required_bytes: 0,
        download_bytes: 0,
        checksum: String::new(),
    };

    let plan = beside.join("plan.jsonl");
    comic_renamer_core::plan::write(&header, &[row], &plan).unwrap();
    plan
}

fn run(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(EXE)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Every path under `roots`, with what it is. The claim a refusal has to make
/// is "no files moved", and only comparing this before and after makes it —
/// an `Err` exit code alone does not.
fn listing(roots: &[&Path]) -> BTreeMap<PathBuf, (bool, u64)> {
    fn walk(path: &Path, out: &mut BTreeMap<PathBuf, (bool, u64)>) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        out.insert(path.to_path_buf(), (meta.is_dir(), meta.ino()));
        if !meta.is_dir() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            walk(&entry.path(), out);
        }
    }
    let mut out = BTreeMap::new();
    for root in roots {
        walk(root, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// The command surface
// ---------------------------------------------------------------------------

#[test]
fn apply_rejects_a_mode_flag() {
    // §6.13: mechanism selection happens at scan, so `apply` takes no mode.
    // A plan carries it, and a flag here could contradict the plan it reads.
    for flag in ["--move", "--copy", "--mode"] {
        let out = run(&["apply", "plan.jsonl", flag], "");
        assert!(!out.status.success(), "{flag} was accepted");
        assert!(
            stderr(&out).contains("unexpected argument"),
            "{flag}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn the_help_does_not_cite_the_spec_at_the_user() {
    // Every flag here has a decision behind it and the temptation is to cite
    // it. "(D23)" and "(§6.8.2)" mean nothing to someone holding a bundle of
    // comics; the provenance belongs in the code, beside the flag.
    for args in [
        ["--help"].as_slice(),
        &["scan", "--help"],
        &["apply", "--help"],
        &["undo", "--help"],
    ] {
        let out = run(args, "");
        let text = stdout(&out) + &stderr(&out);
        for leak in ["§", "D23", "D35", "D38", "6.10", "6.13"] {
            assert!(
                !text.contains(leak),
                "`{}` leaks {leak:?}:\n{text}",
                args.join(" ")
            );
        }
        // And each positional is described, not left blank.
        assert!(!text.contains("<PLAN>  \n"), "undocumented arg: {text}");
    }
}

#[test]
fn undo_rejects_a_mode_flag_too() {
    let out = run(&["undo", "plan.jsonl", "--move"], "");
    assert!(!out.status.success());
}

// ---------------------------------------------------------------------------
// The prompt
// ---------------------------------------------------------------------------

#[test]
fn aborting_at_the_prompt_moves_nothing() {
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);

    // "yes" is the reflexive answer the typed word exists to defeat.
    let out = run(&["apply", f.plan.to_str().unwrap()], "yes\n");

    assert!(!out.status.success(), "a refusal must not exit zero");
    assert!(stderr(&out).contains("aborted at the prompt"), "{}", stderr(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]), "the tree changed");
    assert!(!f.placed.exists());
}

#[test]
fn an_empty_answer_aborts() {
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run(&["apply", f.plan.to_str().unwrap()], "\n");
    assert!(!out.status.success());
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn eof_aborts_rather_than_consenting() {
    // A non-interactive caller must not be taken as having agreed.
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run(&["apply", f.plan.to_str().unwrap()], "");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn the_wrong_word_aborts() {
    // Typing COPY at a move prompt is a user who misread the mode.
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run(&["apply", f.plan.to_str().unwrap()], "COPY\n");
    assert!(!out.status.success());
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn the_disclosure_prints_before_the_prompt() {
    let f = fixture(Mode::Move);
    let out = stdout(&run(&["apply", f.plan.to_str().unwrap()], "no\n"));
    let disclosure = out.find("MOVE MODE").expect("no disclosure");
    let prompt = out.find("Type MOVE").expect("no prompt");
    assert!(disclosure < prompt, "the prompt came first:\n{out}");
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

#[test]
fn the_typed_word_moves_the_file_and_undo_puts_it_back() {
    let f = fixture(Mode::Move);
    let original = f.source.join("hellboy_vol1.pdf");
    let before = listing(&[&f.source, &f.dest]);

    let out = run(&["apply", f.plan.to_str().unwrap()], "MOVE\n");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.placed.exists(), "{}", stdout(&out));
    assert!(!original.exists(), "a move left the original behind");
    assert_eq!(
        f.placed.file_name().unwrap(),
        "Hellboy Vol. 01.pdf",
        "D37: padded to the series width"
    );

    let out = run(&["undo", f.plan.to_str().unwrap()], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(original.exists(), "undo did not restore the original");
    assert!(!f.placed.exists(), "undo left the file in the library");
    assert_eq!(before, listing(&[&f.source, &f.dest]), "not the tree we started with");
}

#[test]
fn yes_skips_the_prompt_but_not_the_disclosure() {
    let f = fixture(Mode::Move);
    let out = run(&["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.placed.exists());
    // The record still has to exist: --yes is consent, not silence.
    assert!(stdout(&out).contains("MOVE MODE"), "{}", stdout(&out));
}

#[test]
fn a_copy_keeps_the_original() {
    let f = fixture(Mode::Copy);
    let original = f.source.join("hellboy_vol1.pdf");
    let out = run(&["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.placed.exists());
    assert!(original.exists(), "a copy must not remove the source");
    assert!(stdout(&out).contains("COPY MODE"), "{}", stdout(&out));
}

#[test]
fn applying_twice_refuses_rather_than_overwriting() {
    // Invariant 6, through the CLI: the second run finds the destination
    // occupied and the syscall refuses it.
    let f = fixture(Mode::Move);
    assert!(run(&["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());

    let before = listing(&[&f.source, &f.dest]);
    let out = run(&["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert_eq!(before, listing(&[&f.source, &f.dest]), "the second run wrote");
    // Nothing is ready any more — the source is gone — so this reports
    // rather than executes. What it must not do is place anything.
    assert!(
        stdout(&out).contains("Nothing to do") || !out.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn undo_with_no_manifest_is_not_an_error() {
    // Idempotent: never applied, or already fully reversed.
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run(&["undo", f.plan.to_str().unwrap()], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("0 restored"), "{}", stdout(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

// ---------------------------------------------------------------------------
// Cross-volume, which needs a real second device
// ---------------------------------------------------------------------------

/// `COMIC_RENAMER_TEST_VOLUME=<mount on another device>`. A same-volume move
/// is a `rename`, so the two ACROSS VOLUMES branches of the disclosure are
/// unreachable without one.
///
/// Core's `apply_exec` already covers the streamed *execution* against the
/// same RAM disk. What is only provable here is the join: that the mechanism
/// the filesystem dictates reaches the disclosure, and that the user is told
/// the run crosses volumes before being asked to consent to it.
///
/// The mechanism is computed by `select_mechanism`, not written into the
/// fixture — hard-coding `Stream` would assume the very answer under test.
/// Going through `scan` would be more faithful still, and would pull the
/// 34 MiB corpus into CI for one assertion.
#[test]
#[ignore = "needs COMIC_RENAMER_TEST_VOLUME on another device"]
fn a_cross_volume_plan_says_so_before_the_prompt() {
    let volume = std::env::var("COMIC_RENAMER_TEST_VOLUME")
        .expect("COMIC_RENAMER_TEST_VOLUME is not set");

    let dir = TempDir::new().unwrap();
    let source = dir.path().canonicalize().unwrap().join("bundle");
    std::fs::create_dir_all(&source).unwrap();
    let file = source.join("hellboy_vol1.pdf");
    std::fs::File::create(&file).unwrap();

    let dest = PathBuf::from(&volume).join("cli-cross-volume");
    std::fs::create_dir_all(&dest).unwrap();

    let mechanism =
        comic_renamer_core::fsops::select_mechanism(&file, &dest, Mode::Move).unwrap();
    assert_eq!(
        mechanism,
        Mechanism::Stream,
        "{volume} is on this device, so there is nothing cross-volume to test"
    );

    let plan = write_plan(dir.path(), &source, &dest, Mode::Move, mechanism, &file);
    let out = run(&["apply", plan.to_str().unwrap()], "no\n");
    let said = stdout(&out);

    assert!(!out.status.success());
    assert!(said.contains("ACROSS VOLUMES"), "{said}");
    // The number that gates the run, stated before consent is asked for.
    assert!(said.contains("must be free at the destination"), "{said}");
    assert!(said.find("ACROSS VOLUMES") < said.find("Type MOVE"), "{said}");

    let _ = std::fs::remove_dir_all(&dest);
}
