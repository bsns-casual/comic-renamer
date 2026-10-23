# Comic Renamer Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A command-line tool that resolves messy comic filenames against a bundled corpus and moves them into a Komga-shaped library, with a reviewable plan and no silent data loss.

**Architecture:** A cargo workspace: a library crate holding all logic, a thin CLI, and an `xtask` crate that runs the measurement gate. **Task 1 builds a skeleton that compiles** — every shared type, error and signature, with `todo!()` bodies. Later tasks fill in bodies and read their interfaces from the code, not from this document.

**Tech Stack:** Rust, **edition 2021**, 1.80+. `rusqlite` (`bundled`), `serde`/`serde_json`, `clap` v4, `unicode-normalization`, `sha2`, `libc`, `ureq`, `indicatif`, `dialoguer`, `comfy-table`, `tempfile` (dev).

**Spec:** `docs/specs/2026-10-19-comic-renamer-design.md` — read it before Task 1. Its §13 log (D1–D37) records why each constraint exists.

> **Why the skeleton exists.** v1 of this plan specified interfaces in prose across sixteen tasks. Three reviews found seven compile-blockers, four forward dependencies and eight error types used but never defined. Prose interfaces drift and no amount of careful reading catches it. Task 1 makes `cargo check` the arbiter instead.

## Global Constraints

- **Rust, CLI only.** No GUI (D17, D18).
- **Presets are the only code that constructs paths.** Rule data and plan rows never carry a path or separator (§6.11).
- **`apply` recomputes destinations through the preset** and never trusts a stored path (D14).
- **Size zero is never an error** (D21).
- **Dataless detection reads `SF_DATALESS`** (`0x4000_0000` in `st_flags`), never `st_blocks == 0` (D25).
- **No-clobber is enforced at the syscall**, per mechanism (D24).
- **Collision comparison is NFC then case-fold**; emitted paths are always NFC (D2, D36).
- **The language filter never falls back** — no match means `review` (D33).
- **Volume numbers are zero-padded to a per-series width** from the corpus run length, floor 2 (D37).
- **The corpus binary is never committed** (D19).
- **Phase 1 ships the plan file only.** No interactive walk, no `--hardlink`, no `--resume`, no journal, no live metadata API, no `--strip-to-parent` (D27, D10).
- **Gate:** located ≥ 92.3%, ready ≥ 66%, silent false positives ≤ 0.48% (D26, D31).

## Review Focus

1. **Non-comic files in the source** — cover art, `readme.txt`, `.DS_Store`, and a *directory* named `Hellboy.pdf`. Skipped silently. → Task 10.
2. **A volume no candidate can satisfy** — `vol13` where every candidate is shorter. `review`, not a panic. → Task 5.
3. **The destination already holds the series folder under different case.** Refuse the row. → Task 8.
4. **A corpus whose hash mismatches or whose cache is truncated.** Refuse to run. → Task 3.
5. **A plan hand-edited into an invalid state.** Refuse the whole file, never the rows that still parse. → Task 7.

---

## File Structure

```
Cargo.toml                       workspace: crates/core, crates/cli, xtask
.cargo/config.toml               [alias] xtask = "run -p xtask --"
crates/core/src/lib.rs           module declarations and re-exports
crates/core/src/types.rs         every shared type and error enum
crates/core/src/normalize.rs     squash, NFC, key variants, collision key
crates/core/src/residue.rs       volume / format / edition tokenizer
crates/core/src/corpus.rs        manifest, fetch + verify, index
crates/core/src/overrides.rs     Layer B
crates/core/src/resolve.rs       the ladder
crates/core/src/preset/{mod,komga}.rs
crates/core/src/plan.rs          JSONL plan
crates/core/src/fsops.rs         mechanisms, no-clobber syscalls, collisions
crates/core/src/undo.rs          append-only undo manifest
crates/core/src/scan.rs
crates/core/src/apply.rs
crates/core/src/komga_api.rs
crates/core/tests/common/mod.rs  fixture generator + assert_unchanged harness
crates/cli/src/main.rs
xtask/src/lib.rs                 grading logic (testable)
xtask/src/main.rs                thin wrapper
```

---

### Task 1: A skeleton that compiles

**Files:** the whole tree above, every function `todo!()`.

This is the task that prevents v1's failure mode. Nothing here is implemented; everything is *declared*, and `cargo check` passes.

**`types.rs` holds everything shared**, because v1 scattered these and they drifted:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Mode { Move, Copy }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Mechanism { Rename, Clone, Stream }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Basis { Override, Unambiguous, VolumeFeasibility, ShortestRun }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReviewReason { Ambiguous(Vec<SeriesRecord>), UnsplitSubtitle(String),
                        NoLanguageMatch, VolumeInfeasible }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Status { Ready(Basis), Review(ReviewReason), Failed }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesRecord { pub name: String, pub publisher: String,
                          pub language: String, pub year_began: u16, pub run: u32 }

#[derive(Clone, Debug, PartialEq)]
pub struct Resolved { pub series: String, pub volume: Option<u32>,
                      pub subtitle: Option<String>, pub run: u32,
                      pub status: Status, pub record: Option<SeriesRecord> }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceStat { pub size: u64, pub mtime_ns: i64, pub inode: u64 }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Collision { pub key: String, pub row_ids: Vec<String>,
                       pub existing: Option<PathBuf> }
```

Errors: `CorpusError`, `OverrideError`, `PlanError`, `ScanError`, `ApplyError`, `UndoError`, `KomgaError` — one enum per module, `thiserror`, each carrying the variants the later tasks' tests name.

Four decisions v1 left implicit and three reviews each tripped over:

- **`Preset::destination` returns a path *relative to* `dest_root`.** `apply` joins. `plan::read` validates that the joined path stays beneath `dest_root`.
- **`SourceStat` is how `apply` detects a changed source.** `PlanRow` carries one.
- **`PlanHeader` carries `required_bytes`, `download_bytes` and `checksum`** — the pre-flight numbers the CLI prints and §6.9's tamper check.
- **`scan` and `apply` take `&mut dyn FnMut(Progress)`.** The CLI's progress bar and the interrupt test both need that seam.

Use **`libc`'s own `renamex_np`, `clonefile` and `RENAME_EXCL`** for Apple targets. Do not redeclare them; edition 2021 avoids the `unsafe extern` requirement that edition 2024 would impose.

- [ ] **Step 1: Create the workspace, all three manifests, and `.cargo/config.toml`**

All three members must have manifests *now* — Cargo hard-errors on a member whose manifest arrives later.

- [ ] **Step 2: Write `types.rs` and every module's declarations with `todo!()` bodies**

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --workspace --all-targets`
Expected: clean, zero errors. Warnings about unused code are fine.

- [ ] **Step 4: Commit** — `git commit -m "feat: workspace skeleton and shared types"`

---

### Task 2: Normalisation and residue

**Files:** `normalize.rs`, `residue.rs` (unit tests in-module). Both pure, no I/O.

`key_variants` adds punctuation expansions: `&`, `+`, `/` → `and`, plus `plus` and `slash` (D16, worth a measured 3.4 points). `collision_key` is NFC then case-fold and **preserves separators** — it is not `squash`. Residue vocabulary is in §6.4 and was derived from measured data; do not trim it.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn squash_strips_punctuation_and_case() {
    assert_eq!(squash("Nemesis: Rogues' Gallery"), "nemesisroguesgallery");
}
#[test] fn key_variants_expand_punctuation() {
    assert!(key_variants("Hack/Slash").contains("hackandslash"));
    let w = key_variants("The Wicked + The Divine");
    assert!(w.contains("thewickedandthedivine") && w.contains("thewickedplusthedivine"));
}
#[test] fn collision_key_folds_case_and_normalises_but_keeps_spaces() {
    assert_eq!(collision_key("Hellboy"), collision_key("hellboy"));
    assert_eq!(collision_key("Pok\u{e9}mon"), collision_key("Poke\u{301}mon"));
    assert_ne!(collision_key("Hellboy"), collision_key("Hell boy"));
}
#[test] fn eats_numeric_word_and_roman_volumes() {
    for (s, n) in [("volume1",1),("vol1",1),("bookone",1),("voliv",4)] {
        assert_eq!(eat(s).tokens, vec![Token::Volume(n)]);
    }
}
#[test] fn leaves_subtitle_as_leftover() {
    let r = eat("volume1seedofdestruction");
    assert_eq!((r.tokens, r.leftover.as_str()), (vec![Token::Volume(1)], "seedofdestruction"));
}
#[test] fn strips_distributor_cruft() {
    assert_eq!(strip_cruft("thewickedandthedivine_vol1_optimized_1420484117"),
               "thewickedandthedivine_vol1");
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p comic-renamer-core normalize residue` → FAIL (`todo!()` panics)
- [ ] **Step 3: Implement both modules**
- [ ] **Step 4: Run to verify they pass** — PASS
- [ ] **Step 5: Commit**

---

### Task 3: Corpus

**Files:** `corpus.rs`, `tests/corpus.rs`.

`Corpus::open` validates `series_count`, `size_bytes` and `schema_version` against the manifest — otherwise the truncation test passes by luck of where the file was cut. `ensure_corpus` verifies size and SHA-256 **before** moving into the cache, so a failed fetch leaves nothing behind.

**Where the gate gets its corpus.** `ensure_corpus` caches to `$XDG_CACHE_HOME/comic-renamer/` (`~/Library/Caches/comic-renamer` on macOS). `cargo xtask measure` calls it; CI restores that path with `actions/cache` keyed on `corpus/manifest.json`'s sha256, falling back to a fetch. No test may touch the network — tests build a small SQLite file locally.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn rejects_hash_mismatch_and_leaves_no_cache() {      // Review Focus 4
    let (dir, m) = manifest_with_wrong_hash();
    assert!(matches!(ensure_corpus(&m, dir.path()), Err(CorpusError::HashMismatch { .. })));
    assert!(!dir.path().join("gcd.sqlite").exists());
}
#[test] fn rejects_a_corpus_whose_row_count_disagrees() {     // Review Focus 4
    assert!(matches!(Corpus::open(&truncated_db(), &valid_manifest()),
                     Err(CorpusError::CountMismatch { .. })));
}
#[test] fn indexes_squash_name_key_and_punctuation_variants() {
    let c = tiny_corpus(&[("Hack/Slash", "hackslash", "Image", "en", 2004, 6)]);
    assert_eq!(c.lookup("hackslash").len(), 1);
    assert_eq!(c.lookup("hackandslash").len(), 1);
}
#[test] fn manifest_requires_sharealike() {
    assert!(load_manifest(&manifest_with_licence("MIT")).is_err());
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** `lookup` returns `&[SeriesRecord]`, so the index is `HashMap<String, Vec<SeriesRecord>>` and records are cloned per key variant — roughly 5 × 230k clones at startup. `// shortcut: clones per key variant; switch to Vec<SeriesRecord> + HashMap<String, Vec<u32>> if startup exceeds a second`
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 4: Layer B overrides

**Files:** `overrides.rs`, `data/overrides.toml`, `data/LICENSE`, `tests/overrides.rs`.

The only place a wrong *title* is correctable, and the floor under the unmatched tail — initialisms like `TWD_CompendiumOne` are unreachable by any corpus (§6.2.1). **Before the resolver, because the resolver consumes it.**

A bare filename key is insufficient — `vol1.pdf` recurs across publishers. Every entry carries a `bundle` field, matched by **`squash(bundle)` being a substring of `squash(source_folder_name)`**. Validation rejects everything in §6.11's forbid list; fields cap at 120 bytes; duplicate keys are an error.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn rejects_path_shaped_and_invisible_fields() {
    for bad in ["../etc","a/b",r"C:\x",r"\\srv\s","/abs","CON","CON.pdf","x.","x ",
                "=cmd()","Hell\u{200b}boy","\u{202e}yobleH"] {
        assert!(Overrides::load(&override_with_series(bad)).is_err(), "accepted {bad:?}");
    }
}
#[test] fn requires_a_bundle_discriminator() {
    assert!(Overrides::load(&override_without_bundle()).is_err());
}
#[test] fn bundle_matches_as_squashed_substring_of_the_folder() {
    let o = Overrides::load(&twd_override()).unwrap();   // bundle: "Image Comics in the '10s"
    assert!(o.lookup("TWD_CompendiumOne", "HumbleImageComicsInThe10s").is_some());
    assert!(o.lookup("TWD_CompendiumOne", "HumbleBoomBundle").is_none());
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement; seed `data/overrides.toml`** with the initialisms the measurement found unreachable (`TWD_*`, `SOA_*`, `GHA_*`). `data/LICENSE` is CC BY-SA 4.0 with GCD attribution (D3/D9).
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 5: The resolver ladder

**Files:** `resolve.rs`, `tests/resolve.rs`.

Longest-prefix over the combined index, with the **boundary guard**: a match counts only if the boundary coincides with a separator in the original stem or the residue begins with a recognised token. That guard is what keeps `witcheromnibus` off *Witch*, and it held across 2,714 files with zero false positives.

Then §6.3 in order: language as a **hard filter with no fallback**; publisher preference from the folder hint; volume feasibility (`run >= volume`) as a hard constraint; shortest feasible run last.

**Shortest-run fires only when a volume token is present.** Without it, two records remain two records and the row is `Review(Ambiguous)`. v1 omitted this and two of its own tests contradicted each other.

Ambiguity is measured over **records** — `(name, publisher, year_began, run)` — never over name strings (D28).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn boundary_guard_rejects_a_midword_prefix() {
    assert_eq!(resolve("witcheromnibus", &no_hint(), &witch_and_witcher(), &no_ov(), "en").series,
               "The Witcher Omnibus");
}
#[test] fn bare_title_with_three_records_is_ambiguous() {
    assert!(matches!(resolve("velvet", &no_hint(), &three_velvets(), &no_ov(), "en").status,
                     Status::Review(ReviewReason::Ambiguous(_))));
}
#[test] fn same_records_with_a_volume_resolve_by_shortest_run() {
    assert_eq!(resolve("velvet_vol1", &no_hint(), &three_velvets(), &no_ov(), "en").status,
               Status::Ready(Basis::ShortestRun));
}
#[test] fn volume_infeasible_goes_to_review() {               // Review Focus 2
    assert!(matches!(resolve("someseries_vol13", &no_hint(), &short_runs(), &no_ov(), "en").status,
                     Status::Review(ReviewReason::VolumeInfeasible)));
}
#[test] fn language_never_falls_back() {                      // D33
    let r = resolve("noragami_vol3", &no_hint(), &japanese_only(), &no_ov(), "en");
    assert!(matches!(r.status, Status::Review(ReviewReason::NoLanguageMatch)));
    assert!(r.record.is_none());
}
#[test] fn publisher_hint_breaks_a_tie() {
    let h = publisher_hint("HumbleDarkHorseAnniversaryBundle", &two_masks());
    assert_eq!(resolve("maskomnibusvolume1secondedition", &h, &two_masks(), &no_ov(), "en").series,
               "The Mask Omnibus");
}
#[test] fn an_override_wins_and_is_reachable() {
    let r = resolve("TWD_CompendiumOne", &hint_for("HumbleImageComicsInThe10s"),
                    &gcd_subset(), &twd_overrides(), "en");
    assert_eq!((r.series.as_str(), r.status), ("The Walking Dead", Status::Ready(Basis::Override)));
}
#[test] fn a_subtitle_is_carried_and_marked_review() {        // D15
    let r = resolve("hellboyomnibusvolume1_seedofdestruction", &no_hint(), &gcd_subset(), &no_ov(), "en");
    assert_eq!(r.subtitle.as_deref(), Some("seedofdestruction"));
    assert!(matches!(r.status, Status::Review(ReviewReason::UnsplitSubtitle(_))));
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** `resolve` takes the source folder name so overrides are reachable.
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 6: Komga preset

**Files:** `preset/mod.rs`, `preset/komga.rs`, `tests/preset_komga.rs`.

Flat `<Series>/<Book>.<ext>`, relative to `dest_root` — only the leaf folder is read, so nesting is pointless (§6.8.1). Volume precedes subtitle because numbering is positional after a natural sort. `pad` uses the digit count of `run`, floor 2 (D37). `sanitize` fixes titles that are legal as titles and hostile as paths — the corpus legitimately returns `Nemesis: Rogues' Gallery`. `preset_by_id(&str) -> Option<Box<dyn Preset>>` exists because `apply` resolves `PlanHeader.preset`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn pads_to_per_series_width() {
    assert_eq!((pad(1,9), pad(1,34), pad(34,34), pad(1,120)), ("01","01","34","001"));
}
#[test] fn emits_a_flat_relative_path() {
    assert_eq!(Komga.destination(&res("Hellboy Omnibus", Some(1), 5, Some("Seed of Destruction")), "pdf"),
               PathBuf::from("Hellboy Omnibus/Hellboy Omnibus Vol. 01 - Seed of Destruction.pdf"));
}
#[test] fn sanitizes_without_rejecting() {
    let s = Komga.destination(&res("Nemesis: Rogues' Gallery", None, 1, None), "pdf")
                 .to_string_lossy().into_owned();
    assert!(!s.contains(':') && s.contains('\''));
}
#[test] fn distinct_titles_never_converge() {
    let a = Komga.destination(&res("Hellboy: Seed", None, 1, None), "pdf");
    let b = Komga.destination(&res("Hellboy - Seed", None, 1, None), "pdf");
    assert_ne!(a, b, "sanitising two titles onto one path silently merges two series");
}
#[test] fn refuses_hostile_components() {
    for t in ["CON", "trailing.", "trailing ", &"x".repeat(300)] {
        let p = Komga.destination(&res(t, None, 1, None), "pdf");
        let c = p.components().next().unwrap().as_os_str().to_string_lossy().into_owned();
        assert!(c.len() <= 255 && !c.ends_with('.') && !c.ends_with(' ') && c != "CON");
    }
}
#[test] fn output_is_nfc() {
    let p = Komga.destination(&res("Poke\u{301}mon", None, 1, None), "pdf");
    assert_eq!(p.to_string_lossy(), nfc(&p.to_string_lossy()));
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** Truncation is at 255 **bytes**, not chars, and must not split a UTF-8 sequence.
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 7: Plan serialisation

**Files:** `plan.rs`, `tests/plan.rs`.

**JSON Lines**: header object, then one row per line. §6.9 rules out TSV — tabs occur in titles, Excel corrupts encoding and line endings, a leading `=` is formula injection. JSONL stays diffable and lets a human or an assistant edit one line without endangering the rest.

`read` validates the whole file and **never returns a partial row set**. `#[serde(deny_unknown_fields)]` so a typo is an error, not a silently ignored field.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn round_trips_every_field() {
    let (h, rows) = sample_plan();
    write(&h, &rows, &p()).unwrap();
    assert_eq!(read(&p()).unwrap(), (h, rows));
}
#[test] fn rejects_duplicate_ids_unknown_status_and_escaping_paths() {  // Review Focus 5
    assert!(matches!(read(&plan_with_duplicate_ids()), Err(PlanError::DuplicateRowId(_))));
    assert!(read(&plan_with_status("definitely-ready")).is_err());
    assert!(matches!(read(&plan_escaping_dest_root()), Err(PlanError::OutsideDestRoot(_))));
}
#[test] fn one_bad_row_rejects_the_whole_file() {                      // Review Focus 5
    assert!(read(&plan_with_one_bad_row_among_ten()).is_err());
}
#[test] fn checksum_detects_an_edited_row() {
    assert!(matches!(read(&plan_with_edited_row_and_stale_checksum()),
                     Err(PlanError::ChecksumMismatch { .. })));
}
```

The checksum covers the header and every row, so `apply` can tell what a human changed (§6.9). An edited plan is not rejected — it is *reported* as edited, and `apply` revalidates rather than trusting it.

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 8: Fixtures and the unchanged-tree harness

**Files:** `crates/core/tests/common/mod.rs`, `tests/harness.rs`.

Tests use generated files, never anyone's library (D12). It lives in `tests/common/mod.rs` because each `tests/*.rs` is its own binary and cannot `use` a sibling.

**Two sparse files, not interchangeable** (§7): `seek`-then-write reports a large `st_size` with non-zero `st_blocks` and drives the streaming path; `truncate`-only reports `st_blocks == 0` and exists to prove the dataless predicate does *not* fire on it.

**The harness is the task's real deliverable.** A safety review found that every refusal test in v1 asserted only the `Err` variant and none asserted the tree was unchanged — so rows 1–3 could execute before row 4 refused and every test still passed. `TreeGuard::new(paths)` snapshots; **`Drop` asserts unchanged** unless the test called `.expect_changed(&[...])` first. Unchanged is the default; a test must opt in to permitting a change and say which paths.

That inversion is also the only thing that kills the theater class: an opaque `bool` helper cannot satisfy a tree-snapshot assertion.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn sparse_variants_differ_in_blocks() {
    let d = tempdir().unwrap();
    assert!(blocks(&sparse_seek_write(d.path(), "a.bin", 5<<30)) > 0);
    assert_eq!(blocks(&sparse_truncate_only(d.path(), "b.bin", 5<<30)), 0);
}
#[test] #[should_panic(expected = "unexpected change")]
fn guard_fails_when_a_file_appears() {
    let d = tempdir().unwrap();
    let _g = TreeGuard::new(&[d.path()]);
    std::fs::write(d.path().join("surprise"), b"x").unwrap();
}
#[test] fn guard_permits_a_declared_change() {
    let d = tempdir().unwrap();
    let g = TreeGuard::new(&[d.path()]).expect_changed(&[&d.path().join("ok")]);
    std::fs::write(d.path().join("ok"), b"x").unwrap();
    drop(g);
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** The snapshot is path → `(len, mtime_ns, sha256-of-first-4KiB)`, recursive, so a same-size rewrite is caught.
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 9: Filesystem operations and collisions

**Files:** `fsops.rs`, `tests/fsops.rs`. **Every test wraps both trees in a `TreeGuard`.**

This is where a mistake destroys a library. `rename(2)` **silently overwrites**; no-clobber comes from the syscall (D24): `renamex_np(RENAME_EXCL)`, `clonefile`'s native `EEXIST`, `O_EXCL` for streams. `is_dataless` reads `SF_DATALESS` (D25). `find_collisions` takes `&dyn Preset` and **recomputes** every destination — comparing `destination_preview` would let a hand-edited plan defeat detection.

`clone_excl` passes flags `0`. `// shortcut: containment via realpath-per-component; CLONE_RESOLVE_BENEATH would move it into the kernel but libc exports no constant`

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn rename_excl_refuses_and_leaves_both_files() {
    let (_g, src, dst) = two_files_distinct_contents();
    assert_eq!(rename_excl(&src, &dst).unwrap_err().kind(), ErrorKind::AlreadyExists);
    assert_eq!(read(&dst), "ORIGINAL");
    assert!(src.exists());
}
#[test] fn plain_rename_would_have_clobbered() {
    // pins WHY rename_excl exists; if this fails the platform changed
    let (_d, src, dst) = two_files_distinct_contents_unguarded();
    std::fs::rename(&src, &dst).unwrap();
    assert_eq!(read(&dst), "SOURCE");
}
#[test] fn clone_excl_refuses_and_is_a_distinct_inode() {
    let (src, dst) = one_file_and_a_free_path();
    clone_excl(&src, &dst).unwrap();
    assert_ne!(inode(&src), inode(&dst));
    assert_eq!(clone_excl(&src, &dst).unwrap_err().kind(), ErrorKind::AlreadyExists);
}
#[test] fn dataless_ignores_sparse_and_empty_files() {            // D25, D21
    assert!(!is_dataless(&sparse_truncate_only(d(), "t.bin", 5<<30)).unwrap());
    assert!(!is_dataless(&zero_byte("m.pdf")).unwrap());
}
#[test] fn detects_case_nfc_quote_and_filename_collisions() {     // D2, §6.10
    assert_eq!(collisions(&["Hellboy/x.pdf","hellboy/x.pdf"]).len(), 1);
    assert_eq!(collisions(&["Pok\u{e9}mon/a.pdf","Poke\u{301}mon/a.pdf"]).len(), 1);
    assert_eq!(collisions(&["Bourdain's/a.pdf","Bourdain\u{2019}s/a.pdf"]).len(), 1);
    assert_eq!(collisions(&["s_vol1.pdf","s_volume1.pdf"]).len(), 1, "one filename, two rows");
}
#[test] fn collides_with_an_existing_destination_folder_and_file() {  // Review Focus 3
    assert_eq!(find_collisions(&rows_for(&["hellboy/x.pdf"]), dest_with(&["Hellboy/"]),
                               &Komga).len(), 1);
    assert_eq!(find_collisions(&rows_for(&["Hellboy/x.pdf"]), dest_with(&["Hellboy/x.pdf"]),
                               &Komga).len(), 1);
}
#[test] fn refuses_a_symlinked_destination_component() {          // §6.11
    assert!(stream_verified(&src(), &through_symlinked_dir()).is_err());
}
#[test] fn never_follows_a_symlink_in_the_source() {
    assert!(select_mechanism(&symlink_into_real_library(), &dest(), Mode::Move).is_err());
}
#[test] fn two_rows_sharing_an_inode_are_reported_not_executed() {
    assert_eq!(find_collisions(&hardlinked_pair(), &empty_dest(), &Komga).len(), 1);
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** `stream_verified` writes a temp in the destination directory, `fsync`s, compares size and SHA-256, then `rename_excl`es into place. **It never unlinks the source** — `apply` owns that, after verification.
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 10: `scan`

**Files:** `scan.rs`, `tests/scan.rs`.

Pre-flight order is fixed by §6.10 and the order matters: `realpath` both roots and **refuse if either contains the other**; `stat().dev` for same-volume; probe the destination's case and normalisation sensitivity; sum **logical** sizes via `st_size`, never `du`; flag dataless sources and total `download_bytes`; select and record a per-row mechanism and `SourceStat`.

**Phase 1 is non-recursive** — top level only, which matches every reference bundle. `// shortcut: non-recursive; nested bundles need a decision on multi-format duplicates (open question 11.4)`. Multi-format duplicates become separate rows, and the summary reports how many titles have more than one.

Non-comic entries are **skipped silently**: extension allowlist `pdf|cbz|cbr|epub|cb7|cbt`, skip dotfiles, skip `^(id|readme|licen[cs]e|credits|cover|thumbs)[-_.]`, and **skip anything that is not a regular file** — exploded bundles ship directories named `Hellboy.pdf`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn refuses_overlapping_roots_in_both_directions() {
    assert!(matches!(scan(&nested_roots(), ..), Err(ScanError::RootsOverlap)));
    assert!(matches!(scan(&source_inside_dest(), ..), Err(ScanError::RootsOverlap)));
}
#[test] fn skips_non_comics_and_a_directory_named_pdf() {       // Review Focus 1
    let d = tree_from_names(&["mazebook.pdf","readme.txt","cover.jpg",
                              "id_imagecomics.pdf",".DS_Store"]);
    std::fs::create_dir(d.path().join("Hellboy.pdf")).unwrap();
    assert_eq!(scan(&opts_for(&d), ..).unwrap().1.len(), 1);
}
#[test] fn scan_leaves_both_trees_byte_identical() {
    let _g = TreeGuard::new(&[src.path(), dest.path()]);
    scan(&opts(), ..).unwrap();
}
#[test] fn records_logical_size_and_download_bytes_separately() {
    let h = scan(&opts_with_a_placeholder_and_a_sparse_file(), ..).unwrap().0;
    assert_eq!(h.required_bytes, 5<<30);
    assert!(h.download_bytes > 0, "D23: download cost is reported apart from disk cost");
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** The publisher hint comes from the **source folder's own name** (§6.3).
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

> ### First usable milestone
> After this task `scan` is read-only by construction and produces a plan of the
> owner's real 31.2 GiB bundle. **`cargo run -- scan …` is the first genuinely
> useful artifact** — Homebrew (Task 15) is irrelevant to the maintainer. Run it
> against the real library here, before building `apply`.

---

### Task 11: `apply` — refusals only

**Files:** `apply.rs` (pre-flight half), `tests/apply_refusals.rs`. **Every test wraps both trees in a `TreeGuard` with no `expect_changed`.**

Before touching anything: re-stat every source against its `SourceStat` and refuse changed rows; re-check free space for streaming rows, refusing the whole run when required exceeds available minus `max(1 GiB, 10%)`; re-verify each row's mechanism and **never silently fall back to streaming**; refuse a move whose source root is cloud-sync without an override (D23). Destinations are **recomputed** through the preset; a mismatch against `destination_preview` is a tampering signal.

Rows whose status is not `Ready` are never executed.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn a_refused_run_moves_zero_files() {
    // the test v1 was missing: Err alone proves nothing
    for plan in [tampered_destination(), mutated_source(), insufficient_space(),
                 cloud_source_move(), mechanism_changed_since_scan()] {
        let _g = TreeGuard::new(&[src(), dest()]);
        assert!(apply(&plan, ..).is_err());
    }
}
#[test] fn refuses_a_lock_held_by_another_run() {
    let _lock = acquire_lock();
    assert!(matches!(apply(&plan(), ..), Err(ApplyError::Locked)));
}
#[test] fn does_not_execute_review_rows() {
    let r = apply(&two_ready_three_review(), ..).unwrap();
    assert_eq!((r.executed, r.skipped), (2, 3));
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement the pre-flight and the lockfile.** The lock is an `O_EXCL` file beside the plan; `undo` takes it too.
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 12: Undo manifest and `apply` execution

**Files:** `undo.rs`, `apply.rs` (execution half), `tests/apply_exec.rs`.

**The manifest is append-only.** Each operation appends a `pending` row *before* it is attempted and a `done` row after; `reverse` reads the last status per id. v1 said `mark_done` rewrites the status in place, which is impossible — `"pending"` is 7 bytes and `"done"` is 4. Append is also crash-safe, which in-place rewriting is not.

`create` uses `O_EXCL`: re-running a plan must not truncate the previous run's manifest, which is the only record of where those files went. Each row records its `Mode`, because `reverse` of a copy **deletes the destination** while `reverse` of a move **moves it back** — and getting that backwards clobbers a kept original. `reverse` strips the `'` formula-defusal prefix before using a path, refuses when the original path is reoccupied, and is idempotent.

RFC 4180 quoting, doubled embedded quotes — the GCD lists *Hellboy, the Corpse and the Iron Shoes*.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn create_refuses_an_existing_manifest() {
    let _ = UndoWriter::create(&p()).unwrap();
    assert_eq!(UndoWriter::create(&p()).unwrap_err().kind(), ErrorKind::AlreadyExists);
}
#[test] fn pending_is_on_disk_before_the_operation() {
    let (mut w, path) = writer();
    w.record_pending(&a(), &b(), Mechanism::Rename, Mode::Move).unwrap();
    assert!(read_to_string(&path).unwrap().contains("pending"));
}
#[test] fn apply_aborts_when_a_pending_row_cannot_be_flushed() {
    let _g = TreeGuard::new(&[src(), dest()]);
    assert!(apply(&plan_with_readonly_manifest_dir(), ..).is_err());
}
#[test] fn a_failed_stream_leaves_the_source_intact() {
    // the critical v1 omission: verify -> rename -> THEN unlink was prose only
    let g = TreeGuard::new(&[src(), dest()]);   // no expect_changed
    assert!(apply_with_corrupted_temp(&copy_plan()).is_err());
    drop(g);
}
#[test] fn enospc_midstream_removes_the_temp_and_keeps_the_source() {
    let _g = TreeGuard::new(&[src()]);
    assert!(apply(&plan_onto_a_full_volume(), ..).is_err());
    assert_eq!(temp_files_in(dest()), 0);
}
#[test] fn reverse_of_a_copy_deletes_the_destination_only() {
    let _g = TreeGuard::new(&[src()]);          // source must be untouched
    apply(&copy_plan(), ..).unwrap();
    reverse(&manifest()).unwrap();
    assert_eq!(std::fs::read_dir(dest()).unwrap().count(), 0);
}
#[test] fn reverse_is_idempotent_and_refuses_a_reoccupied_path() {
    reverse(&manifest()).unwrap();
    reverse(&manifest()).unwrap();
    std::fs::write(original_path(), b"squatter").unwrap();
    assert!(reverse(&manifest()).is_err());
}
#[test] fn scan_apply_undo_restores_the_tree() {
    let before = snapshot(src());
    apply(&plan(), ..).unwrap();
    reverse(&manifest()).unwrap();
    assert_eq!(snapshot(src()), before);
}
#[test] fn an_interrupted_apply_reverses_the_completed_rows() {
    let r = apply_until_row(2, &five_row_plan());
    reverse(&manifest()).unwrap();
    assert_eq!(rows_still_moved(), 0);
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** Per mechanism: `Rename` is atomic with nothing to hash; `Clone` is `clone_excl`; `Stream` is temp → `fsync` → verify → `rename_excl` → **then** unlink for a move. A permission failure aborts the remaining rows rather than continuing.
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 13: CLI, disclosure, Komga pre-flight

**Files:** `crates/cli/src/main.rs`, `komga_api.rs`, `crates/cli/tests/cli.rs`.

```
comic-renamer scan <source> --dest <dir> --preset komga [--move|--copy] [--lang en] [-o plan.jsonl]
comic-renamer apply <plan.jsonl>
comic-renamer undo <undo-manifest.csv>
```

**The mode flag is on `scan`, not `apply`** — mechanism selection happens at scan, impossible if scan is never told the mode. `apply` takes no mode flag (§6.13).

Komga pre-flight folds in here rather than standing alone, because a free-standing checker nothing calls is dead code. `assert_safe` is pure and tested; `check_libraries` uses `ureq`. Enabled by `--komga-url`; the token comes from `COMIC_RENAMER_KOMGA_TOKEN`, never a flag, because flags land in shell history. Skipped when no URL is given. It asserts `hashFiles == true` and `emptyTrashAfterScan == false` and **refuses** — hashing enabled after a rename cannot help, because a deleted record can never acquire a hash (D35).

Disclosure states the mode, its consequence, disk arithmetic and **download cost separately**, then requires the typed word on first run and on any cross-volume plan.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn assert_safe_refuses_both_unsafe_settings() {
    assert!(assert_safe(&[lib(false, false)]).is_err());
    assert!(assert_safe(&[lib(true,  true )]).is_err());
    assert!(assert_safe(&[lib(true,  false)]).is_ok());
}
#[test] fn apply_rejects_a_mode_flag() {
    assert!(run(&["apply","plan.jsonl","--move"]).is_usage_error());
}
#[test] fn disclosure_separates_disk_from_download() {
    let o = run_apply_with_placeholders();
    assert!(o.contains("MOVE MODE") && o.contains("extra disk") && o.contains("DOWNLOADED"));
}
#[test] fn aborting_at_the_prompt_moves_nothing() {
    let _g = TreeGuard::new(&[src(), dest()]);
    apply_with_stdin("yes");
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement.** `clap` derive, `indicatif` fed by the progress callback, `comfy-table` for the summary.
- [ ] **Step 4: Run to verify they pass**
- [ ] **Step 5: Commit**

---

### Task 14: The gate

> **Superseded 2026-10-23 by `docs/plans/2026-10-23-mapping-first-rebuild.md`
> (its Task 13).** Do not implement this task. The three thresholds were
> dissolved rather than re-derived (D50): nothing is stored without approval, so
> a wrong guess costs typing, not a misfiled file. `cargo xtask measure` now
> reports and exits 0, and the hard checks are in `cargo xtask check-mappings`
> and the `mappings` CI job. The text below is kept for the reasoning only.

**Files:** `xtask/src/lib.rs` (grading logic, testable), `xtask/src/main.rs`, `.github/workflows/verify.yml`.

`cargo xtask measure` prints located-rate, ready-rate and silent false positive rate, exiting non-zero when any gate is breached. Logic lives in `lib.rs` because a bin-only crate cannot be imported by its own tests.

Corpora are committed: `fixtures/heldout-filenames.txt` (2,717 names) gives the located rate, ≥ **92.3%**; `fixtures/humble-ground-truth.tsv` (2,223 verified rows) gives ready ≥ **66%** and silent FP ≤ **0.48%**.

**Grade carefully — a naive comparison overstates the error rate five-fold.** The first measurement reported 2.65% and every sampled failure was the grader's fault: Humble writes `&` where the corpus writes "and", prefixes promos with "Free Issue" *including a zero-width character*, and inserts `Vol 1:` mid-title.

**And the grader must not drop leading articles or fold case** — the corpus's own `name_key` does, which makes it the natural implementation and would silently pass `MASK Omnibus` against *The Mask Omnibus*. That is the gate's whole job.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn accepts_the_four_grading_artifacts() {
    assert!(grade_ok("Love and Rockets", "Love & Rockets Vol. IV"));
    assert!(grade_ok("Wuvable Oaf: Blood & Metal", "Wuvable Oaf: Blood and Metal"));
    assert!(grade_ok("Red Sonja", "\u{200b}Free Issue Red Sonja #7"));
    assert!(grade_ok("Kevin Keller: Welcome to Riverdale", "Kevin Keller Vol 1: Welcome to Riverdale"));
}
#[test] fn still_fails_a_genuinely_wrong_answer() {
    assert!(!grade_ok("MASK Omnibus", "The Mask Omnibus"), "leading articles must NOT be dropped");
    assert!(!grade_ok("\u{30ce}\u{30e9}\u{30ac}\u{30df} [Noragami]", "Noragami Vol. 3"));
}
```

- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement and add a `gate` job to `verify.yml`**, restoring the corpus cache keyed on `corpus/manifest.json`'s sha256 (Task 3).
- [ ] **Step 4: Run the gate** — `cargo xtask measure` → located ≥ 92.3%, ready ≥ 66%, FP ≤ 0.48%, exit 0.
- [ ] **Step 5: Commit**

---

### Task 15: Homebrew

> **Not superseded.** The mapping-first rebuild did not touch this task. It is
> still open, and the build prerequisites it must state changed: Rust 1.88 or
> newer (not 1.80), plus a C toolchain from `xcode-select --install`.

**Files:** `Formula/comic-renamer.rb`, `.github/workflows/release.yml`, `README.md`.

Unsigned, via Homebrew (D7) — brew installs are exempt from the Gatekeeper failure that kills an unsigned *downloaded* binary silently, and a static Rust binary has no runtime to manage.

- [ ] **Step 1:** `brew install --build-from-source ./Formula/comic-renamer.rb` → FAIL, no formula
- [ ] **Step 2:** Write the formula and a tag-triggered release workflow that runs the gate before attaching the binary
- [ ] **Step 3:** `brew audit --strict ./Formula/comic-renamer.rb && comic-renamer --version` → clean
- [ ] **Step 4:** README install line, plus the honest ComicTagger prior-art note §8 requires
- [ ] **Step 5: Commit**

---

## Deferred, with reasons

Carried as `shortcut:` comments in the code, collected by `/ponytail-debt`:

- **Non-recursive scan**, and multi-format duplicates as separate rows (open question 11.4).
- **Corpus index clones records per key variant** — fix only if startup exceeds a second.
- **`clone_excl` passes flags `0`**; containment is `realpath`-per-component. `CLONE_RESOLVE_BENEATH` would move it into the kernel but `libc` exports no constant.
- **Per-row re-stat is a pre-flight batch**, so a row executing minutes later on a live-syncing tree uses a stale stat. Narrow the window by re-statting immediately before each operation if it bites.

## Phase 1 exit criteria

- [ ] Resolver fixtures pass (Tasks 5, 6).
- [ ] Located ≥ 92.3%, ready ≥ 66%, silent FP ≤ 0.48% (Task 14).
- [ ] `scan` → review → `apply` → `undo` round-trips against generated fixtures, including both sparse cases and every collision class (Tasks 8, 9, 12).
- [ ] Every mechanism refuses an existing destination **at the syscall** (Task 9).
- [ ] Every refusal path moves zero files, proven by `TreeGuard` (Tasks 11, 12).
