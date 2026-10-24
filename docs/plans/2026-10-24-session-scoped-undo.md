# Session-scoped undo and a startup menu — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the record of what a run did from a file the user hands in to a file
the tool owns, take away its authority to authorise a deletion, and reach it from a
startup menu and a run-keyed `undo`.

**Architecture:** A new `core::record` module owns where records live, how they are
named, enumerated, liveness-tested and superseded; it takes the directory as an
argument so core stays free of `config_dir()`. `core::undo` keeps the CSV format and
grows the five per-row fields a preset needs plus two roots-row fields, so `reverse`
recomputes destinations itself and no longer takes them from its caller. Deleting a
copied file is gated on a content check against the filesystem, not on the record.
`apply` and `check` take a plan the caller has already read, which closes the
confirm-then-re-read window.

**Tech Stack:** Rust 1.88, `libc` (`flock`), `sha2` (already a dependency, used by
`fsops`), `ratatui`/`crossterm` (already present, for the menu if a TUI is chosen).

**Spec:** `docs/specs/2026-10-24-session-scoped-undo-and-startup-menu.md`

## Global Constraints

- Rust edition and floor: `rust-version` is **1.88**; do not raise it.
- macOS only. `renamex_np`, `clonefile`, `SF_DATALESS` and `flock` are all already used.
- **Invariant 1:** presets are the only code that constructs paths. `reverse`
  recomputing a destination must go through `Preset::destination`.
- **Invariant 2:** `apply` never trusts a path string in a plan. Unchanged by this work.
- **Invariant 6:** no-clobber at the syscall. The move-reverse keeps `renamex_np(RENAME_EXCL)`.
- **Invariant 7:** collision comparison is NFC-normalised and case-folded, via
  `normalize::collision_key`. Applies to the lock name derived from `dest_root`.
- **New invariant 11:** a record never authorises its own deletion.
- Do not run `cargo fmt`; `rustfmt` is not installed and the tree has never been through it.
- Do not reformat files you are not otherwise changing.
- `cargo test --workspace` must end with 0 failures and **0 compiler warnings**.
  Baseline at the start of this plan: 409 passed, 7 ignored.
- `cargo xtask check-mappings` must pass and `cargo xtask measure` must still report
  `located` 91.6%.
- **`timeout` is not installed on this machine.** `timeout N cargo test` produces no
  output and looks exactly like a hang. The suite runs in about 3 seconds.
- Never run `git push`, `gh pr create`, or any remote-touching command.

## Review Focus

Five conditions the spec implies and whose absence would bite a user. Each has a
test in the task that owns the code.

1. **A record whose roots no longer exist** — library unmounted or moved since the
   run. Every row refuses with a reason naming the root; no panic, no partial
   reverse reported as clean. (Task 2)
2. **A live record for the same root pair** — supersede must never delete a record
   whose run is still going, or a crash in run B destroys run A's only way back.
   (Task 1)
3. **A record written by an older build** — wrong column count must refuse with a
   message saying so, never misparse a column into the wrong field. (Task 2)
4. **The records directory cannot be written** — no `HOME`, no `XDG_CONFIG_HOME`, or
   the directory is unwritable. `apply` **refuses before placing anything** rather
   than running without a record: a run nobody can reverse is exactly the case a
   user would want to be asked about. (Task 4)
5. **A `dest_root` whose path is awkward as a filename** — spaces, `/`, non-ASCII,
   or longer than 255 bytes. The lock filename is derived by hashing the canonical
   root, never by escaping it. (Task 1)

---

## Task 1: The record store

**Files:**
- Create: `crates/core/src/record.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod record;`)
- Test: `crates/core/tests/record.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  ```rust
  pub fn dir_in(config_dir: &Path) -> PathBuf;              // config_dir.join("runs")
  pub fn ensure_dir(dir: &Path) -> std::io::Result<()>;
  pub fn basename(now: std::time::SystemTime) -> String;
  pub fn lock_name(dest_root: &Path) -> String;
  pub fn is_live(path: &Path) -> std::io::Result<bool>;
  pub fn hold(path: &Path) -> std::io::Result<std::fs::File>;
  ```
  `basename` is `.comic-renamer.<unix-secs>.<pid>.<seq>.record`, the
  pid-plus-counter shape of `fsops::temp_basename` plus a wall-clock stamp.
  `lock_name` is `<hex sha256 of collision_key(canonical dest_root)>.lock`.
  `hold` takes an exclusive non-blocking `flock` and returns the file whose
  lifetime holds it. `is_live` returns `true` when `hold` would fail with
  `EWOULDBLOCK`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_basename_is_unique_per_call_and_carries_the_stamp() {
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_000_000);
    let a = basename(now);
    let b = basename(now);
    assert_ne!(a, b, "two records in one process must not collide");
    assert!(a.contains("1793000000"), "{a}");
    assert!(a.starts_with(".comic-renamer."), "{a}");
    assert!(a.ends_with(".record"), "{a}");
}

/// Review Focus 5. A root is a path, not a filename, so it is hashed. Spaces,
/// separators, non-ASCII and 300 bytes all have to produce one usable name.
#[test]
fn a_lock_name_is_a_filename_whatever_the_root_looks_like() {
    for root in ["/Volumes/My Comics/lib", "/a/b/c", &format!("/{}", "x".repeat(300)), "/Volumes/Caf\u{e9}"] {
        let name = lock_name(Path::new(root));
        assert!(name.len() <= 255, "{root}: {}", name.len());
        assert_eq!(Path::new(&name).components().count(), 1, "{root}: {name}");
        assert!(name.ends_with(".lock"), "{name}");
    }
}

/// Invariant 7 reaches the lock: one library must not be two locks.
#[test]
fn one_library_is_one_lock_whatever_its_case_or_composition() {
    assert_eq!(
        lock_name(Path::new("/Volumes/Hellboy")),
        lock_name(Path::new("/Volumes/hellboy")),
        "case-folded"
    );
    assert_eq!(
        lock_name(Path::new("/Volumes/Caf\u{e9}")),
        lock_name(Path::new("/Volumes/Cafe\u{301}")),
        "NFC-normalised"
    );
    assert_ne!(
        lock_name(Path::new("/Volumes/Hellboy")),
        lock_name(Path::new("/Volumes/Hellboy2")),
        "and still distinguishes two libraries"
    );
}

/// Review Focus 2, half of it: a held record reads as live.
#[test]
fn a_held_record_is_live_and_a_released_one_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.record");
    std::fs::write(&path, "x").unwrap();
    assert!(!is_live(&path).unwrap(), "nobody holds it yet");
    let held = hold(&path).unwrap();
    assert!(is_live(&path).unwrap(), "held in this process still conflicts");
    drop(held);
    assert!(!is_live(&path).unwrap(), "released");
}

#[test]
fn ensure_dir_is_idempotent_and_creates_parents() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir_in(&dir.path().join("comic-renamer"));
    ensure_dir(&runs).unwrap();
    ensure_dir(&runs).unwrap();
    assert!(runs.is_dir());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test record`
Expected: FAIL, unresolved module `record`.

- [ ] **Step 3: Implement the six functions in `crates/core/src/record.rs`**

`lock_name` hashes `normalize::collision_key(&canonical.to_string_lossy())` with
`sha2::Sha256` and hex-encodes it. `hold` uses `libc::flock` with
`LOCK_EX | LOCK_NB`, mirroring `apply::Lock::acquire`; `is_live` calls `hold` and
maps `EWOULDBLOCK` to `true`, any other error through.

- [ ] **Step 4: Run them to verify they pass, and the suite stays green**

Run: `cargo test -p comic-renamer-core --test record` then `cargo test --workspace`
Expected: PASS; workspace 414 passed, 0 failed, 7 ignored, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/record.rs crates/core/src/lib.rs crates/core/tests/record.rs
git commit -m "feat(record): where a run's record lives, and how to tell a live one"
```

---

## Task 2: The record carries what a reverse needs

**Files:**
- Modify: `crates/core/src/undo.rs` (`COLUMNS`, `UndoWriter::create`,
  `record_pending`, `Row`, `read_manifest`, `reverse`)
- Test: `crates/core/tests/undo_schema.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  ```rust
  pub const COLUMNS: &str = "seq,status,mode,mechanism,source_name,dest_relative,\
                             size,inode,mtime_ns,series,volume,subtitle,run,extension,\
                             preset,planned";
  pub struct Summary {
      pub source_root: PathBuf,
      pub dest_root: PathBuf,
      pub preset: String,
      pub planned: u32,
      pub placed: u32,
      pub interrupted: bool,
  }
  pub fn summarise(manifest: &Path) -> Result<Summary, UndoError>;
  pub fn reverse(manifest: &Path) -> Result<UndoReport, UndoError>;
  ```
  `UndoWriter::create(path, source_root, dest_root, preset, planned)` gains the
  last two. `record_pending` gains `series: &str, volume: Option<u32>,
  subtitle: Option<&str>, run: u32, extension: &str`. `reverse` **loses**
  `source_root`, `dest_root` and `recomputed`: the record carries all three, and
  it canonicalises the roots and recomputes each destination through
  `preset_by_id(&summary.preset)` itself. `interrupted` is true when any row's
  terminal state is missing.

- [ ] **Step 1: Write the failing tests**

```rust
/// The blocking defect the design was revised for: with the plan gone there
/// has to be enough in the record to rebuild every destination.
#[test]
fn a_record_alone_rebuilds_every_destination() {
    let (dir, record) = a_written_record();   // one Copy row, series "Hellboy", volume 1, run 3
    let s = summarise(&record).unwrap();
    assert_eq!(s.preset, "komga");
    assert_eq!(s.planned, 1);
    assert_eq!(s.placed, 1);
    assert!(!s.interrupted);
    // No plan is passed, and no caller-supplied destinations.
    let report = reverse(&record).unwrap();
    assert_eq!(report.refused, Vec::<String>::new());
    assert_eq!(report.restored, 1);
    drop(dir);
}

/// Review Focus 3. A record from an older build must say so.
#[test]
fn a_record_with_the_old_columns_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.record");
    std::fs::write(&path, "seq,status,mode,mechanism,source_name,dest_relative,size,inode,mtime_ns\n").unwrap();
    let err = summarise(&path).unwrap_err().to_string();
    assert!(err.contains("older build"), "{err}");
}

/// Review Focus 1. The library is gone; every row refuses with a reason, and
/// the report does not read as a clean undo.
#[test]
fn a_record_whose_roots_are_gone_refuses_every_row() {
    let (dir, record) = a_written_record();
    std::fs::remove_dir_all(dir.path().join("library")).unwrap();
    let report = reverse(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(!report.refused.is_empty(), "a missing library is refusals, not silence");
    assert!(report.refused.iter().any(|r| r.contains("library")), "{:?}", report.refused);
    drop(dir);
}

#[test]
fn the_roots_row_is_not_a_data_row() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    assert!(text.lines().next().unwrap() == COLUMNS, "the header is the column list");
    assert_eq!(text.lines().filter(|l| l.contains(",roots,")).count(), 1);
    drop(dir);
}
```

`a_written_record()` is a helper in the test file: makes a tempdir with
`source/` and `library/`, writes one file, drives `UndoWriter` through
`create` → `record_pending` → `record_placed` → `record_done` for a Copy row,
and returns the tempdir and the record's path.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test undo_schema`
Expected: FAIL — `summarise` not found, `reverse` takes four arguments.

- [ ] **Step 3: Grow `COLUMNS` and the writer, then narrow `reverse`**

Fill the five new per-row columns on `pending` rows only and the two roots-row
columns on the roots row; leave each blank where it does not apply, as the
existing rows already do. `read_manifest` compares the header to `COLUMNS`
exactly and returns the "older build" error on any mismatch. Delete the
`recorded_source != source_root` cross-check and its test: with the record
supplying both sides it is `x == x` (design §6).

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p comic-renamer-core --test undo_schema` then `cargo test --workspace`
Expected: PASS. Call sites in `apply.rs` and `apply_refusals.rs` are updated in Task 4,
so the workspace build is expected to fail until then **only if** you change
`apply.rs` in this task — do not. Keep `apply::undo` compiling by passing the
record path through; it is rewritten in Task 4.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/undo.rs crates/core/tests/undo_schema.rs
git commit -m "feat(undo): the record carries the fields a destination is rebuilt from"
```

---

## Task 3: A record never authorises its own deletion

**Files:**
- Modify: `crates/core/src/undo.rs` (`reverse_one`'s Copy arm)
- Modify: `crates/core/src/fsops.rs` (expose a hash of a path)
- Test: `crates/core/tests/undo_authority.rs`

**Interfaces:**
- Consumes: Task 2's `reverse(manifest)`.
- Produces:
  ```rust
  // fsops
  pub fn sha256_of(path: &Path) -> std::io::Result<String>;
  // undo
  pub enum Trust { InSession, FromDisk }
  pub fn reverse_with(manifest: &Path, trust: Trust) -> Result<UndoReport, UndoError>;
  ```
  `reverse(manifest)` becomes `reverse_with(manifest, Trust::FromDisk)`.
  Under `Trust::FromDisk`, a Copy row's destination is unlinked **only** when a
  file exists at `source_root/source_name` and `sha256_of` agrees for both.
  Under `Trust::InSession` the existing identity check alone applies.
  `sha256_of` opens with `open_nofollow` and reuses the existing hashing loop.

- [ ] **Step 1: Write the failing tests**

```rust
/// The executed reproduction from
/// docs/research/2026-10-23-two-pre-existing-data-safety-findings.md, as a
/// regression test. A planted record names a library file and carries its real
/// identity; nothing was ever copied from the source it names.
#[test]
fn a_planted_record_does_not_delete_a_library_file() {
    let f = planted();           // 5-byte source, 33-byte library file, forged identity
    let report = reverse_with(&f.record, Trust::FromDisk).unwrap();
    assert_eq!(report.restored, 0, "it restored nothing, because it placed nothing");
    assert!(!report.refused.is_empty(), "and it says so: {:?}", report.refused);
    assert!(f.library_file.exists(), "the file no run placed is still there");
}

#[test]
fn a_real_copy_is_reversed_because_the_source_still_matches() {
    let f = real_copy();         // source and destination are the same bytes
    let report = reverse_with(&f.record, Trust::FromDisk).unwrap();
    assert_eq!(report.restored, 1);
    assert!(!f.library_file.exists(), "the copy is gone");
    assert!(f.source_file.exists(), "the original is not");
}

/// Same bytes at the source is what authorises it, so a source that differs
/// refuses even though the record's identity matches the destination exactly.
#[test]
fn a_source_with_different_content_refuses_the_delete() {
    let f = real_copy();
    std::fs::write(&f.source_file, b"different bytes entirely").unwrap();
    let report = reverse_with(&f.record, Trust::FromDisk).unwrap();
    assert_eq!(report.restored, 0);
    assert!(f.library_file.exists());
}

/// In-session skips the content check, which is what keeps "I just ran this,
/// undo it" as fast as it is now.
#[test]
fn in_session_reverses_without_reading_the_source() {
    let f = real_copy();
    std::fs::remove_file(&f.source_file).unwrap();
    let report = reverse_with(&f.record, Trust::InSession).unwrap();
    assert_eq!(report.restored, 1, "the in-memory record is trusted: {:?}", report.refused);
}

/// Reversing a move is unaffected: RENAME_EXCL already refuses to clobber.
#[test]
fn a_move_is_reversed_without_a_content_check() {
    let f = real_move();
    let report = reverse_with(&f.record, Trust::FromDisk).unwrap();
    assert_eq!(report.restored, 1);
    assert!(f.source_file.exists());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test undo_authority`
Expected: FAIL — `a_planted_record_does_not_delete_a_library_file` deletes the file,
which is the finding.

- [ ] **Step 3: Implement the check**

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p comic-renamer-core --test undo_authority` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/undo.rs crates/core/src/fsops.rs crates/core/tests/undo_authority.rs
git commit -m "fix(undo): a copied file is deleted only if its source still matches"
```

---

## Task 4: `apply` writes to the store, and the lock moves to the library

**Files:**
- Modify: `crates/core/src/apply.rs` (`apply_inner`'s writer, `Lock`, `lock_path`,
  `undo`, delete `undo_path`)
- Modify: `crates/core/tests/apply_refusals.rs:339,342,348`
- Test: `crates/core/tests/apply_record.rs`

**Interfaces:**
- Consumes: Task 1's `record::{dir_in, ensure_dir, basename, lock_name, hold}`,
  Task 2's `UndoWriter::create(.., preset, planned)` and `summarise`.
- Produces:
  ```rust
  pub fn apply(
      plan: &Path, records: &Path, allow_cloud_move: bool,
      progress: &mut dyn FnMut(Progress),
  ) -> Result<ApplyResult, ApplyError>;
  pub fn check(/* same, minus records */) -> Result<ApplyResult, ApplyError>;
  pub fn supersede(records: &Path, source_root: &Path, dest_root: &Path) -> std::io::Result<usize>;
  ```
  `Lock::acquire(records, dest_root)` replaces `Lock::acquire(plan)` and
  `lock_path(plan)`; `undo_path(plan)` is deleted. `supersede` removes records whose
  summary matches both roots and which `record::is_live` says nobody holds, and
  returns how many it removed.

- [ ] **Step 1: Write the failing tests**

```rust
/// Review Focus 4. A run that cannot be recorded does not happen: a run nobody
/// can reverse is the case a user would want to be asked about.
#[test]
fn apply_refuses_when_the_records_directory_cannot_be_written() {
    let f = fixture();
    let blocked = f.dir.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let guard = assert_unchanged(&f.source_root, &f.dest_root);
    let err = apply(&f.plan, &blocked, false, &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("record"), "{err}");
    guard.assert();
}

/// Review Focus 2. Superseding must not touch a record whose run is going.
#[test]
fn supersede_leaves_a_live_record_alone() {
    let f = two_records_for_one_root_pair();
    let held = comic_renamer_core::record::hold(&f.older).unwrap();
    assert_eq!(supersede(&f.records, &f.source_root, &f.dest_root).unwrap(), 0);
    assert!(f.older.exists(), "a live run's record is its only way back");
    drop(held);
    assert_eq!(supersede(&f.records, &f.source_root, &f.dest_root).unwrap(), 1);
}

#[test]
fn a_second_run_into_one_library_supersedes_the_first() {
    let f = fixture();
    apply(&f.plan, &f.records, false, &mut |_| {}).unwrap();
    let first = one_record_in(&f.records);
    apply(&f.plan_again, &f.records, false, &mut |_| {}).unwrap();
    assert!(!first.exists(), "the same root pair keeps only its newest record");
    assert_eq!(count_records(&f.records), 1);
}

#[test]
fn two_bundles_into_one_library_each_keep_their_record() {
    let f = two_bundles();
    apply(&f.plan_a, &f.records, false, &mut |_| {}).unwrap();
    apply(&f.plan_b, &f.records, false, &mut |_| {}).unwrap();
    assert_eq!(count_records(&f.records), 2, "disjoint bundles, both reversible");
}

/// The lock is per-library now, not per-plan: two plans into one library serialise.
#[test]
fn two_plans_into_one_library_cannot_run_at_once() {
    let f = two_bundles();
    let _held = Lock::acquire(&f.records, &f.dest_root).unwrap();
    assert!(matches!(Lock::acquire(&f.records, &f.dest_root), Err(ApplyError::Locked)));
}

/// A run that places nothing writes no record, so no offer is ever made for it.
#[test]
fn a_run_that_places_nothing_leaves_no_record() {
    let f = every_row_refused();
    apply(&f.plan, &f.records, false, &mut |_| {}).unwrap();
    assert_eq!(count_records(&f.records), 0);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test apply_record`
Expected: FAIL — `apply` takes three arguments, `Lock::acquire` takes one.

- [ ] **Step 3: Wire the store into `apply_inner`**

Keep creation lazy, on the first row that executes, for the reason at
`apply.rs:150` — but validate the records directory **before** the first row, so
Review Focus 4 refuses before anything moves rather than after. Call `supersede`
once, after the run completes, so a failed run does not destroy the previous
record. Take `record::hold` on the record for the run's lifetime.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p comic-renamer-core --test apply_record` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/apply.rs crates/core/tests/apply_record.rs crates/core/tests/apply_refusals.rs
git commit -m "feat(apply): the record is the tool's, and the lock is the library's"
```

---

## Task 5: The plan is read once and held

**Files:**
- Modify: `crates/core/src/apply.rs` (`apply`, `check`, `apply_inner`)
- Modify: `crates/cli/src/main.rs:363-420`
- Test: `crates/cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 4's `apply`/`check`.
- Produces:
  ```rust
  pub fn apply(
      plan: &PlanHeader, rows: &[PlanRow], records: &Path,
      allow_cloud_move: bool, progress: &mut dyn FnMut(Progress),
  ) -> Result<ApplyResult, ApplyError>;
  pub fn check(
      plan: &PlanHeader, rows: &[PlanRow],
      allow_cloud_move: bool, progress: &mut dyn FnMut(Progress),
  ) -> Result<ApplyResult, ApplyError>;
  ```
  Neither reads the plan file. `main.rs` reads it once with
  `plan::read_allowing_edits`, computes the disclosure from that copy, takes the
  confirmation, and passes the same copy to `apply`. The lock is taken by the
  caller around the whole sequence, via Task 4's `Lock::acquire(records, dest_root)`.

- [ ] **Step 1: Write the failing test**

```rust
/// The second finding. The plan is rewritten between the disclosure and the
/// confirmation; what runs is what the user was shown, not what is on disk.
#[test]
fn a_plan_rewritten_after_the_disclosure_does_not_run() {
    let f = fixture_with_two_plans();      // plan A: 1 row; plan B: a different row
    let mut child = spawn_apply_expecting_confirmation(&f.plan_path);
    wait_for_disclosure(&mut child);
    std::fs::copy(&f.plan_b, &f.plan_path).unwrap();   // swap under the prompt
    confirm(&mut child, "MOVE");
    let out = child.wait_with_output().unwrap();
    assert!(f.a_destination.exists(), "plan A is what was consented to");
    assert!(!f.b_destination.exists(), "plan B never ran: {}", String::from_utf8_lossy(&out.stdout));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p comic-renamer --test cli a_plan_rewritten`
Expected: FAIL — plan B's destination exists, which is the finding.

- [ ] **Step 3: Move the read to the caller**

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p comic-renamer --test cli a_plan_rewritten` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/apply.rs crates/cli/src/main.rs crates/cli/tests/cli.rs
git commit -m "fix(apply): execute the plan the user was shown, not the file"
```

---

## Task 6: `undo` names a run

**Files:**
- Modify: `crates/cli/src/main.rs` (the `Undo` variant and its handler)
- Test: `crates/cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1's `record::is_live`, Task 2's `summarise`, Task 3's
  `reverse_with(.., Trust::FromDisk)`.
- Produces: the `Undo { run: Option<String>, list: bool, yes: bool }` variant,
  replacing `Undo { plan: PathBuf }`. With neither argument it reverses the most
  recent record no live process holds; `--list` prints one line per record and
  reverses nothing; a run id selects one. A run id is the record's basename.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn undo_list_names_each_run_without_reversing_anything() {
    let f = two_applied_runs();
    let out = run(&["undo", "--list"], &f);
    assert!(out.stdout.contains(&f.run_a_id), "{}", out.stdout);
    assert!(out.stdout.contains(&f.run_b_id), "{}", out.stdout);
    assert!(f.a_destination.exists(), "--list reverses nothing");
}

#[test]
fn undo_with_no_argument_reverses_the_most_recent_run() {
    let f = two_applied_runs();
    run(&["undo", "--yes"], &f);
    assert!(!f.b_destination.exists(), "the newest went back");
    assert!(f.a_destination.exists(), "the older did not");
}

#[test]
fn undo_takes_a_run_id_and_never_a_plan_path() {
    let f = two_applied_runs();
    run(&["undo", "--yes", &f.run_a_id], &f);
    assert!(!f.a_destination.exists());
    let out = run(&["undo", "--yes", f.plan.to_str().unwrap()], &f);
    assert!(!out.status.success(), "a plan path is not a run id: {}", out.stderr);
}

#[test]
fn undo_does_not_print_the_records_path() {
    let f = two_applied_runs();
    let out = run(&["undo", "--list"], &f);
    assert!(!out.stdout.contains("/runs/"), "availability, not a path: {}", out.stdout);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer --test cli undo_`
Expected: FAIL — `--list` is not a recognised argument.

- [ ] **Step 3: Reshape the variant and its handler**

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p comic-renamer --test cli undo_` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs crates/cli/tests/cli.rs
git commit -m "feat(cli): undo names a run, not a plan"
```

---

## Task 7: The startup menu

**Files:**
- Create: `crates/cli/src/menu.rs`
- Modify: `crates/cli/src/main.rs` (`Cli::command` to `Option<Command>`, dispatch)
- Test: `crates/cli/src/menu.rs` (`mod tests`), `crates/cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1's `record::is_live`, Task 2's `summarise` (for `interrupted`,
  `planned`, `placed`), Task 3's `reverse_with`.
- Produces:
  ```rust
  pub enum Waiting { Interrupted(Summary), Completed(Summary) }
  pub fn waiting(records: &Path) -> std::io::Result<Vec<(PathBuf, Waiting)>>;
  pub fn render(waiting: &[(PathBuf, Waiting)], session_run: bool) -> String;
  pub fn run(records: &Path) -> Result<(), String>;
  ```
  `waiting` skips live records and empty ones, and sorts interrupted first.
  `render` is the screen as text, so the wording is testable without a terminal.
  `run` refuses without a TTY exactly as `map` does.

- [ ] **Step 1: Write the failing tests**

```rust
/// The first draft had one offer worded "a run was interrupted", which
/// described a clean `apply --yes` from yesterday as a crash.
#[test]
fn the_two_offers_are_worded_differently() {
    let screen = render(&[(p("a"), Waiting::Interrupted(summary(312, 400))),
                          (p("b"), Waiting::Completed(summary(400, 400)))], false);
    let interrupted = screen.lines().position(|l| l.contains("interrupted")).expect(&screen);
    let completed = screen.lines().position(|l| l.contains("placed 400 files")).expect(&screen);
    assert!(interrupted < completed, "the half-migrated library comes first:\n{screen}");
    assert!(!screen.contains("interrupted on") || completed > interrupted);
    assert!(screen.contains("312 of 400"), "a partial run says how far it got:\n{screen}");
}

#[test]
fn a_live_or_empty_record_is_not_offered() {
    let f = records_with(&[Kind::Live, Kind::Empty, Kind::Completed]);
    let w = waiting(&f.records).unwrap();
    assert_eq!(w.len(), 1, "{w:?}");
}

#[test]
fn item_four_appears_only_with_a_run_from_this_session() {
    assert!(!render(&[], false).contains("reverse this session"));
    assert!(render(&[], true).contains("reverse this session"));
}

#[test]
fn the_menu_never_prints_a_record_path() {
    let screen = render(&[(p("/Users/x/Library/Application Support/comic-renamer/runs/r"),
                           Waiting::Completed(summary(400, 400)))], true);
    assert!(!screen.contains("Application Support"), "{screen}");
}

#[test]
fn the_menu_refuses_without_a_terminal() {
    let out = run_binary_with_no_args_and_no_tty();
    assert!(!out.status.success());
    assert!(out.stderr.contains("terminal"), "{}", out.stderr);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer menu`
Expected: FAIL, unresolved module `menu`.

- [ ] **Step 3: Implement the menu**

Plain prompts rather than `ratatui`: the screen is five lines plus the offers and
`render` already makes it a string. `Cli::command` becomes `Option<Command>` and
`None` dispatches to `menu::run`.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p comic-renamer menu` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/menu.rs crates/cli/src/main.rs crates/cli/tests/cli.rs
git commit -m "feat(cli): a startup menu, and two offers that are not the same offer"
```

---

## Task 8: Quitting the menu is declining

**Files:**
- Modify: `crates/cli/src/menu.rs`
- Test: `crates/cli/src/menu.rs` (`mod tests`)

**Interfaces:**
- Consumes: Task 7's `menu::run`, Task 1's `record::is_live`.
- Produces: `pub fn decline(records: &Path, offered: &[PathBuf]) -> std::io::Result<usize>`,
  called on the quit path as an explicit step. It deletes the records it was
  **offered and the user declined with `k`**, plus this session's own run, and never
  the interrupted ones. It is not a `Drop` impl.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn quitting_deletes_the_completed_record_it_offered() {
    let f = records_with(&[Kind::Completed]);
    assert_eq!(decline(&f.records, &[f.completed.clone()]).unwrap(), 1);
    assert!(!f.completed.exists());
}

/// A half-migrated library stays reversible until someone fixes it, so `l` is
/// not a decline.
#[test]
fn quitting_keeps_an_interrupted_record() {
    let f = records_with(&[Kind::Interrupted]);
    assert_eq!(decline(&f.records, &[f.interrupted.clone()]).unwrap(), 0);
    assert!(f.interrupted.exists());
}

/// A panic must not be a decline: the record has to survive exactly then.
#[test]
fn declining_is_not_a_destructor() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/menu.rs")).unwrap();
    assert!(!src.contains("impl Drop"), "the quit-path delete must be an explicit step");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer menu::tests::quitting`
Expected: FAIL — `decline` not found.

- [ ] **Step 3: Implement `decline` and call it on the quit path**

- [ ] **Step 4: Run them to verify they pass, and the whole suite with both gates**

Run: `cargo test --workspace`, then `cargo xtask check-mappings`, then `cargo xtask measure`
Expected: 0 failures, 0 warnings; `check-mappings` passes; `located` 91.6%.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/menu.rs
git commit -m "feat(menu): quitting is declining the undo you were shown"
```

---

## Task 9: Write down what changed

**Files:**
- Modify: `CLAUDE.md` (invariants, State, Known gaps)
- Modify: `docs/specs/2026-10-19-comic-renamer-design.md` (§13: D54–D57, amend D38)
- Modify: `README.md` (the menu, `undo`'s new shape)
- Modify: `docs/specs/2026-10-24-session-scoped-undo-and-startup-menu.md` (status)

**Interfaces:**
- Consumes: every earlier task.
- Produces: nothing code depends on.

- [ ] **Step 1: Add invariant 11 to `CLAUDE.md`, verbatim from the design's §2**

- [ ] **Step 2: Fold D54–D57 into §13 and amend D38**

D38's entry keeps its reasoning and gains a line saying its premise is void
in-session and that its lock consequence moved to the library rather than being
dropped. §6.13's amendment note is updated the same way.

- [ ] **Step 3: Update `CLAUDE.md`'s State and Known gaps**

State: the menu is the no-argument entry point, `undo` names a run. Known gaps:
re-check the `ApplyResult.failed` entry, and record the measured test count.

- [ ] **Step 4: Verify every claim you just wrote**

Run the commands the docs cite and paste real numbers. Do not write a count you
did not run.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md docs/specs
git commit -m "docs: invariant 11, D54-D57, and the menu"
```

---

## Self-review

**1. Spec coverage.** §2's D54 → Task 3; D55 → Task 8; D56 → Task 6; D57 → Task 4.
§3's location and naming → Task 1; schema → Task 2; lock → Task 4; liveness →
Tasks 1 and 7; lazy creation → Task 4; `Trust` → Task 3. §4 → Task 5. §5 → Task 7.
§6's breakage → Task 9, except the roots cross-check deletion, which is Task 2's
step 3. §7's four builder choices are settled in Tasks 1, 3 and 7 (`sha256_of`,
plain prompts, `render` as a string) — the fourth, sweeping pre-existing records,
is **deliberately not planned**: no release has shipped, so no such record exists.

**2. Step scan.** No step says "handle errors" or "add validation". Each code step
carries a signature; the only code blocks are tests and the two `COLUMNS`/interface
declarations the implementer cannot guess.

**3. Type consistency.** `Summary` is defined once in Task 2 and consumed by Tasks 4
and 7. Task 3 adds `reverse_with(manifest, trust)` and leaves `reverse(manifest)` as
`reverse_with(manifest, Trust::FromDisk)`, so Task 2's tests keep compiling
unchanged; `UndoWriter::create` gains two parameters in Task 2 and its only
non-test caller is rewritten in Task 4, which is why Task 2's step 4 says not to
touch `apply.rs`.

**4. Review Focus.** All five have an owning task and a named test: 1 and 3 in
Task 2, 2 and 4 in Task 4, 5 in Task 1.

**5. Proportion.** The design is 335 lines; this plan is about 500, most of it test
assertions. No function body appears.
