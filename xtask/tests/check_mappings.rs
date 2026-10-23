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
    std::fs::create_dir_all(dir.join("data/mappings")).unwrap();
    vcs(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("data/mappings/a.tsv"), "key\n").unwrap();
    vcs(&dir, &["add", "."]);
    vcs(&dir, &["commit", "-q", "-m", "base"]);
    vcs(&dir, &["checkout", "-q", "-b", "pr"]);
    dir
}

fn add_rows(dir: &Path, file: &str, n: usize) {
    let rows: String = (0..n).map(|i| format!("row{i}\n")).collect();
    std::fs::write(dir.join("data/mappings").join(file), rows).unwrap();
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

#[test]
fn a_pr_at_the_row_cap_passes_and_one_over_fails() {
    let dir = repo("rows");
    add_rows(&dir, "b.tsv", 2500);
    let at = run(&dir, &["main"]);
    assert!(at.status.success(), "{}", String::from_utf8_lossy(&at.stderr));

    add_rows(&dir, "c.tsv", 1);
    let over = run(&dir, &["main"]);
    assert_eq!(over.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&over.stderr).contains("2501"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_base_ref_fails_rather_than_passing_silently() {
    let dir = repo("badref");
    let out = run(&dir, &["no-such-ref"]);
    assert_eq!(out.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_over_two_mebibytes_fails_without_a_base_ref() {
    let dir = repo("size");
    assert!(run(&dir, &[]).status.success());
    std::fs::write(dir.join("data/mappings/big.tsv"), vec![b'x'; 2 * 1024 * 1024 + 1]).unwrap();
    let out = run(&dir, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("big.tsv"));
    let _ = std::fs::remove_dir_all(&dir);
}
