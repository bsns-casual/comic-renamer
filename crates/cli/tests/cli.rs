//! The command surface, end to end against the real binary.
//!
//! Plans are built here rather than scanned: `scan` needs a mapping, and
//! none of what this file checks is about lookup. `apply` and `undo` are driven through the binary because the
//! disclosure, the prompt and the exit code are the behaviour under test and
//! none of them exist in the library.

use comic_renamer_core::preset::preset_by_id;
use comic_renamer_core::types::{
    Mechanism, Mode, PlanHeader, PlanRow, Resolved, SourceStat, Status,
};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
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
    /// This fixture's `XDG_CONFIG_HOME`, so the record store the binary
    /// writes into is this fixture's and not the developer's. Without it the
    /// suite leaves a record of every test run in `Application Support` —
    /// measured, not assumed: 7 records and 22 lock files from one run.
    config: PathBuf,
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
        issue: None,
        subtitle: None,
        run: 10,
        status: Status::Mapped,
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
        config: root.join("config"),
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
    write_plan_as("plan.jsonl", "Hellboy", beside, source_root, dest_root, mode, mechanism, file)
}

/// As `write_plan`, under a chosen plan name and series — so two plans can
/// share one pair of roots without their destinations colliding.
#[allow(clippy::too_many_arguments)]
fn write_plan_as(
    plan_name: &str,
    series: &str,
    beside: &Path,
    source_root: &Path,
    dest_root: &Path,
    mode: Mode,
    mechanism: Mechanism,
    file: &Path,
) -> PathBuf {
    let meta = std::fs::metadata(file).unwrap();
    let resolved = Resolved {
        series: series.into(),
        volume: Some(1),
        issue: None,
        subtitle: None,
        run: 10,
        status: Status::Mapped,
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
        version: comic_renamer_core::plan::PLAN_VERSION,
        preset: "komga".into(),
        mode,
        source_root: source_root.to_path_buf(),
        dest_root: dest_root.to_path_buf(),
        lang: "en".into(),
        required_bytes: 0,
        download_bytes: 0,
        unmapped: 0,
        checksum: String::new(),
    };

    let plan = beside.join(plan_name);
    comic_renamer_core::plan::write(&header, &[row], &plan).unwrap();
    plan
}

fn run(args: &[&str], stdin: &str) -> Output {
    run_with_env(args, stdin, &[])
}

/// `apply` and `undo` both reach the record store, which lives under
/// `config_dir()` — and `apply` reaches it before the prompt, so even a run
/// the user refuses creates the store and its lock file. Every invocation
/// that gets past `clap` therefore goes through here, pointed at the
/// fixture's own directory.
fn run_in(f: &Fixture, args: &[&str], stdin: &str) -> Output {
    run_in_with_env(f, args, stdin, &[])
}

fn run_in_with_env(f: &Fixture, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
    let config = f.config.to_str().unwrap();
    let mut all = vec![("XDG_CONFIG_HOME", config)];
    all.extend_from_slice(env);
    run_with_env(args, stdin, &all)
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
fn every_flag_the_disclosure_advertises_actually_exists() {
    // The unit-test version of this checked for the string "hardlink" and
    // nothing else, so it passed while the block advertised `--copy` against
    // a CLI that only took `--mode copy`. Scrape the flags out of a real
    // disclosure instead of naming the ones to look for.
    let f = fixture(Mode::Move);
    let said = stdout(&run_in(&f, &["apply", f.plan.to_str().unwrap()], "no\n"));
    assert!(said.contains("MOVE MODE"), "no disclosure to scrape:\n{said}");

    let help: String = ["--help", "scan", "apply", "undo"]
        .iter()
        .map(|a| {
            let args: Vec<&str> = if *a == "--help" {
                vec!["--help"]
            } else {
                vec![a, "--help"]
            };
            stdout(&run(&args, ""))
        })
        .collect();

    let mut advertised = 0usize;
    for word in said.split_whitespace() {
        let flag = word.trim_end_matches([',', '.', ':', ';']);
        if !flag.starts_with("--") || flag.len() < 4 {
            continue;
        }
        advertised += 1;
        assert!(
            help.contains(flag),
            "the disclosure advertises {flag}, which no command accepts:\n{said}"
        );
    }
    assert!(advertised > 0, "scraped no flags, so this asserts nothing");
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
    let out = run(&["undo", "--move"], "");
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
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap()], "yes\n");

    assert!(!out.status.success(), "a refusal must not exit zero");
    assert!(stderr(&out).contains("aborted at the prompt"), "{}", stderr(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]), "the tree changed");
    assert!(!f.placed.exists());
}

#[test]
fn an_empty_answer_aborts() {
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap()], "\n");
    assert!(!out.status.success());
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn eof_aborts_rather_than_consenting() {
    // A non-interactive caller must not be taken as having agreed.
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap()], "");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn the_wrong_word_aborts() {
    // Typing COPY at a move prompt is a user who misread the mode.
    let f = fixture(Mode::Move);
    let before = listing(&[&f.source, &f.dest]);
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap()], "COPY\n");
    assert!(!out.status.success());
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn the_disclosure_prints_before_the_prompt() {
    let f = fixture(Mode::Move);
    let out = stdout(&run_in(&f, &["apply", f.plan.to_str().unwrap()], "no\n"));
    let disclosure = out.find("MOVE MODE").expect("no disclosure");
    let prompt = out.find("Type MOVE").expect("no prompt");
    assert!(disclosure < prompt, "the prompt came first:\n{out}");
}

/// The count of files `scan` could not map has no row to carry it, so it rides
/// in the header — and `apply` is a separate invocation, possibly against an
/// old plan. Every plan in this file set it to 0, so the one place `apply`
/// reads it could be replaced by a literal zero with the whole suite green.
#[test]
fn the_plans_unmapped_count_is_disclosed_by_the_binary() {
    let f = fixture(Mode::Move);
    // Through the real reader and writer, as a user editing a plan would be.
    let (mut header, rows) = comic_renamer_core::plan::read(&f.plan).unwrap();
    header.unmapped = 7;
    comic_renamer_core::plan::write(&header, &rows, &f.plan).unwrap();

    let out = stdout(&run_in(&f, &["apply", f.plan.to_str().unwrap()], "no\n"));
    assert!(out.contains("left where they are: 7 of 8 rows"), "{out}");
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

#[test]
fn the_typed_word_moves_the_file_and_undo_puts_it_back() {
    let f = fixture(Mode::Move);
    let original = f.source.join("hellboy_vol1.pdf");
    let before = listing(&[&f.source, &f.dest]);

    let out = run_in(&f, &["apply", f.plan.to_str().unwrap()], "MOVE\n");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.placed.exists(), "{}", stdout(&out));
    assert!(!original.exists(), "a move left the original behind");
    assert_eq!(
        f.placed.file_name().unwrap(),
        "Hellboy Vol. 01.pdf",
        "D37: padded to the series width"
    );

    let out = run_in(&f, &["undo"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(original.exists(), "undo did not restore the original");
    assert!(!f.placed.exists(), "undo left the file in the library");
    assert_eq!(before, listing(&[&f.source, &f.dest]), "not the tree we started with");
}

/// Runs `apply` interactively, lets `meddle` change the world between the
/// disclosure and the confirmation, then types the word. Returns whether the
/// run succeeded and everything it put on stdout.
///
/// That window is the interesting one: the disclosure has told the user what
/// will happen, and nothing they type afterwards should be able to mean
/// something else.
fn apply_through_the_window(f: &Fixture, meddle: impl FnOnce()) -> (bool, String) {
    let mut child = Command::new(EXE)
        .env("XDG_CONFIG_HOME", f.config.to_str().unwrap())
        .args(["apply", f.plan.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Read until the disclosure is out. The `--copy` line is its last, and by
    // then the pre-flight has run and every number has been computed — so
    // everything the user is consenting to is already decided.
    let mut stdout_pipe = BufReader::new(child.stdout.take().unwrap());
    let mut said = String::new();
    loop {
        let mut line = String::new();
        let read = stdout_pipe.read_line(&mut line).unwrap();
        assert!(read > 0, "the disclosure never arrived:\n{said}");
        said.push_str(&line);
        if line.contains("--copy") {
            break;
        }
    }
    assert!(said.contains("MOVE MODE"), "{said}");

    meddle();

    child.stdin.take().unwrap().write_all(b"MOVE\n").unwrap();
    // Drained before waiting: a child blocked writing to a full pipe never
    // exits, and a `wait` that never returns reads exactly like a hang.
    stdout_pipe.read_to_string(&mut said).unwrap();
    (child.wait().unwrap().success(), said)
}

/// Finding 2. The plan is rewritten between the disclosure and the
/// confirmation; what runs is what the user was shown.
///
/// The consent the typed word gives is consent to a particular set of files.
/// A plan re-read after the disclosure makes that consent transferable to
/// whatever is on disk by the time the word is typed — and the plan is a file
/// in the bundle folder, which is a folder other things write into.
#[test]
fn a_plan_rewritten_after_the_disclosure_does_not_run() {
    let f = fixture(Mode::Move);
    let shown = f.source.join("hellboy_vol1.pdf");
    // The file the swapped-in plan moves instead. Same series, so both plans
    // name one destination and the only thing that says which plan ran is
    // *which source is gone*.
    let decoy = f.source.join("decoy.pdf");
    std::fs::File::create(&decoy).unwrap();

    let (ok, said) = apply_through_the_window(&f, || {
        let swapped = write_plan(
            f.source.parent().unwrap(),
            &f.source,
            &f.dest,
            Mode::Move,
            Mechanism::Rename,
            &decoy,
        );
        assert_eq!(
            swapped, f.plan,
            "the rewrite missed the plan the child was given, so this proves nothing"
        );
    });
    assert!(ok, "{said}");

    assert!(
        decoy.exists(),
        "the plan swapped in after the disclosure is the one that ran:\n{said}"
    );
    assert!(
        !shown.exists(),
        "the plan the user was shown did not run:\n{said}"
    );
    assert!(f.placed.exists(), "{said}");
}

/// The one way a run reaches the end having placed nothing and without
/// failing: the destination appears inside the window, so the disclosure's
/// arithmetic counted a row that the run then refuses for a collision.
///
/// Worth pinning because of what gets printed. A run that recorded nothing
/// has nothing to reverse, and offering `comic-renamer undo` after it points
/// the user at a run that does not exist.
#[test]
fn a_run_that_places_nothing_does_not_offer_an_undo() {
    let f = fixture(Mode::Move);
    let (ok, said) = apply_through_the_window(&f, || {
        std::fs::create_dir_all(f.placed.parent().unwrap()).unwrap();
        std::fs::write(&f.placed, b"it got there first").unwrap();
    });

    assert!(said.contains("0 placed"), "{said}");
    assert!(
        !said.contains("comic-renamer undo"),
        "an undo was offered for a run that recorded nothing:\n{said}"
    );
    // The exit code is zero here, and this test does not assert otherwise.
    // The non-zero path belongs to the run that is refused *before* the
    // prompt (`nothing_to_do`); a run that gets consent and then places
    // nothing has always exited zero, and changing that is not this task's.
    let _ = ok;
    // And the file that got there first is untouched.
    assert_eq!(
        std::fs::read(&f.placed).unwrap(),
        b"it got there first",
        "{said}"
    );
}

/// A record is a file the tool owns. The run says it can be reversed and
/// names no path: a printed path invites being handed back in, and a path the
/// user supplies is the authority the record store exists to take away.
#[test]
fn apply_says_undo_is_available_without_naming_the_record() {
    let f = fixture(Mode::Move);
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    let said = stdout(&out);
    assert!(
        said.contains("comic-renamer undo"),
        "a run that placed a file did not say it could be reversed:\n{said}"
    );
    for leak in [
        ".record",
        "/runs",
        "Application Support",
        f.config.to_str().unwrap(),
    ] {
        assert!(!said.contains(leak), "the run names {leak:?}:\n{said}");
    }
}

#[test]
fn yes_skips_the_prompt_but_not_the_disclosure() {
    let f = fixture(Mode::Move);
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.placed.exists());
    // The record still has to exist: --yes is consent, not silence.
    assert!(stdout(&out).contains("MOVE MODE"), "{}", stdout(&out));
}

#[test]
fn a_copy_keeps_the_original() {
    let f = fixture(Mode::Copy);
    let original = f.source.join("hellboy_vol1.pdf");
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.placed.exists());
    assert!(original.exists(), "a copy must not remove the source");
    assert!(stdout(&out).contains("COPY MODE"), "{}", stdout(&out));
}

#[test]
fn applying_a_move_twice_refuses_because_the_source_is_gone() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());

    let before = listing(&[&f.source, &f.dest]);
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "");

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
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    assert!(f.placed.exists());

    // Distinguishable content, so an overwrite cannot look like a no-op.
    std::fs::write(&f.placed, b"the file already in the library").unwrap();
    let before = listing(&[&f.source, &f.dest]);

    let out = run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "");
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
                r#""status":"mapped""#,
                r#""status":"unmapped""#
            )
        ),
    )
    .unwrap();

    let before = listing(&[&f.source, &f.dest]);
    let out = run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "");
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
    let out = run_in(&f, &["undo"], "");
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
    // The store is reached before the prompt, so even this refusal would
    // otherwise create one under the developer's own config directory.
    let config = dir.path().join("config");
    let out = run_with_env(
        &["apply", plan.to_str().unwrap()],
        "no\n",
        &[("XDG_CONFIG_HOME", config.to_str().unwrap())],
    );
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
        let out = run_in_with_env(
            &f,
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
    let out = run_in_with_env(
        &f,
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
    let out = run_in(
        &f,
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
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
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

    let out = run_in(&f, &["apply", f.plan.to_str().unwrap()], "MOVE\n");
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
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());

    // Something else is at the destination now, so this is not the file the
    // run put there and reversing it would move a stranger's file.
    std::fs::write(&f.placed, b"a different file entirely").unwrap();

    let out = run_in(&f, &["undo"], "");
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

// ---------------------------------------------------------------------------
// map
// ---------------------------------------------------------------------------

fn map_cmd(f: &Fixture, config: &Path) -> Command {
    let mut c = Command::new(EXE);
    c.current_dir(f.source.parent().unwrap())
        .env("XDG_CONFIG_HOME", config)
        .arg("map")
        .arg(&f.source)
        .arg("--dest")
        .arg(&f.dest)
        .stdin(Stdio::null());
    c
}

#[test]
fn map_over_a_pipe_with_unmapped_files_is_refused_and_writes_no_plan() {
    let f = fixture(Mode::Move);
    let config = TempDir::new().unwrap();
    let out = map_cmd(&f, config.path()).output().unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("terminal"), "{}", stderr(&out));
    assert!(!stdout(&out).contains('\x1b'));
    assert!(!f.source.parent().unwrap().join("comic-renamer-plan.jsonl").exists());
    assert!(f.source.join("hellboy_vol1.pdf").exists());
}

#[test]
fn map_with_nothing_unmapped_needs_no_terminal_and_writes_the_plan() {
    let f = fixture(Mode::Move);
    let config = TempDir::new().unwrap();
    let dir = config.path().join("comic-renamer");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mappings.tsv"),
        "key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count\tbundle\n\
         hellboyvol1\tHellboy\t1\t\t\ten\t\t\n",
    )
    .unwrap();
    let out = map_cmd(&f, config.path()).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("nothing needed mapping"), "{}", stdout(&out));
    assert!(f.source.parent().unwrap().join("comic-renamer-plan.jsonl").exists());
}

// ---------------------------------------------------------------------------
// undo names a run
// ---------------------------------------------------------------------------

/// The record store the binary writes into for this fixture.
fn runs_dir(f: &Fixture) -> PathBuf {
    f.config.join("comic-renamer").join("runs")
}

fn record_files(f: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(runs_dir(f))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".record"))
        .collect();
    names.sort();
    names
}

/// A second bundle and library, applied under the same record store.
struct Second {
    original: PathBuf,
    placed: PathBuf,
}

/// Applies the fixture's own plan, then a second library's. Each apply is its
/// own process, so the second is the newer run by the nanosecond stamp in its
/// record's name; no pause is needed between them.
fn two_runs(f: &Fixture) -> Second {
    assert!(run_in(f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());

    let root = f._dir.path().canonicalize().unwrap().join("two");
    let source = root.join("bundle");
    let dest = root.join("library");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    let original = source.join("hellboy_vol1.pdf");
    std::fs::File::create(&original).unwrap();
    let plan = write_plan(&root, &source, &dest, Mode::Move, Mechanism::Rename, &original);
    let relative = f.placed.strip_prefix(&f.dest).unwrap();
    let out = run_in(f, &["apply", plan.to_str().unwrap(), "--yes"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    Second {
        original,
        placed: dest.join(relative),
    }
}

fn listed_ids(out: &Output) -> Vec<String> {
    stdout(out)
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[test]
fn undo_list_names_each_run_without_reversing_anything() {
    let f = fixture(Mode::Move);
    let second = two_runs(&f);
    let before = listing(&[&f.source, &f.dest]);
    let records_before = record_files(&f);
    assert_eq!(records_before.len(), 2);

    let out = run_in(&f, &["undo", "--list"], "");
    assert!(out.status.success(), "{}", stderr(&out));

    let mut ids = listed_ids(&out);
    ids.sort();
    assert_eq!(ids, records_before, "{}", stdout(&out));
    assert!(f.placed.exists() && second.placed.exists());
    assert_eq!(before, listing(&[&f.source, &f.dest]));
    assert_eq!(records_before, record_files(&f), "--list removed a record");
}

#[test]
fn undo_list_with_no_record_store_says_so_and_succeeds() {
    // Nothing has been applied, so the store was never created.
    let f = fixture(Mode::Move);
    assert!(!runs_dir(&f).exists());
    let out = run_in(&f, &["undo", "--list"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("no runs to reverse"), "{}", stdout(&out));
}

#[test]
fn undo_list_is_newest_first_and_names_no_path() {
    let f = fixture(Mode::Move);
    two_runs(&f);
    let out = run_in(&f, &["undo", "--list"], "");
    let ids = listed_ids(&out);
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.reverse();
    assert_eq!(ids, sorted, "newest first: {}", stdout(&out));
    // An id is a file name, so it carries the `.record` suffix; what must
    // never appear is a path. That is why this test checks for `/` in the ids
    // and the other undo output tests check for `.record`: they hold the same
    // rule, applied to output that does and does not print ids.
    for id in &ids {
        assert!(!id.contains('/'), "an id that is a path: {id}");
    }
    let said = stdout(&out) + &stderr(&out);
    for leak in ["/runs", "Application Support", f.config.to_str().unwrap()] {
        assert!(!said.contains(leak), "--list names {leak:?}:
{said}");
    }
}

#[test]
fn undo_with_no_argument_reverses_the_most_recent_run() {
    let f = fixture(Mode::Move);
    let second = two_runs(&f);

    let out = run_in(&f, &["undo"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(second.original.exists(), "the newest run was not reversed");
    assert!(!second.placed.exists());
    assert!(f.placed.exists(), "the older run was reversed too");
    assert_eq!(record_files(&f).len(), 1, "the older record must remain");
}

#[test]
fn undo_by_run_id_reverses_that_run_and_not_the_newest() {
    let f = fixture(Mode::Move);
    let second = two_runs(&f);
    let ids = listed_ids(&run_in(&f, &["undo", "--list"], ""));
    let older = ids.last().unwrap();

    let out = run_in(&f, &["undo", older], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.source.join("hellboy_vol1.pdf").exists(), "the named run was not reversed");
    assert!(!f.placed.exists());
    assert!(second.placed.exists(), "the newest run was reversed instead");
}

#[test]
fn a_run_id_is_a_name_and_never_a_path() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    let record = runs_dir(&f).join(&record_files(&f)[0]);
    let before = listing(&[&f.source, &f.dest]);

    // The last two are real: the plan, and the record's own absolute path.
    // Both name something undo could act on, and neither is a run id.
    for bad in [
        "../../etc/passwd",
        "/etc/passwd",
        "comic-renamer-plan.jsonl",
        f.plan.to_str().unwrap(),
        record.to_str().unwrap(),
    ] {
        let out = run_in(&f, &["undo", bad], "");
        assert!(!out.status.success(), "{bad}: {}", stdout(&out));
        assert!(stderr(&out).contains("no run named"), "{bad}: {}", stderr(&out));
    }
    assert_eq!(before, listing(&[&f.source, &f.dest]), "a bad id moved a file");
    assert!(record.exists(), "a bad id removed the record");
}

#[test]
fn undo_works_after_the_bundle_folder_is_deleted() {
    // The common case: move a bundle in, delete the emptied folder, change
    // your mind. Roots come from the record, so the missing folder is not
    // something the lookup needs.
    for by_id in [true, false] {
        let f = fixture(Mode::Move);
        assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
            .status
            .success());
        std::fs::remove_dir(&f.source).unwrap();

        let id = record_files(&f).remove(0);
        let args: Vec<&str> = if by_id { vec!["undo", &id] } else { vec!["undo"] };
        let out = run_in(&f, &args, "");
        assert!(out.status.success(), "by_id={by_id}: {}", stderr(&out));
        assert!(
            f.source.join("hellboy_vol1.pdf").exists(),
            "by_id={by_id}: not restored"
        );
        assert!(!f.placed.exists(), "by_id={by_id}");
    }
}

#[test]
fn undo_prints_no_record_path_and_leaves_no_trace_of_one() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    // A refused row exercises the error path as well as the report.
    std::fs::write(&f.placed, b"a different file entirely").unwrap();

    let out = run_in(&f, &["undo"], "");
    assert!(!out.status.success());
    let said = stdout(&out) + &stderr(&out);
    for leak in [
        ".record",
        "/runs",
        "Application Support",
        f.config.to_str().unwrap(),
    ] {
        assert!(!said.contains(leak), "undo names {leak:?}:
{said}");
    }
}

#[test]
fn undo_rejects_a_run_together_with_list() {
    let out = run(&["undo", "--list", "some-run"], "");
    assert!(!out.status.success());
}

#[test]
fn undo_with_no_argument_skips_a_run_that_is_still_going() {
    let f = fixture(Mode::Move);
    let second = two_runs(&f);
    let newest = runs_dir(&f).join(record_files(&f).last().unwrap());
    // What a live `apply` does to its own record.
    let _held = comic_renamer_core::record::hold(&newest).unwrap();

    let listed = stdout(&run_in(&f, &["undo", "--list"], ""));
    let line_for = |id: &str| listed.lines().find(|l| l.starts_with(id)).unwrap().to_string();
    let ids = record_files(&f);
    assert!(line_for(&ids[1]).contains("running"), "{listed}");
    assert!(!line_for(&ids[0]).contains("running"), "{listed}");

    let out = run_in(&f, &["undo"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(second.placed.exists(), "reversed a record a live run holds");
    assert!(!f.placed.exists(), "the older run was not reversed instead");
    assert!(newest.exists());
}

/// The other half of the same filter, and the half that was silent. With a
/// live run and *nothing else* to reverse, `undo` printed
/// `0 restored · 0 skipped · 0 refused` and exited 0 — told, with a success
/// code, that there was nothing to undo at the moment 156 of 600 files had
/// moved. "Never applied" and "a run is writing right now" are different
/// answers and only `undo --list` had the second.
#[test]
fn undo_says_a_run_is_in_progress_rather_than_that_there_is_nothing_to_do() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    let only = runs_dir(&f).join(record_files(&f).remove(0));
    // What a live `apply` holds on its own record.
    let _held = comic_renamer_core::record::hold(&only).unwrap();

    let out = run_in(&f, &["undo"], "");
    let said = stdout(&out) + &stderr(&out);
    assert!(!out.status.success(), "a silent success: {said}");
    assert!(said.contains("a run is in progress"), "{said}");
    assert!(
        !said.contains("0 restored"),
        "the counts say nothing was there to undo: {said}"
    );
    // And it is still a report about runs, not about files: no record path.
    for leak in [".record", "/runs", f.config.to_str().unwrap()] {
        assert!(!said.contains(leak), "undo names {leak:?}:\n{said}");
    }
    assert!(f.placed.exists(), "nothing was touched");
}

/// Where the files went back to. A record that names a `source_root` of its
/// own relocates the file it names — D54's accepted residual — but the report
/// printed counts and no path, `--list` showed only the library, and the
/// record is deleted when nothing is refused: after the fact nothing on the
/// system said where.
#[test]
fn undo_names_the_folder_it_put_the_files_back_into() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());

    let listed = run_in(&f, &["undo", "--list"], "");
    assert!(
        stdout(&listed).contains(f.source.to_str().unwrap()),
        "--list never said which bundle a run came from: {}",
        stdout(&listed)
    );

    let out = run_in(&f, &["undo"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(f.source.to_str().unwrap()),
        "the report named no destination for the restored file: {}",
        stdout(&out)
    );
    assert!(f.source.join("hellboy_vol1.pdf").exists());
}

/// And it says it only when something moved. A reverse that refused its only
/// row restored nothing, so naming the bundle folder would claim a file had
/// gone back into it — which is the opposite of what happened, and the row is
/// still in the library.
#[test]
fn a_reverse_that_restored_nothing_names_no_folder() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    // Something else holds the original name, so the row cannot go back.
    std::fs::write(f.source.join("hellboy_vol1.pdf"), b"squatter").unwrap();

    let out = run_in(&f, &["undo"], "");
    assert!(!out.status.success(), "a refused row is not a clean reverse");
    let said = stdout(&out);
    assert!(said.contains("0 restored"), "{said}");
    assert!(!said.contains("put back into"), "{said}");
    assert!(f.placed.exists(), "the refused row is still in the library");
}

#[test]
fn undo_with_no_argument_steps_over_a_record_it_cannot_read() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    // Newer than any real record, and not one.
    std::fs::write(
        runs_dir(&f).join(".comic-renamer.9000000000000000000.1.0.record"),
        b"not a record",
    )
    .unwrap();

    let out = run_in(&f, &["undo"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.source.join("hellboy_vol1.pdf").exists());

    let listed = run_in(&f, &["undo", "--list"], "");
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(!stdout(&listed).contains("9000000000000000000"), "{}", stdout(&listed));
}

#[test]
fn undo_refuses_while_the_library_is_locked() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    let _lock = comic_renamer_core::apply::Lock::library(&runs_dir(&f), &f.dest).unwrap();
    let before = listing(&[&f.source, &f.dest]);

    let out = run_in(&f, &["undo"], "");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert_eq!(before, listing(&[&f.source, &f.dest]));
}

#[test]
fn undo_does_not_rebuild_a_bundle_folder_when_the_library_is_gone() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    std::fs::remove_dir(&f.source).unwrap();
    std::fs::remove_dir_all(&f.dest).unwrap();

    let out = run_in(&f, &["undo", &record_files(&f)[0]], "");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(!f.source.exists(), "recreated a folder with nothing to put in it");
}

#[test]
fn the_menu_refuses_without_a_terminal() {
    let f = fixture(Mode::Move);
    let out = run_in(&f, &[], "");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("the menu needs an interactive terminal"), "{}", stderr(&out));
    assert!(!(stdout(&out) + &stderr(&out)).contains(f.config.to_str().unwrap()));
}

/// The sweep's count reaches the terminal, not just `apply`'s progress
/// channel. It was discarded, so re-importing a bundle took the earlier run's
/// undo in silence — the user would find out by running `undo` and being told
/// there was nothing to do. Two plans over one pair of roots, each with its
/// own series so the second run executes rather than colliding.
#[test]
fn re_importing_a_bundle_says_which_undo_the_sweep_took() {
    let f = fixture(Mode::Move);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    assert_eq!(record_files(&f).len(), 1, "the first run left no record");

    let root = f.plan.parent().unwrap().to_path_buf();
    let other = f.source.join("velvet_vol1.pdf");
    std::fs::File::create(&other).unwrap();
    let second = write_plan_as(
        "plan-b.jsonl", "Velvet", &root, &f.source, &f.dest, Mode::Move,
        Mechanism::Rename, &other,
    );

    let out = run_in(&f, &["apply", second.to_str().unwrap(), "--yes"], "");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stderr(&out).contains("1 earlier run(s) of this bundle can no longer be undone"),
        "the sweep took the first run's record in silence: {}",
        stderr(&out)
    );
    assert_eq!(record_files(&f).len(), 1, "both records survived, so nothing was swept");
}

/// D58: reversing a copy moves the library file aside, and the report says
/// where it went, how many are there, and that nothing empties it.
///
/// `Mode::Copy`, which every other undo test here is not: a copy is the only
/// mode whose reverse has a library file to deal with, and the whole decision
/// is about what happens to it.
#[test]
fn undo_of_a_copy_says_where_the_library_file_was_moved_aside() {
    let f = fixture(Mode::Copy);
    assert!(run_in(&f, &["apply", f.plan.to_str().unwrap(), "--yes"], "")
        .status
        .success());
    assert!(f.placed.exists());

    let out = run_in(&f, &["undo"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    let said = stdout(&out);
    assert!(said.contains("1 restored"), "{said}");
    assert!(said.contains("1 file moved aside into"), "{said}");
    assert!(said.contains(".comic-renamer-aside"), "{said}");
    assert!(
        said.contains("nothing deletes them for you"),
        "the report has to say the directory is never emptied: {said}"
    );
    assert!(!f.placed.exists(), "the destination was cleared");
    assert!(f.source.join("hellboy_vol1.pdf").exists());

    // And the file is really where the report said. Zero bytes (invariant 3),
    // so existence is the whole claim.
    let aside = f.dest.join(".comic-renamer-aside");
    let found: Vec<PathBuf> = std::fs::read_dir(&aside)
        .unwrap()
        .flatten()
        .flat_map(|run| std::fs::read_dir(run.path()).unwrap().flatten().collect::<Vec<_>>())
        .map(|e| e.path())
        .collect();
    assert_eq!(
        found.len(),
        1,
        "one run directory holding one file, found {found:?}"
    );
    assert_eq!(
        found[0].file_name(),
        f.placed.file_name(),
        "the name is the leaf the library used"
    );
}
