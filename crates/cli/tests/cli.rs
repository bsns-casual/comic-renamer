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
    run_with_env(args, stdin, &[])
}

fn run_with_env(args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(EXE);
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
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

/// Serves `body` once, as JSON, and returns its base URL.
///
/// Hand-rolled rather than reaching for a mock-HTTP crate: the pre-flight
/// needs one request answered, and `TcpListener` already does that.
fn one_shot_json(body: &'static str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let Ok((mut socket, _)) = listener.accept() else {
            return;
        };
        // Drain the request head, or the client sees a reset before its read.
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
            line.clear();
        }
        let _ = socket.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
    });
    url
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
fn applying_a_move_twice_refuses_because_the_source_is_gone() {
    let f = fixture(Mode::Move);
    assert!(run(&["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());

    let before = listing(&[&f.source, &f.dest]);
    let out = run(&["apply", f.plan.to_str().unwrap(), "--yes"], "");

    // Named, not an either-or: an assertion that accepts two outcomes stops
    // telling you which one happened, and the weaker branch is the one that
    // would hide a regression.
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("changed since the scan"),
        "{}",
        stderr(&out)
    );
    assert_eq!(before, listing(&[&f.source, &f.dest]), "the second run wrote");
}

#[test]
fn applying_a_copy_twice_refuses_and_leaves_the_destination_alone() {
    // A copy leaves the source in place, so the second run passes every
    // *source* check and is stopped by the collision check instead.
    //
    // This is not invariant 6's syscall-level no-clobber: `find_collisions`
    // catches the occupied destination first, so the CLI cannot reach
    // `clone_excl`'s native EEXIST from here. Core's Task 9 suite covers the
    // syscall. What this covers is the layer above it — the refusal is
    // reported, the file already in the library is untouched, and the exit
    // code says nothing was placed.
    let f = fixture(Mode::Copy);
    assert!(run(&["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    assert!(f.placed.exists());

    // Distinguishable content, so an overwrite cannot look like a no-op.
    std::fs::write(&f.placed, b"the file already in the library").unwrap();
    let before = listing(&[&f.source, &f.dest]);

    let out = run(&["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]), "the second run wrote");
    assert_eq!(
        std::fs::read(&f.placed).unwrap(),
        b"the file already in the library",
        "the destination was overwritten"
    );
    // Named, with the reason, not just counted.
    let said = stdout(&out);
    assert!(said.contains("already in the library"), "{said}");
    assert!(said.contains("Hellboy Vol. 01.pdf"), "{said}");
    // And no mode block over a run that will do nothing: a MOVE prompt that
    // moves nothing teaches the user the prompt does not mean anything.
    assert!(!said.contains("COPY MODE"), "{said}");
}

#[test]
fn a_plan_with_no_ready_row_says_so_and_does_not_exit_zero() {
    // A caller cannot tell "the library matches the plan" from "it does not"
    // out of an exit code that is zero either way.
    let f = fixture(Mode::Move);
    let text = std::fs::read_to_string(&f.plan).unwrap();
    let (header, row) = text.split_once('\n').unwrap();
    std::fs::write(
        &f.plan,
        format!(
            "{header}\n{}\n",
            row.replace(
                r#""status":{"ready":"unambiguous"}"#,
                r#""status":{"review":"no-language-match"}"#
            )
        ),
    )
    .unwrap();

    let before = listing(&[&f.source, &f.dest]);
    let out = run(&["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert!(!out.status.success(), "{}", stdout(&out));
    let said = stdout(&out);
    assert!(said.contains("Nothing to do"), "{said}");
    assert!(said.contains("Needs a human: 1 row"), "{said}");
    assert!(!said.contains("MOVE MODE"), "{said}");
    assert_eq!(before, listing(&[&f.source, &f.dest]));
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

// ---------------------------------------------------------------------------
// Komga pre-flight (§6.8.2). Hashing switched on after a rename cannot help,
// so this refuses rather than warns — and a warning a user scrolls past is
// exactly the silent loss D35 is about.
// ---------------------------------------------------------------------------

#[test]
fn an_unsafe_komga_library_refuses_and_moves_nothing() {
    for body in [
        // Hashing off: a deleted record can never acquire a hash afterwards.
        r#"[{"name":"Comics","hashFiles":false,"emptyTrashAfterScan":false}]"#,
        // Trash emptied on scan: the deleted record is purged before restore.
        r#"[{"name":"Comics","hashFiles":true,"emptyTrashAfterScan":true}]"#,
        // One bad library among good ones. The plan does not say which the
        // user is renaming into, so refusing only the named one is a guess.
        r#"[{"name":"A","hashFiles":true,"emptyTrashAfterScan":false},
            {"name":"B","hashFiles":false,"emptyTrashAfterScan":false}]"#,
    ] {
        let f = fixture(Mode::Move);
        let before = listing(&[&f.source, &f.dest]);
        let url = one_shot_json(body);
        let out = run_with_env(
            &["apply", f.plan.to_str().unwrap(), "--komga-url", &url, "--yes"],
            "",
            &[("COMIC_RENAMER_KOMGA_TOKEN", "test-token")],
        );
        assert!(!out.status.success(), "accepted {body}");
        assert!(stderr(&out).contains("hashFiles"), "{}", stderr(&out));
        assert_eq!(before, listing(&[&f.source, &f.dest]), "moved a file: {body}");
    }
}

#[test]
fn a_safe_komga_library_lets_the_run_proceed() {
    let f = fixture(Mode::Move);
    let url = one_shot_json(r#"[{"name":"Comics","hashFiles":true,"emptyTrashAfterScan":false}]"#);
    let out = run_with_env(
        &["apply", f.plan.to_str().unwrap(), "--komga-url", &url, "--yes"],
        "",
        &[("COMIC_RENAMER_KOMGA_TOKEN", "test-token")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.placed.exists());
}

#[test]
fn a_komga_url_without_a_token_refuses_rather_than_skipping_the_check() {
    // Silently skipping would turn the flag into decoration.
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run(
        &["apply", f.plan.to_str().unwrap(), "--komga-url", "http://127.0.0.1:1", "--yes"],
        "",
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("COMIC_RENAMER_KOMGA_TOKEN"), "{}", stderr(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn no_komga_url_skips_the_check_entirely() {
    // Not everyone runs Komga, and the check must not become mandatory by
    // accident. Covered by every other apply test, pinned here on purpose.
    let f = fixture(Mode::Move);
    assert!(run(&["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
}

// ---------------------------------------------------------------------------
// Ordering: what refuses, refuses before the user is asked for anything
// ---------------------------------------------------------------------------

#[test]
fn a_plan_that_will_refuse_refuses_before_disclosing_or_prompting() {
    // The pre-flight runs first on purpose. Disclosing a 31 GiB move and
    // taking the typed word, only to refuse afterwards, asks a user to
    // consent to something that was never going to happen.
    let f = fixture(Mode::Move);
    std::fs::write(f.source.join("hellboy_vol1.pdf"), b"changed since the scan").unwrap();

    let out = run(&["apply", f.plan.to_str().unwrap()], "MOVE\n");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("changed since the scan"), "{}", stderr(&out));
    let said = stdout(&out);
    assert!(!said.contains("MOVE MODE"), "disclosed a doomed run:\n{said}");
    assert!(!said.contains("Type MOVE"), "prompted for a doomed run:\n{said}");
    assert!(!f.placed.exists());
}

// ---------------------------------------------------------------------------
// undo's exit code is the only signal a script gets
// ---------------------------------------------------------------------------

#[test]
fn undo_exits_non_zero_when_a_row_cannot_be_reversed() {
    let f = fixture(Mode::Move);
    assert!(run(&["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());

    // Something else is at the destination now, so this is not the file the
    // run put there and reversing it would move a stranger's file.
    std::fs::write(&f.placed, b"a different file entirely").unwrap();

    let out = run(&["undo", f.plan.to_str().unwrap()], "");
    assert!(
        !out.status.success(),
        "a refusal exited zero, so a script would read the undo as clean:\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).contains("1 refused"), "{}", stdout(&out));
    assert!(stderr(&out).contains("refused:"), "{}", stderr(&out));
    // And it left the stranger alone rather than reversing over it.
    assert!(f.placed.exists());
}
