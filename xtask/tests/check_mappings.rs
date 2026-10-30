//! Drives the real binary in a throwaway repository, so the diff arguments, the
//! base-ref argument position and the exit code are all under test.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn vcs(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

fn repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xtask-check-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    vcs(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("data/decisions.tsv"), "key\n").unwrap();
    vcs(&dir, &["add", "."]);
    vcs(&dir, &["commit", "-q", "-m", "base"]);
    vcs(&dir, &["checkout", "-q", "-b", "pr"]);
    dir
}

fn add_rows(dir: &Path, file: &str, n: usize) {
    let rows: String = (0..n).map(|i| format!("row{i}\n")).collect();
    std::fs::write(dir.join("data").join(file), rows).unwrap();
    vcs(dir, &["add", "."]);
    vcs(dir, &["commit", "-q", "-m", "rows"]);
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("check-mappings")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

/// Against `data/decisions.tsv` itself, not a scratch `b.tsv`/`c.tsv` pair.
///
/// Two reasons it changed on 2026-10-30. The pathspec narrowed to the one
/// shipped table, so rows added to any other file are correctly not counted —
/// a test planting `data/b.tsv` was measuring a path the cap no longer governs.
/// And a second `.tsv` under `data/` is now itself a build failure, so the old
/// fixture exercised a state the repository is forbidden to be in.
#[test]
fn a_pr_at_the_row_cap_passes_and_one_over_fails() {
    let dir = repo("rows");
    add_rows(&dir, "decisions.tsv", 2500);
    let at = run(&dir, &["main"]);
    assert!(at.status.success(), "{}", String::from_utf8_lossy(&at.stderr));

    add_rows(&dir, "decisions.tsv", 2501);
    let over = run(&dir, &["main"]);
    assert_eq!(over.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&over.stderr).contains("2501"), "{}", String::from_utf8_lossy(&over.stderr));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_base_ref_fails_rather_than_passing_silently() {
    let dir = repo("badref");
    let out = run(&dir, &["no-such-ref"]);
    assert_eq!(out.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Note what this does and does not pin. It writes 4 MiB + 1 and expects a
/// refusal, which a 2 MiB cap would also give — so it proves the walk finds the
/// file, not that the cap is 4 MiB. **The boundary itself is pinned by
/// `a_file_at_the_cap_passes_and_one_byte_over_does_not` in `xtask/src/lib.rs`**,
/// which was the only test to fail when D67's constant was reverted.
#[test]
fn a_tsv_too_big_is_refused_without_a_base_ref() {
    let dir = repo("size");
    assert!(run(&dir, &[]).status.success());
    // 4 MiB since D67, and the reason is the binary rather than the file: the
    // table is `include_str!`-embedded, so every byte here is a byte shipped.
    std::fs::write(dir.join("data/big.tsv"), vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
    let out = run(&dir, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("big.tsv"));
    let _ = std::fs::remove_dir_all(&dir);
}
