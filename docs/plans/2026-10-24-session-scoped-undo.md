# Session-scoped undo and a startup menu — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the record of what a run did from a file the user hands in to a file
the tool owns, take away its authority to authorise a deletion, and reach it from a
startup menu and a run-keyed `undo`.

**Architecture:** `core::record` owns everything about records as a *set* — naming,
enumeration, liveness, retention and the per-library lock — and takes the directory
as an argument so core never reaches for `config_dir()`. `core::undo` keeps the CSV
format and grows the fields a preset needs, so `reverse` recomputes destinations
itself. Every path that unlinks a library file routes through one guarded helper
that checks the filesystem rather than the record. `apply` and `check` take a plan
the caller has already read and a lock the caller already holds.

**Tech Stack:** Rust 1.88, `libc` (`flock`), `corpus::sha256_hex` and
`fsops::hash_open_file` for hashing, `fsops::ensure_root`, `normalize::collision_key`.

**Spec:** `docs/specs/2026-10-24-session-scoped-undo-and-startup-menu.md`

**Revision:** second draft, 2026-10-24. The first failed a three-seat review:
four of nine tasks could not compile at their own commit, the delete gate covered
one of the two arms that need it, and `supersede` could destroy the only record of
a 400-file run. The review notes below each task say what changed.

## Global Constraints

- `rust-version` is **1.88**; do not raise it. macOS only.
- **Invariant 1:** presets are the only code that constructs paths.
- **Invariant 2:** `apply` never trusts a path string in a plan.
- **Invariant 6:** no-clobber at the syscall, and **never check-then-act on a
  destructive operation.** This applies to `supersede` as much as to a rename.
- **Invariant 7:** collision comparison is NFC-normalised and case-folded via
  `normalize::collision_key`, including the lock name derived from `dest_root`.
- **New invariant 11:** a record never authorises its own deletion.
- **`crates/core/tests/apply_exec.rs` is 898 lines and calls every signature this
  plan changes** — `apply` 19 times, `undo` 14, `reverse` 14, `UndoWriter::create`
  7, `undo_path` via `Fixture::manifest()` at line 68, and `Lock::acquire` at 891.
  It is named in the Files list of every task that changes one of those. A task
  whose Files list omits it cannot end green.
- Do not run `cargo fmt`; `rustfmt` is not installed. Do not reformat untouched files.
- `cargo test --workspace` must end 0 failures, **0 compiler warnings**. Baseline:
  409 passed, 7 ignored.
- `cargo xtask check-mappings` passes and `cargo xtask measure` reports `located` 91.6%.
- **`timeout` is not installed.** `timeout N cargo test` prints nothing and looks
  exactly like a hang. The suite runs in ~3 seconds.
- Never run `git push`, `gh pr create`, or any remote-touching command.

## Design change this plan makes

**`Trust::InSession` is cut; verification is unconditional.** The design's §3 says
the content check is skipped for a reverse within the session that wrote the record,
to keep "I just ran this, undo it" fast. Two seats independently rejected it: the
enum has one call site no test exercises, and a single miswired `InSession` reopens
the finding with a green suite. An exemption to invariant 11 that nothing proves is
worth more than the file reads it saves. Cost: an in-session undo of 300 rows reads
600 files. Update §3 in Task 10.

## Review Focus

1. **A record whose roots no longer exist** — every row refuses with a reason naming
   the root; no panic, no partial reverse reported clean. (Task 3)
2. **A live record must survive superseding** — or a crash in run B destroys run A's
   only way back. (Task 2)
3. **A record from an older build** — a header that is *present and wrong* refuses by
   name. A *headerless* record keeps its first row, which is what
   `a_manifest_with_no_header_keeps_its_first_row` exists to pin. (Task 3)
4. **The records directory cannot be written** — `apply` refuses before placing
   anything, via a new `ApplyError` variant. (Task 5)
5. **Roots that canonicalise equal** — `source_name == dest_relative` would make the
   content gate hash one inode against itself and agree. `RootsOverlap` must still
   refuse. (Task 4)
6. **A non-regular file at the recorded source** — a FIFO must not block `open`
   forever while the library lock is held. (Task 4)

---

## Task 1: Record naming, liveness, and the per-library lock

**Files:**
- Create: `crates/core/src/record.rs`
- Modify: `crates/core/src/lib.rs`, `crates/core/src/apply.rs` (extract `flock_nb`)
- Test: `crates/core/tests/record.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn basename(now: std::time::SystemTime) -> String;
  pub fn lock_name(dest_root: &Path) -> Result<String, std::io::Error>;
  pub fn is_live(path: &Path) -> std::io::Result<bool>;
  pub fn hold(path: &Path) -> std::io::Result<std::fs::File>;
  ```
  `basename` is `.comic-renamer.<unix-secs>.<pid>.<seq>.record`, written fresh —
  `fsops::temp_basename` yields a `.partial` suffix and cannot be reused directly,
  but its `static AtomicU64` counter shape is the thing to copy.
  `lock_name` canonicalises `dest_root`, hashes
  `normalize::collision_key(&canonical.to_string_lossy())` with
  `corpus::sha256_hex`, and returns `<hex>.lock`. It returns `Result` so a
  canonicalisation failure cannot silently fall back to two locks for one library.
  `hold` and `apply::Lock::acquire` both call one new
  `fsops::flock_nb(&File) -> io::Result<bool>`; the `unsafe` block exists once.

*Review changes:* `ensure_dir` cut — `fsops::ensure_root` (`fsops.rs:291`) is the
same function. `dir_in` cut — inline `config_dir()?.join("runs")` at the one CLI
call site. `lock_name` returns `Result`, was `String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_basename_is_unique_per_call_and_carries_the_stamp() {
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_000_000);
    let (a, b) = (basename(now), basename(now));
    assert_ne!(a, b);
    assert!(a.contains("1793000000") && a.starts_with(".comic-renamer.") && a.ends_with(".record"), "{a}");
}

/// Review Focus: a root is a path, not a filename, so it is hashed.
#[test]
fn a_lock_name_is_one_filename_whatever_the_root_looks_like() {
    let dir = tempfile::tempdir().unwrap();
    for leaf in ["My Comics", "Caf\u{e9}", &"x".repeat(200)] {
        let root = dir.path().join(leaf);
        std::fs::create_dir(&root).unwrap();
        let name = lock_name(&root).unwrap();
        assert!(name.len() <= 255 && name.ends_with(".lock"), "{name}");
        assert_eq!(Path::new(&name).components().count(), 1, "{name}");
    }
}

/// Invariant 7 reaches the lock. Both paths must exist, or the test passes
/// through `lock_name`'s error arm and proves nothing.
#[test]
fn one_library_is_one_lock_whatever_its_case_or_composition() {
    let dir = tempfile::tempdir().unwrap();
    let nfc = dir.path().join("Caf\u{e9}");
    std::fs::create_dir(&nfc).unwrap();
    let nfd = dir.path().join("Cafe\u{301}");          // same directory on APFS
    assert_eq!(lock_name(&nfc).unwrap(), lock_name(&nfd).unwrap());
    let other = dir.path().join("Hellboy");
    std::fs::create_dir(&other).unwrap();
    assert_ne!(lock_name(&nfc).unwrap(), lock_name(&other).unwrap());
}

#[test]
fn a_canonicalisation_failure_is_an_error_not_a_second_lock() {
    assert!(lock_name(Path::new("/no/such/library")).is_err());
}

/// `flock` attaches to the open file description, so a second open in this
/// same process conflicts. An implementer who reaches for `fcntl`/`F_SETLK`
/// gets per-process semantics and this assertion silently inverts.
#[test]
fn a_held_record_is_live_and_a_released_one_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.record");
    std::fs::write(&path, "x").unwrap();
    assert!(!is_live(&path).unwrap());
    let held = hold(&path).unwrap();
    assert!(is_live(&path).unwrap());
    drop(held);
    assert!(!is_live(&path).unwrap());
}

/// Review Focus 6's sibling: a record that cannot be opened for write must not
/// make the whole sweep fail. `is_live` answers the question it was asked.
#[test]
fn a_read_only_record_is_not_live_rather_than_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro.record");
    std::fs::write(&path, "x").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms).unwrap();
    assert!(!is_live(&path).unwrap(), "nobody holds it, so it is not live");
}
```

- [ ] **Step 2: Run them; expect FAIL (unresolved module `record`)**

Run: `cargo test -p comic-renamer-core --test record`

- [ ] **Step 3: Implement the four functions and extract `fsops::flock_nb`**

`is_live` opens read-only, so a read-only record answers the question instead of
erroring; only a genuine `EWOULDBLOCK` means live.

- [ ] **Step 4: Run them, then the workspace**

Run: `cargo test -p comic-renamer-core --test record` then `cargo test --workspace`
Expected: PASS; workspace still 0 failures, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/record.rs crates/core/src/lib.rs crates/core/src/apply.rs crates/core/tests/record.rs
git commit -m "feat(record): how a record is named, whether it is live, and one lock per library"
```

---

## Task 2: Enumeration and retention

**Files:**
- Modify: `crates/core/src/record.rs`
- Test: `crates/core/tests/record.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces:
  ```rust
  pub fn list(dir: &Path) -> std::io::Result<Vec<PathBuf>>;          // newest first, by name
  pub fn supersede(
      dir: &Path, keep: &Path, source_root: &Path, dest_root: &Path,
  ) -> std::io::Result<usize>;
  ```
  `list` skips anything not matching `basename`'s shape, so a junk file in the
  directory cannot abort a sweep. `supersede` is called **while the caller holds
  the library lock**, deletes every record other than `keep` whose roots match both
  arguments and which `is_live` says nobody holds, and returns the count. Its
  caller passes the record it just created as `keep`, so a run that created none
  never calls it.

*Review changes:* split out of Task 4, because D57 is the one policy the design
says the owner may want different and it has to be rejectable on its own.
`supersede` gained `keep` and the lock obligation: the first draft called it "after
the run completes", which deleted record A when run 2 collided on every row and
wrote no record B — 400 files made permanently unreversible by running one command
twice. Holding the lock across the delete is invariant 6: `is_live()` then
`unlink()` is check-then-act aimed at the only record of where a user's files went.

- [ ] **Step 1: Write the failing tests**

```rust
/// Review Focus 2.
#[test]
fn supersede_leaves_a_live_record_alone() {
    let f = two_records_for_one_root_pair();
    let held = hold(&f.older).unwrap();
    assert_eq!(supersede(&f.dir, &f.newer, &f.source_root, &f.dest_root).unwrap(), 0);
    assert!(f.older.exists(), "a live run's record is its only way back");
    drop(held);
    assert_eq!(supersede(&f.dir, &f.newer, &f.source_root, &f.dest_root).unwrap(), 1);
}

/// The HIGH finding the first draft shipped: a second run that places nothing
/// must not take the first run's record with it.
#[test]
fn a_run_that_wrote_no_record_supersedes_nothing() {
    let f = two_records_for_one_root_pair();
    // `keep` is a record that was never created.
    let absent = f.dir.join("nothing");
    assert_eq!(supersede(&f.dir, &absent, &f.source_root, &f.dest_root).unwrap(), 2);
    // ...which is why `apply` must not call it at all in that case; see Task 5.
}

#[test]
fn supersede_keeps_a_different_root_pairs_record() {
    let f = two_bundles_one_library();
    assert_eq!(supersede(&f.dir, &f.b, &f.source_b, &f.dest_root).unwrap(), 0);
    assert!(f.a.exists(), "disjoint bundles, both reversible");
}

#[test]
fn list_is_newest_first_and_ignores_a_junk_file() {
    let f = two_records_for_one_root_pair();
    std::fs::write(f.dir.join("README"), "not a record").unwrap();
    let found = list(&f.dir).unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0], f.newer);
}
```

`two_records_for_one_root_pair()` and `two_bundles_one_library()` are helpers in the
test file: each creates a tempdir, real `source*/` and `library/` directories, and
writes records with `UndoWriter` (Task 3's arity) carrying those canonical roots.

- [ ] **Step 2: Run them; expect FAIL (`list` not found)**
- [ ] **Step 3: Implement `list` and `supersede`**
- [ ] **Step 4: Run them, then the workspace**
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(record): one record per root pair, and never the live one"
```

---

## Task 3: The record carries what a reverse needs — additively

**Files:**
- Modify: `crates/core/src/undo.rs`
- Test: `crates/core/tests/undo_schema.rs`

**Interfaces:**
- Consumes: nothing.
- Produces, **alongside today's `create` and `reverse`, which stay until Task 5**:
  ```rust
  pub const COLUMNS: &str = "seq,status,mode,mechanism,source_name,dest_relative,\
      size,inode,mtime_ns,series,volume,subtitle,run,extension,preset,planned";
  pub struct Summary {
      pub source_root: PathBuf, pub dest_root: PathBuf,
      pub preset: String, pub planned: u32,
      pub placed: u32, pub interrupted: bool,
      pub started: std::time::SystemTime,
  }
  pub fn summarise(manifest: &Path) -> Result<Summary, UndoError>;
  pub fn reverse_from(manifest: &Path) -> Result<UndoReport, UndoError>;
  impl UndoWriter {
      pub fn create_with(path: &Path, source_root: &Path, dest_root: &Path,
                         preset: &str, planned: u32) -> io::Result<UndoWriter>;
  }
  // record_pending gains: series: &str, volume: Option<u32>,
  //                       subtitle: Option<&str>, run: u32, extension: &str
  ```
  `placed` counts rows whose terminal state is `placed` or `done`. `interrupted` is
  true when any row has no terminal state. `started` comes from the roots row's
  `mtime`, so §5's screen can say when the run happened. `reverse_from` takes roots
  and preset id from the record, canonicalises both, **keeps the `RootsOverlap`
  refusal** (Review Focus 5), and recomputes each destination through
  `preset_by_id`.

*Review changes:* additive rather than in-place, which is the only shape where
"do not touch `apply.rs`" is an instruction that can be obeyed and end green.
`Summary` gained `started`. The header rule is resolved below. `record_failed`,
`record_placed` and `record_done` each write exactly 9 fields today
(`undo.rs:90,101,106`) and must be padded to 16, which the first draft missed.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_record_alone_rebuilds_every_destination() {
    let (dir, record) = a_written_record();
    let s = summarise(&record).unwrap();
    assert_eq!((s.preset.as_str(), s.planned, s.placed, s.interrupted), ("komga", 1, 1, false));
    let report = reverse_from(&record).unwrap();
    assert_eq!(report.refused, Vec::<String>::new());
    assert_eq!(report.restored, 1);
    drop(dir);
}

/// Review Focus 3, both halves. A header that is present and wrong is an older
/// build. A record with no header at all keeps its first row — the bug
/// `a_manifest_with_no_header_keeps_its_first_row` (apply_exec.rs:787) pins is
/// that `read_manifest` ate record 0 unconditionally, and that proof survives.
#[test]
fn a_wrong_header_is_refused_and_a_missing_one_is_not() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();

    let old = dir.path().join("old.record");
    std::fs::write(&old, "seq,status,mode,mechanism,source_name,dest_relative,size,inode,mtime_ns\n").unwrap();
    assert!(summarise(&old).unwrap_err().to_string().contains("older build"));

    let headerless = dir.path().join("bare.record");
    std::fs::write(&headerless, text.lines().skip(1).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    assert_eq!(summarise(&headerless).unwrap().planned, 1, "the roots row is not eaten as a header");
    drop(dir);
}

/// Review Focus 1.
#[test]
fn a_record_whose_roots_are_gone_refuses_every_row() {
    let (dir, record) = a_written_record();
    std::fs::remove_dir_all(dir.path().join("library")).unwrap();
    let report = reverse_from(&record).unwrap();
    assert_eq!(report.restored, 0);
    assert!(report.refused.iter().any(|r| r.contains("library")), "{:?}", report.refused);
    drop(dir);
}

/// Review Focus 5. Equal roots would make Task 4's gate hash one inode against
/// itself and agree, deleting the only copy.
#[test]
fn a_record_whose_roots_canonicalise_equal_is_refused() {
    let (dir, record) = a_record_with_one_root_for_both();
    assert!(matches!(reverse_from(&record), Err(UndoError::NoRoot) | Err(UndoError::Parse { .. })));
    drop(dir);
}

#[test]
fn every_status_row_has_the_full_column_count() {
    let (dir, record) = a_written_record();
    let text = std::fs::read_to_string(&record).unwrap();
    let want = COLUMNS.split(',').count();
    for line in text.lines().skip(1) {
        assert_eq!(line.split(',').count(), want, "{line}");
    }
    drop(dir);
}
```

`a_written_record()` makes a tempdir with `source/` and `library/`, one real file
whose name a preset can produce a destination for, drives `create_with` →
`record_pending` → `record_placed` → `record_done` for a Copy row with series
`Hellboy`, volume 1, run 3, extension `pdf`, and returns `(TempDir, PathBuf)` with
**canonicalised** roots. `a_record_with_one_root_for_both()` is the same with one
directory as both roots.

- [ ] **Step 2: Run them; expect FAIL (`summarise` not found)**
- [ ] **Step 3: Grow `COLUMNS`, add `create_with`, `summarise`, `reverse_from`**

Pad the three terminal-status writers to 16 fields. `read_manifest` refuses only a
header whose first field is `seq` and whose text is not `COLUMNS`.

- [ ] **Step 4: Run them, then the workspace. Nothing else changed, so it stays green.**
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(undo): a record that carries its own roots, preset and fields"
```

---

## Task 4: One gate on every unlink of a library file

**Files:**
- Modify: `crates/core/src/undo.rs` (`reverse_one`), `crates/core/src/fsops.rs`
- Test: `crates/core/tests/undo_authority.rs`

**Interfaces:**
- Consumes: Task 3's `reverse_from`.
- Produces:
  ```rust
  // fsops
  pub fn sha256_of(path: &Path) -> std::io::Result<String>;   // regular files only
  ```
  Inside `undo.rs`, one private helper gates both of the unlink sites that act on
  record-supplied source data — `undo.rs:317` (Copy) and `undo.rs:334` (Move, the
  duplicate a failed unlink left) — and removes the destination only when a regular
  file at `source_root/source_name` hashes equal to it. **`undo.rs:378` is not
  gated:** that path removes a duplicate the tool itself streamed back moments
  earlier and re-verifies identity through the handle, so the source is not
  record-claimed and hashing it would be cost without safety.
  `sha256_of` opens with `open_nofollow`, **refuses a non-regular file** so a FIFO
  at the recorded source cannot block forever while the library lock is held
  (Review Focus 6), and reuses `fsops::hash_open_file`.

*Review changes:* the first draft gated only the Copy arm. The Move arm's `placed`
branch reaches `entry.remove()` on identity alone, so a planted record with
`mode=move, mechanism=clone, status=placed`, a source file the planter made, and the
target's real identity deletes a library file and reports `restored: 1` — the
research note's reproduction through a different arm. `Trust` is cut (see "Design
change"). The FIFO refusal is new.

- [ ] **Step 1: Write the failing tests**

```rust
/// The executed reproduction from the research note, as a regression test.
#[test]
fn a_planted_record_does_not_delete_a_library_file() {
    for mode in [Mode::Copy, Mode::Move] {
        let f = planted(mode);
        let report = reverse_from(&f.record).unwrap();
        assert_eq!(report.restored, 0, "{mode:?}");
        assert!(!report.refused.is_empty(), "{mode:?}: {:?}", report.refused);
        assert!(f.library_file.exists(), "{mode:?}: the file no run placed is still there");
    }
}

#[test]
fn a_real_copy_is_reversed_because_the_source_still_matches() {
    let f = real_copy();
    assert_eq!(reverse_from(&f.record).unwrap().restored, 1);
    assert!(!f.library_file.exists() && f.source_file.exists());
}

#[test]
fn a_source_with_different_content_refuses_the_delete() {
    let f = real_copy();
    std::fs::write(&f.source_file, b"different bytes entirely").unwrap();
    assert_eq!(reverse_from(&f.record).unwrap().restored, 0);
    assert!(f.library_file.exists());
}

/// Same length, different bytes — the case an identity check passes and a
/// content check does not. `apply_exec.rs:880` already pins the same idea for
/// the old gate.
#[test]
fn a_same_size_stranger_at_the_source_refuses_the_delete() {
    let f = real_copy();
    let len = std::fs::metadata(&f.source_file).unwrap().len() as usize;
    std::fs::write(&f.source_file, vec![b'z'; len]).unwrap();
    assert_eq!(reverse_from(&f.record).unwrap().restored, 0);
    assert!(f.library_file.exists());
}

/// Review Focus 6.
#[test]
fn a_fifo_at_the_source_is_refused_rather_than_blocking() {
    let f = real_copy();
    std::fs::remove_file(&f.source_file).unwrap();
    mkfifo(&f.source_file);
    assert_eq!(reverse_from(&f.record).unwrap().restored, 0);
    assert!(f.library_file.exists());
}

/// Reversing a move by renaming back is untouched: RENAME_EXCL refuses to clobber.
#[test]
fn a_move_is_reversed_by_renaming_back_without_a_content_check() {
    let f = real_move();
    assert_eq!(reverse_from(&f.record).unwrap().restored, 1);
    assert!(f.source_file.exists());
}
```

`planted(mode)` builds the research note's fixture: a 5-byte source the planter
made, a 33-byte library file no run placed, and a record carrying the library
file's real identity. `real_copy()` and `real_move()` drive a genuine
`apply`-shaped operation so source and destination really correspond. `mkfifo`
is a two-line `libc::mkfifo` helper in the test file.

- [ ] **Step 2: Run them; expect FAIL — the planted Move case deletes the file**
- [ ] **Step 3: Implement `sha256_of` and the shared gate**
- [ ] **Step 4: Run them, then the workspace**
- [ ] **Step 5: Commit**

```bash
git commit -m "fix(undo): one gate on every unlink that trusts a record's source"
```

---

## Task 5: `apply` takes a held plan, a held lock, and the record store

**Files:**
- Modify: `crates/core/src/apply.rs`, `crates/core/src/types.rs`
- Modify: `crates/core/tests/apply_exec.rs`, `crates/core/tests/apply_refusals.rs`
- Modify: `crates/cli/src/main.rs`
- Test: `crates/core/tests/apply_record.rs`

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces:
  ```rust
  pub struct Lock { /* … */ }
  impl Lock { pub fn library(records: &Path, dest_root: &Path) -> Result<Lock, ApplyError>; }
  pub fn apply(
      lock: &Lock, header: &PlanHeader, rows: &[PlanRow], records: &Path,
      allow_cloud_move: bool, progress: &mut dyn FnMut(Progress),
  ) -> Result<ApplyResult, ApplyError>;
  pub fn check(
      header: &PlanHeader, rows: &[PlanRow],
      allow_cloud_move: bool, progress: &mut dyn FnMut(Progress),
  ) -> Result<ApplyResult, ApplyError>;
  pub fn reverse_run(lock: &Lock, record: &Path) -> Result<UndoReport, ApplyError>;
  // ApplyResult gains: pub record: Option<PathBuf>
  // ApplyError gains:  Records(String)
  ```
  `Lock::acquire(plan)`, `lock_path(plan)`, `undo_path(plan)` and `apply::undo` are
  all **deleted**. The lock lives in the caller so one process cannot deadlock
  against itself, and `apply` taking `&Lock` makes "locked" a type obligation
  rather than a convention. `apply` validates the records directory **before the
  first row**, returning `ApplyError::Records` (Review Focus 4); creates the record
  lazily on the first row that executes, for the reason at `apply.rs:150`; holds
  `record::hold` on it for the run; and calls `record::supersede(dir, created, …)`
  **only when a record was created**, under the same lock. `reverse_run` replaces
  `apply::undo` and **deletes** the record after a reverse with no refusals, where
  today it renames it to `<name>.reversed` (`apply.rs:380-391`) — §3's lifecycle
  table says deleted, and `O_EXCL`-per-plan is no longer the reason for the rename.
  `main.rs` reads the plan once, takes the lock, computes the disclosure from that
  copy, prompts, and passes the same copy to `apply`.

*Review changes:* merged the first draft's Tasks 4 and 5 — one decision about one
object, and splitting them produced a self-deadlock where Task 4 kept the internal
acquire and Task 5 added a caller-held one. `apply_exec.rs` is in the Files list.
`ApplyResult.record` is new, so Task 7 can know which record this session wrote.
Delete-after-reverse was in no task.

- [ ] **Step 1: Write the failing tests**

```rust
/// Review Focus 4. A run that cannot be recorded does not happen.
#[test]
fn apply_refuses_when_the_records_directory_cannot_be_written() {
    let f = fixture();
    let blocked = f.dir.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let guard = TreeGuard::new(&[&f.source_root, &f.dest_root]);
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let err = apply(&lock, &f.header, &f.rows, &blocked, false, &mut |_| {}).unwrap_err();
    assert!(matches!(err, ApplyError::Records(_)), "{err:?}");
    guard.assert();
}

/// The HIGH finding: running one command twice must not make the first run
/// unreversible.
#[test]
fn a_second_run_that_places_nothing_keeps_the_first_runs_record() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let first = apply(&lock, &f.header, &f.rows, &f.records, false, &mut |_| {}).unwrap();
    let record = first.record.clone().expect("a run that placed rows wrote a record");
    let again = apply(&lock, &f.header, &f.rows, &f.records, false, &mut |_| {}).unwrap();
    assert_eq!(again.record, None, "every row collided, so nothing was recorded");
    assert!(record.exists(), "and the first run is still reversible");
}

#[test]
fn a_second_real_run_supersedes_the_first() {
    let f = two_plans_one_root_pair();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let a = apply(&lock, &f.header_a, &f.rows_a, &f.records, false, &mut |_| {}).unwrap();
    let b = apply(&lock, &f.header_b, &f.rows_b, &f.records, false, &mut |_| {}).unwrap();
    assert!(!a.record.unwrap().exists());
    assert!(b.record.unwrap().exists());
}

/// The successor to `apply_and_undo_share_one_lock` (apply_exec.rs:884), which
/// pinned the per-plan lock this task deletes.
#[test]
fn apply_and_reverse_share_one_library_lock() {
    let f = fixture();
    let held = Lock::library(&f.records, &f.dest_root).unwrap();
    assert!(matches!(Lock::library(&f.records, &f.dest_root), Err(ApplyError::Locked)));
    drop(held);
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = apply(&lock, &f.header, &f.rows, &f.records, false, &mut |_| {}).unwrap();
    assert_eq!(reverse_run(&lock, &r.record.unwrap()).unwrap().restored, 1);
}

#[test]
fn a_reversed_record_is_deleted_not_renamed_aside() {
    let f = fixture();
    let lock = Lock::library(&f.records, &f.dest_root).unwrap();
    let r = apply(&lock, &f.header, &f.rows, &f.records, false, &mut |_| {}).unwrap();
    let record = r.record.unwrap();
    reverse_run(&lock, &record).unwrap();
    assert!(!record.exists());
    assert!(!record.with_extension("record.reversed").exists());
}

/// Finding 2. The plan is rewritten between the disclosure and the
/// confirmation; what runs is what the user was shown.
#[test]  // in crates/cli/tests/cli.rs
fn a_plan_rewritten_after_the_disclosure_does_not_run() { /* see Step 1 note */ }
```

The CLI test spawns the binary, waits for the disclosure on stdout, overwrites the
plan file with a different one, then sends `MOVE\n`. `crates/cli/tests/cli.rs`
already drives the binary with `std::process::Command`; this is the first test
there that must write to its stdin mid-run, so it needs `Stdio::piped()` on both.

- [ ] **Step 2: Run them; expect FAIL on arity, and the CLI test on plan B running**
- [ ] **Step 3: Implement, and sweep `apply_exec.rs` and `apply_refusals.rs`**

`Fixture::manifest()` (`apply_exec.rs:68`) returns `undo_path(&self.plan)` and must
become the record this run wrote. `Recorded::reverse` (`:163`) and the direct
`reverse` calls at `:271, 288, 620, 668, 865` move to `reverse_from`. The seven
`UndoWriter::create` calls move to `create_with`.
`reverse_refuses_a_manifest_whose_roots_disagree_with_the_plan` (`:284`) is deleted:
with the record supplying both sides it asserts `x == x`. Its concern lives on in
Task 3's `a_record_whose_roots_canonicalise_equal_is_refused`.

- [ ] **Step 4: Run the workspace and both gates**

Run: `cargo test --workspace`, `cargo xtask check-mappings`, `cargo xtask measure`

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(apply): a held plan, a library lock, and a record the tool owns"
```

---

## Task 6: `undo` names a run

**Files:**
- Modify: `crates/cli/src/main.rs`
- Test: `crates/cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1's `is_live`, Task 3's `summarise`, Task 5's `Lock::library` and
  `reverse_run`.
- Produces: `Undo { run: Option<String>, list: bool }`, replacing
  `Undo { plan: PathBuf }`. No `--yes`: `run_undo` has no prompt to skip, and
  adding one is not in the design. A run id is a record's **file name**, matched
  against `record::list`'s output — never `records.join(run)`, which would make any
  relative path a run id. With no argument, the most recent record `is_live` says
  nobody holds.

*Review changes:* `--yes` cut. The lookup is a match against enumerated names
rather than a join.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn undo_list_names_each_run_without_reversing_anything() { /* both ids present, both files intact */ }

#[test]
fn undo_with_no_argument_reverses_the_most_recent_run() { /* newest reversed, older not */ }

#[test]
fn a_run_id_is_a_name_and_never_a_path() {
    // A traversal, an absolute path, and a plan path are all rejected.
    for bad in ["../../etc/passwd", "/etc/passwd", "comic-renamer-plan.jsonl"] {
        let out = run(&["undo", bad], &f);
        assert!(!out.status.success(), "{bad}: {}", out.stderr);
    }
}

#[test]
fn undo_does_not_print_a_record_path() { /* no "/runs/" and no "Application Support" in stdout */ }
```

- [ ] **Step 2: Run them; expect FAIL (`--list` unrecognised)**
- [ ] **Step 3: Reshape the variant and its handler**

Also replace `main.rs:430-431`, which prints `undo manifest: <path>` and
`comic-renamer undo <plan>`. Both cite a deleted function and a deleted argument
shape; the replacement states availability — that `comic-renamer undo` will
reverse this run — and names no path.

- [ ] **Step 4: Run them, then the workspace**
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(cli): undo names a run, and apply stops printing a path"
```

---

## Task 7: The menu, its offers, and its keys

**Files:**
- Create: `crates/cli/src/menu.rs`
- Modify: `crates/cli/src/main.rs`, `crates/cli/src/map/mod.rs` (`require_terminal`
  to `pub(crate)`)
- Test: `crates/cli/src/menu.rs` (`mod tests`), `crates/cli/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 1, 3, 5, 6.
- Produces:
  ```rust
  pub fn waiting(records: &Path) -> std::io::Result<Vec<(PathBuf, Summary)>>;
  pub fn render(waiting: &[(PathBuf, Summary)], session_run: bool) -> String;
  pub enum Key { Reverse(PathBuf), Decline(PathBuf), Map, Scan, Apply, ReverseSession, Quit }
  pub fn dispatch(key: char, waiting: &[(PathBuf, Summary)]) -> Option<Key>;
  pub fn run(records: &Path) -> Result<(), String>;
  ```
  `waiting` skips live records, records with `placed == 0`, and anything `summarise`
  refuses; it sorts `interrupted` first. `render` is a pure function of the summary
  list so the wording is testable without a terminal, and a `Summary` whose
  `interrupted` is true renders "interrupted … (N of M rows placed)" while one that
  is false renders "placed M files on <date>". `dispatch` maps a keystroke to an
  intent without performing it, which is the seam that makes `r` and `u` testable;
  `map::run_with` is the precedent. `run` refuses without a terminal via
  `map::require_terminal(stdout_is_terminal, stdin_is_terminal)`.

*Review changes:* `Waiting` cut — `Summary.interrupted` already carries it.
`dispatch` and `Key` are new: the first draft produced `render` and `run` and no
task drove a key, so the `r`/`u` paths and menu items 1–3 were unimplemented.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_two_offers_are_worded_differently_and_interrupted_comes_first() { /* … */ }

#[test]
fn a_live_or_empty_record_is_not_offered() { /* Live, placed==0, and unparseable all filtered */ }

#[test]
fn item_four_appears_only_with_a_run_from_this_session() { /* render(&[], false) vs true */ }

#[test]
fn the_menu_never_prints_a_record_path() { /* no "Application Support" in render's output */ }

#[test]
fn r_reverses_the_interrupted_record_and_u_the_completed_one() {
    let w = vec![(p("a"), interrupted(312, 400)), (p("b"), completed(400))];
    assert!(matches!(dispatch('r', &w), Some(Key::Reverse(x)) if x == p("a")));
    assert!(matches!(dispatch('u', &w), Some(Key::Reverse(x)) if x == p("b")));
    assert!(matches!(dispatch('k', &w), Some(Key::Decline(x)) if x == p("b")));
    assert!(matches!(dispatch('l', &w), None), "leaving an interrupted run declines nothing");
}

#[test]  // crates/cli/tests/cli.rs
fn the_menu_refuses_without_a_terminal() { /* no args, piped stdio, non-zero, "terminal" in stderr */ }
```

- [ ] **Step 2: Run them; expect FAIL (unresolved module `menu`)**
- [ ] **Step 3: Implement the menu; `Cli::command` becomes `Option<Command>`**

Plain prompts, not `ratatui`: `render` already makes the screen a string. Items 1,
2 and 3 collect their own options and call the same entry points `Command::Map`,
`Command::Scan` and `Command::Apply` use, so there is one implementation per action.

- [ ] **Step 4: Run them, then the workspace**
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(cli): a startup menu, two offers that differ, and keys that are testable"
```

---

## Task 8: Quitting is declining

**Files:**
- Modify: `crates/cli/src/menu.rs`
- Test: `crates/cli/src/menu.rs` (`mod tests`)

**Interfaces:**
- Consumes: Task 7's `dispatch`, Task 5's `ApplyResult.record`.
- Produces: `pub fn decline(declined: &[PathBuf], session_run: Option<&Path>) -> std::io::Result<usize>`.
  An explicit step on the quit path, **never a `Drop` impl**: a panic unwinds, and
  that is exactly when the record must survive. It deletes only records the user
  declined with `k` plus this session's own run, which `run` learns from
  `ApplyResult.record`.

*Review changes:* takes the declined list rather than re-deriving it, so "a record
never offered is never deleted" is provable. The session's own record comes from
`ApplyResult.record`, which the first draft had no way to learn.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn quitting_deletes_a_declined_record_and_this_sessions_run() { /* both gone, count 2 */ }

#[test]
fn a_record_that_was_never_offered_is_never_deleted() {
    let f = three_records();
    assert_eq!(decline(&[f.a.clone()], None).unwrap(), 1);
    assert!(f.b.exists() && f.c.exists());
}

/// A half-migrated library stays reversible until someone fixes it.
#[test]
fn an_interrupted_record_is_not_declined_by_quitting() { /* `l` produced no Decline, so not in the list */ }

#[test]
fn declining_is_not_a_destructor() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/menu.rs")).unwrap();
    assert!(!src.contains("impl Drop"), "the quit-path delete must be an explicit step");
}
```

- [ ] **Step 2: Run them; expect FAIL (`decline` not found)**
- [ ] **Step 3: Implement `decline` and call it on the quit path, with one line saying so**
- [ ] **Step 4: Run the workspace and both gates**
- [ ] **Step 5: Commit**

```bash
git commit -m "feat(menu): quitting is declining the undo you were shown"
```

---

## Task 9: Write down what changed

**Files:**
- Modify: `CLAUDE.md`, `README.md`,
  `docs/specs/2026-10-19-comic-renamer-design.md`,
  `docs/specs/2026-10-21-undo-and-reverse-design.md`,
  `docs/specs/2026-10-24-session-scoped-undo-and-startup-menu.md`

- [ ] **Step 1: Add invariant 11 to `CLAUDE.md`, verbatim from the design's §2**
- [ ] **Step 2: Fold D54–D57 into §13 and amend D38**

D38 keeps its reasoning and gains a line: its premise is void once the record is
tool-written, and its lock consequence moved to the library rather than being
dropped.

- [ ] **Step 3: Update `docs/specs/2026-10-21-undo-and-reverse-design.md`**

`undo.rs`'s module header points at it, and this work reverses its anchor decision.
It was in no task of the first draft.

- [ ] **Step 4: Update the 2026-10-24 design's §3 to record that `Trust` was cut**

The design promises a fast in-session reverse; the plan removed the exemption.
Say which, and why, rather than leaving the document describing code that does not
exist.

- [ ] **Step 5: Update `CLAUDE.md`'s State and Known gaps, then verify every number**

Run the commands the docs cite and paste real output. Do not write a count you did
not run, and do not count call sites by grepping a qualified path — the census for
this very plan was wrong by six times for exactly that reason.

- [ ] **Step 6: Commit**

```bash
git commit -m "docs: invariant 11, D54-D57, and what the plan changed about the design"
```

---

## Self-review

**1. Spec coverage.** D54 → Task 4; D55 → Task 8; D56 → Task 6; D57 → Task 2.
§3's naming and liveness → Task 1; schema → Task 3; lock → Tasks 1 and 5; lazy
creation, records-dir refusal and delete-after-reverse → Task 5; `Trust` →
cut, recorded in Task 9. §4 → Task 5. §5's screen, offers, keys and items → Task 7;
the path-print removal → Task 6. §6's breakage → Tasks 5 and 9.
§7's four open questions: `sha256_of` (Task 4), plain prompts (Task 7),
partial-run rendering (Task 7's `render`), and sweeping pre-existing records —
**deliberately not planned**, since no release has shipped and no such record exists.

**2. Step scan.** No step says "handle errors" or "add validation". Code blocks are
tests and interface declarations only; no function body appears.

**3. Type consistency.** `Summary` is defined in Task 3 and consumed by Tasks 2, 5,
6 and 7. `create_with`/`reverse_from` are added in Task 3 and the old `create`/
`reverse` are deleted in Task 5, which is the only task whose Files list includes
`apply_exec.rs`'s 48 call sites — so every task before it compiles against both.
`ApplyResult.record` is produced in Task 5 and consumed in Task 8.

**4. Review Focus.** Six conditions, each with an owning task and a named test:
1 and 3 and 5 in Task 3, 2 in Task 2, 4 in Task 5, 6 in Task 4.

**5. Proportion.** The design is 345 lines; this plan is about 560, nearly all of it
test assertions and interface declarations.
