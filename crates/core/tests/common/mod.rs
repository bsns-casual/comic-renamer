//! Shared test fixtures. Lives here because each `tests/*.rs` is its own
//! binary and cannot `use` a sibling.
//!
//! Task 8 adds the sparse-file generators and `TreeGuard` alongside these.

#![allow(dead_code)] // each test binary uses a different subset

use comic_renamer_core::corpus::{Corpus, Manifest, CORPUS_SCHEMA_VERSION};
use std::path::Path;
use tempfile::TempDir;

/// Long enough that a slow machine does not fail a working refusal, short
/// enough that a broken one is a test failure rather than a dead CI job.
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// `reverse_from` on a worker, with a deadline.
///
/// Every caller plants something at a path a record names that must not be
/// opened and read: a FIFO, where a blocking `open` waits for a writer that
/// never comes, or a character device, where the read itself never ends —
/// measured at 52 s of CPU and still climbing with the refusal removed.
/// libtest has no per-test timeout and neither does this project's CI, so
/// without a deadline a regression is a job that dies at the runner's limit
/// rather than a test that fails in three seconds. The deadline is also the
/// *only* signal for `O_NONBLOCK` specifically: refused and refused-late give
/// the same `not a regular file` message.
///
/// The worker is leaked if it never returns, so a caller must keep its
/// temporary directory rather than removing a path a live thread still holds.
pub fn reverse_before(
    record: &Path,
    deadline: std::time::Duration,
) -> comic_renamer_core::types::UndoReport {
    let record = record.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(comic_renamer_core::undo::reverse_from(&record));
    });
    rx.recv_timeout(deadline)
        .expect(
            "reverse_from never returned: the open or the read of a non-regular path \
             was not refused, so either O_NONBLOCK or refuse_non_regular has regressed",
        )
        .unwrap()
}

/// `(name, name_key, publisher, language, year_began, run)`.
pub type Row<'a> = (&'a str, &'a str, &'a str, &'a str, u16, u32);

pub fn write_db(path: &Path, rows: &[Row<'_>]) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
         INSERT INTO meta VALUES ('schema_version', '1');
         CREATE TABLE series (
             name        TEXT    NOT NULL,
             name_key    TEXT    NOT NULL,
             publisher   TEXT    NOT NULL,
             language    TEXT    NOT NULL,
             year_began  INTEGER NOT NULL,
             issue_count INTEGER NOT NULL
         );",
    )
    .unwrap();
    for (name, name_key, publisher, language, year, run) in rows {
        conn.execute(
            "INSERT INTO series VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![name, name_key, publisher, language, year, run],
        )
        .unwrap();
    }
}

/// A manifest that agrees with the file at `path`.
pub fn manifest_for(path: &Path, series_count: u64) -> Manifest {
    Manifest {
        url: format!("file://{}", path.display()),
        sha256: comic_renamer_core::corpus::sha256_hex(&std::fs::read(path).unwrap()),
        size_bytes: std::fs::metadata(path).unwrap().len(),
        series_count,
        schema_version: CORPUS_SCHEMA_VERSION,
        licence: "CC BY-SA 4.0".into(),
        dump_date: "2026-09-15".into(),
        attribution: "Grand Comics Database (https://www.comics.org)".into(),
        upstream_manifest: "https://example.invalid/gcd-data.json".into(),
        licence_url: None,
    }
}

/// The `TempDir` comes back with the corpus: `Corpus` holds an open handle on
/// a file that has to outlive it, so a fixture cannot be a bare value.
pub fn corpus(rows: &[Row<'_>]) -> (TempDir, Corpus) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gcd.sqlite");
    write_db(&path, rows);
    let manifest = manifest_for(&path, rows.len() as u64);
    let corpus = Corpus::open(&path, &manifest).unwrap();
    (dir, corpus)
}

/// `(stem, series, volume, volume_count)`. The key is derived from the stem,
/// so a fixture is written the way a contributor would write it.
pub type MapRow<'a> = (&'a str, &'a str, Option<u32>, Option<u32>);

/// A mapping holding exactly `rows`, loaded through the real TSV path so the
/// fixture is validated like shipped data is.
pub fn mappings(rows: &[MapRow<'_>]) -> comic_renamer_core::mapping::Mappings {
    use comic_renamer_core::mapping::{key, to_tsv, MappingRow, Mappings};
    let rows: Vec<MappingRow> = rows
        .iter()
        .map(|(stem, series, volume, volume_count)| MappingRow {
            key: key(stem).unwrap(),
            series: (*series).into(),
            volume: *volume,
            subtitle: None,
            lang: "en".into(),
            volume_count: *volume_count,
            bundle: String::new(),
        })
        .collect();
    Mappings::load(&to_tsv(&rows), None).unwrap()
}

// ---------------------------------------------------------------------------
// Task 8: fixtures and the unchanged-tree harness.
//
// Tests use generated files, never anyone's library (D12).
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// `seek` then write: a large `st_size` with **non-zero** `st_blocks`. Drives
/// the streaming path.
///
/// Not interchangeable with `sparse_truncate_only` — §7 keeps both because
/// they measure different things, and one cannot do both jobs.
pub fn sparse_seek_write(dir: &Path, name: &str, size: u64) -> PathBuf {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    file.seek(SeekFrom::Start(size - 1)).unwrap();
    file.write_all(b"\0").unwrap();
    file.sync_all().unwrap();
    path
}

/// `truncate` only: a large `st_size` with `st_blocks == 0`. Exists to prove
/// the dataless predicate does *not* fire on it (invariant 8, D25).
pub fn sparse_truncate_only(dir: &Path, name: &str, size: u64) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(size).unwrap();
    file.sync_all().unwrap();
    path
}

/// Size zero is never an error (invariant 3, D21) — the whole fixture suite is
/// built from these.
pub fn zero_byte(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::File::create(&path).unwrap();
    path
}

pub fn blocks(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().blocks()
}

/// What a path is, closely enough that any change we care about shows up.
///
/// **The inode is in here deliberately.** An earlier version recorded only
/// `(len, mtime_ns, head-hash)`, and `clonefile` preserves mtime to the
/// nanosecond — so a clone-and-replace that swapped the file under a path was
/// invisible to every guarded test. In a project whose invariants are about
/// inodes, clones and hardlinks, identity is the field that matters most.
#[derive(Clone, Debug, PartialEq)]
pub enum Fingerprint {
    Dir,
    Symlink(PathBuf),
    File {
        len: u64,
        mtime_ns: i64,
        inode: u64,
        nlink: u64,
        /// Head and tail, so a rewrite past 4 KiB is not invisible either.
        edges: String,
    },
}

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_dir() {
        return Some(Fingerprint::Dir);
    }
    if meta.file_type().is_symlink() {
        return Some(Fingerprint::Symlink(
            std::fs::read_link(path).unwrap_or_default(),
        ));
    }
    Some(Fingerprint::File {
        len: meta.len(),
        mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
        inode: meta.ino(),
        nlink: meta.nlink(),
        edges: edge_hash(path, meta.len()),
    })
}

/// Hashes the first and last 4 KiB. Whole-file hashing is not an option: the
/// sparse fixtures are 5 GiB.
fn edge_hash(path: &Path, len: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    const WINDOW: u64 = 4096;
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let mut head = vec![0u8; WINDOW.min(len) as usize];
    if file.read_exact(&mut head).is_ok() {
        buffer.extend_from_slice(&head);
    }
    if len > WINDOW {
        let mut tail = vec![0u8; WINDOW.min(len - WINDOW) as usize];
        if file.seek(SeekFrom::Start(len - tail.len() as u64)).is_ok()
            && file.read_exact(&mut tail).is_ok()
        {
            buffer.extend_from_slice(&tail);
        }
    }
    comic_renamer_core::corpus::sha256_hex(&buffer)
}

fn snapshot(roots: &[&Path]) -> BTreeMap<PathBuf, Option<Fingerprint>> {
    let mut out = BTreeMap::new();
    for root in roots {
        walk(root, &mut out);
    }
    out
}

fn walk(path: &Path, out: &mut BTreeMap<PathBuf, Option<Fingerprint>>) {
    out.insert(path.to_path_buf(), fingerprint(path));
    // read_dir on a symlink would follow it and recurse outside the root.
    if !matches!(fingerprint(path), Some(Fingerprint::Dir)) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        walk(&entry.path(), out);
    }
}

/// Snapshots a tree and asserts on `Drop` that nothing changed.
///
/// **Unchanged is the default.** A safety review found every refusal test in
/// an earlier draft asserted only the `Err` variant, so rows 1-3 could execute
/// before row 4 refused and the suite stayed green. "The function returned
/// `Err`" and "no files moved" are not the same claim, and only this makes the
/// second one.
///
/// A test that means to change something says **what kind** of change. An
/// earlier `expect_changed` exempted a named path from every difference, so a
/// test that permitted a destination being *created* silently lost the ability
/// to notice it being *destroyed*.
pub struct TreeGuard {
    roots: Vec<PathBuf>,
    before: BTreeMap<PathBuf, Option<Fingerprint>>,
    created: Vec<PathBuf>,
    removed: Vec<PathBuf>,
    modified: Vec<PathBuf>,
}

impl TreeGuard {
    pub fn new(roots: &[&Path]) -> TreeGuard {
        TreeGuard {
            roots: roots.iter().map(|p| p.to_path_buf()).collect(),
            before: snapshot(roots),
            created: Vec::new(),
            removed: Vec::new(),
            modified: Vec::new(),
        }
    }

    /// These paths must be absent before and present after.
    pub fn expect_created(mut self, paths: &[&Path]) -> TreeGuard {
        self.created = paths.iter().map(|p| p.to_path_buf()).collect();
        self
    }

    /// These paths must be present before and absent after.
    pub fn expect_removed(mut self, paths: &[&Path]) -> TreeGuard {
        self.removed = paths.iter().map(|p| p.to_path_buf()).collect();
        self
    }

    /// These paths must be present before and after, and must differ.
    pub fn expect_modified(mut self, paths: &[&Path]) -> TreeGuard {
        self.modified = paths.iter().map(|p| p.to_path_buf()).collect();
        self
    }
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        // A panic inside Drop while already unwinding aborts the process and
        // hides the real failure. The cost is that this guard's claim holds
        // only for tests that otherwise pass.
        if std::thread::panicking() {
            return;
        }
        let roots: Vec<&Path> = self.roots.iter().map(PathBuf::as_path).collect();
        let after = snapshot(&roots);

        let mut wrong: Vec<String> = Vec::new();
        // Declared paths are included even when they appear in neither
        // snapshot, or a promised change that never happened passes silently.
        let mut paths: Vec<&PathBuf> = self
            .before
            .keys()
            .chain(after.keys())
            .chain(self.created.iter())
            .chain(self.removed.iter())
            .chain(self.modified.iter())
            .collect();
        paths.sort();
        paths.dedup();

        for path in paths {
            let was = self.before.get(path).and_then(Option::as_ref);
            let now = after.get(path).and_then(Option::as_ref);
            let shown = path.display();

            if self.created.contains(path) {
                match (was, now) {
                    (None, Some(_)) => {}
                    (Some(_), _) => wrong.push(format!("expected created but already there: {shown}")),
                    (None, None) => wrong.push(format!("expected created, never appeared: {shown}")),
                }
            } else if self.removed.contains(path) {
                match (was, now) {
                    (Some(_), None) => {}
                    (None, _) => wrong.push(format!("expected removed but was not there: {shown}")),
                    (Some(_), Some(_)) => wrong.push(format!("expected removed, still there: {shown}")),
                }
            } else if self.modified.contains(path) {
                match (was, now) {
                    (Some(a), Some(b)) if a != b => {}
                    (Some(_), Some(_)) => wrong.push(format!("expected modified, unchanged: {shown}")),
                    _ => wrong.push(format!("expected modified, but it appeared or vanished: {shown}")),
                }
            } else if was != now {
                wrong.push(match (was, now) {
                    (None, Some(_)) => format!("unexpected change, appeared: {shown}"),
                    (Some(_), None) => format!("unexpected change, vanished: {shown}"),
                    _ => format!("unexpected change, modified: {shown}"),
                });
            }
        }
        assert!(
            wrong.is_empty(),
            "unexpected change to the tree:\n  {}",
            wrong.join("\n  ")
        );
    }
}

/// A source tree of named, **zero-byte** files. Invariant 3: size zero is
/// never an error, and the whole fixture suite depends on it — `scan` stats
/// these but never opens them.
pub fn tree_from_names(dir: &Path, names: &[&str]) {
    for name in names {
        std::fs::File::create(dir.join(name)).unwrap();
    }
}
