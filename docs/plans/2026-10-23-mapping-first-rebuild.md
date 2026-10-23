# Mapping-first rebuild implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace title *derivation* with title *lookup*, so a file bought from a comic marketplace renames from a recorded mapping rather than a corpus guess, and an unmapped file is mapped interactively and contributed upstream.

**Architecture:** A `MappingRow` keyed by a frozen normalisation of the filename stem is the source of truth. `MappingRow` converts to the existing `Resolved`, so `Preset::destination` and the whole execution half stay untouched. The GCD resolver is demoted to a suggestion engine behind an `Option<&Corpus>`. A `map` subcommand opens a ratatui session over unmapped files, writes approved rows to a local TSV, emits a plan, and delegates to `apply`.

**Tech Stack:** Rust 2021, `rust-version = "1.85"`. Existing: `clap`, `rusqlite`, `ring`, `serde`, `thiserror`. New: `ratatui` + `crossterm` (Task 8).

**Spec:** `docs/specs/2026-10-22-mapping-first-rebuild.md`, approved 2026-10-23. It amends `docs/specs/2026-10-19-comic-renamer-design.md`; read §2 of the former for what it supersedes. Kavita findings: `docs/research/2026-10-23-kavita-from-source.md`.

## Global Constraints

Every task's requirements implicitly include these.

- **The nine invariants in `CLAUDE.md` are not subject to minimisation**, and neither is the test discipline around them. Minimise the code, never the proof.
- **Presets are the only code that constructs paths** (invariant 1). No mapping field may contain a path or separator. No task in this plan modifies `crates/core/src/preset/`.
- **`apply` recomputes destinations from metadata** (invariant 2, D14). No task may make `apply` trust `destination_preview`.
- **No-clobber at the syscall** (invariant 6). No task replaces `renamex_np(…, RENAME_EXCL)`, `clonefile`'s native `EEXIST`, or `O_EXCL`.
- **Nothing the binary needs is read from the working directory.** Shipped data is `include_str!`-embedded. This defect has shipped once already.
- **Key rule, frozen:** NFC → lowercase → strip non-alphanumerics. Nothing else. Asserts **2,036** keys over `fixtures/humble-ground-truth.tsv`, zero collisions.
- **Caps:** 120 bytes per field; 255 bytes per rendered path component; 1024 bytes total rendered destination; 2,500 rows per PR; 2 MiB per mapping file.
- **Local user data** lives in `$XDG_CONFIG_HOME/comic-renamer/` else `~/Library/Application Support/comic-renamer/`. **Never** `~/Library/Caches` — do not reuse `cache_dir()`.
- **Mark deliberate shortcuts** with `// shortcut: <ceiling>; <upgrade path>`.
- macOS-only; `cargo` is at `~/.cargo/bin` and not on a non-interactive PATH.
- Commit and push directly to `main`.

## Review Focus

Five input classes the spec implies, that no task's happy path exercises, most likely first. Each has its test added to the task that owns the code.

1. **A stem that normalises to an empty key** — `___.pdf`, `(1).pdf`, a name of only punctuation. An empty key must never become a row, or one row matches every such file. Task 1.
2. **A hand-edited mapping file with CRLF or a BOM.** Contributors edit these in spreadsheets and on Windows. A trailing `\r` inside a key or series silently corrupts both. Task 1.
3. **An unreadable or malformed local mapping file.** These are the user's own corrections; losing them silently is worse than failing loudly. Task 3.
4. **Two approved rows in one session rendering to the same destination.** Must be reported in the confirmation, not discovered at the syscall. Task 10.
5. **The TUI with stdout not a TTY** — piped, redirected, CI. Must refuse with a clear message, never hang or emit escape codes into a pipe. Task 7.

---

## File Structure

**Create**
- `crates/core/src/mapping.rs` — `MappingRow`, `key()`, TSV parse/serialise, `Mappings` store, lookup, collision detection, §6.11 validation.
- `crates/core/src/title.rs` — `parse_title`, the sentence→fields parser and the boundary detection the grader shares.
- `crates/cli/src/map/mod.rs` — `map` command, orientation, confirmation, contribution.
- `crates/cli/src/map/table.rs` — the three-column approval table.
- `crates/cli/src/map/detail.rs` — the single-row detail editor.
- `data/mappings/` — shipped TSVs (populated in Task 12).

**Modify**
- `crates/core/src/types.rs` — `Status` becomes `Mapped`/`Unmapped`; add `MappingError`; bump `PLAN_VERSION`.
- `crates/core/src/scan.rs` — look up instead of resolve; corpus becomes optional.
- `crates/core/src/resolve.rs` — demoted; add `suggest`.
- `crates/core/src/lib.rs` — register `mapping`, `title`.
- `crates/cli/src/main.rs` — `map` subcommand; `config_dir()`.
- `xtask/src/lib.rs`, `xtask/src/main.rs` — grader uses `core::title`; gate becomes reporting.
- `.github/workflows/verify.yml` — mapping gate job.

**Untouched:** `crates/core/src/preset/`, `fsops.rs`, `apply.rs`, `undo.rs`, `plan.rs`'s writer, `komga_api.rs`.

---

### Task 1: The frozen key, the row, and TSV I/O

**Files:**
- Create: `crates/core/src/mapping.rs`
- Modify: `crates/core/src/lib.rs`, `crates/core/src/types.rs` (add `MappingError`)
- Test: `crates/core/tests/mapping.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn key(stem: &str) -> Option<String>`; `pub struct MappingRow { pub key: String, pub series: String, pub volume: Option<u32>, pub subtitle: Option<String>, pub lang: String, pub volume_count: Option<u32>, pub bundle: String }`; `pub fn parse_tsv(text: &str) -> Result<Vec<MappingRow>, MappingError>`; `pub fn to_tsv(rows: &[MappingRow]) -> String`; `impl MappingRow { pub fn resolved(&self) -> Resolved }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn key_is_frozen_at_2036_over_the_humble_fixture() {
    let keys: std::collections::HashSet<String> =
        rows(include_str!("../../../fixtures/humble-ground-truth.tsv"))
            .iter().filter_map(|r| key(&r.stem)).collect();
    assert_eq!(keys.len(), 2036, "the key rule is frozen; see D40");
}

#[test]
fn key_collides_for_no_two_distinct_titles() { /* 0 collisions over both fixtures */ }

#[test]
fn key_is_none_when_nothing_alphanumeric_survives() {
    assert_eq!(key("___"), None);
    assert_eq!(key("(1)"), None);
    assert_eq!(key(" "), None);
    assert!(key("a").is_some());
}

#[test]
fn parse_tsv_strips_crlf_and_a_bom() {
    let rows = parse_tsv("\u{feff}key\tseries\tvolume\tsubtitle\tlang\tvolume_count\tbundle\r\nx\tX\t\t\ten\t\tb\r\n").unwrap();
    assert_eq!(rows[0].key, "x");
    assert_eq!(rows[0].series, "X");
    assert_eq!(rows[0].bundle, "b", "a trailing CR must not survive in the last field");
}

#[test]
fn tsv_round_trips_a_title_with_a_slash_and_a_colon() {
    let r = MappingRow { series: "Hack/Slash: Back to School".into(), /* .. */ };
    assert_eq!(parse_tsv(&to_tsv(&[r.clone()])).unwrap()[0], r);
}

#[test]
fn resolved_carries_the_row_into_the_preset_unchanged() {
    // MappingRow::resolved() -> Resolved with Status::Mapped, so Preset::destination needs no change.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core --test mapping`
Expected: FAIL — `mapping` module does not exist.

- [ ] **Step 3: Implement `mapping.rs`**

`key` returns `Option<String>`: NFC, lowercase, retain `char::is_alphanumeric`, `None` when the result is empty. `parse_tsv` strips a leading BOM once, splits on `\n`, trims a trailing `\r` from each line, requires the exact seven-column header, and errors with the 1-indexed line number. `volume`/`volume_count` parse as `Option<u32>` from an empty-or-numeric field. `to_tsv` emits the header plus one line per row.

- [ ] **Step 4: Run to verify they pass**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core --test mapping`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/mapping.rs crates/core/src/lib.rs crates/core/src/types.rs crates/core/tests/mapping.rs
git commit -m "feat(core): the frozen key, the mapping row, and TSV I/O"
```

---

### Task 2: §6.11 validation — the reject/sanitise split and the caps

**Files:**
- Modify: `crates/core/src/mapping.rs`
- Test: `crates/core/tests/mapping_validate.rs`

**Interfaces:**
- Consumes: Task 1's `MappingRow`.
- Produces: `impl MappingRow { pub fn validate(&self) -> Result<(), MappingError> }`; `pub fn rendered_budget_ok(row: &MappingRow, preset: &dyn Preset) -> bool`.

Per the spec's §7.3 table: `: / \ | " < > ? *` are **stored verbatim** — `preset::komga::sanitize` already maps all nine. Rejected: tab, CR, LF, NUL and C0/C1 controls, mixed-script confusables, a field over 120 bytes, a leading `=` `+` `-` `@`, a field that is only dots, non-NFC input.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn accepts_a_real_title_with_a_slash_and_a_colon() {
    assert!(row("Hack/Slash: Back to School").validate().is_ok(),
        "26.8% of real titles carry a rejected character under the old list; D47");
    assert!(row("Hypnotwist / Scarlet By Starlight").validate().is_ok());
}

#[test]
fn rejects_what_sanitising_cannot_fix() {
    assert!(row("a\tb").validate().is_err(), "a tab breaks the TSV format itself");
    assert!(row("a\rb").validate().is_err());
    assert!(row("=cmd|'/c calc'").validate().is_err(), "contributors edit this in spreadsheets");
    assert!(row("Nemesis\u{0435}").validate().is_err(), "Cyrillic e makes an invisible duplicate series");
    assert!(row("..").validate().is_err());
    assert!(row(&"x".repeat(121)).validate().is_err());
}

#[test]
fn rejects_a_rendered_destination_over_budget() {
    // 255 bytes per component, 1024 total, measured through Preset::destination.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core --test mapping_validate`
Expected: FAIL — `validate` not found.

- [ ] **Step 3: Implement `validate` and `rendered_budget_ok`**

Confusable detection: reject a field whose characters span more than one Unicode script, ignoring `Common` and `Inherited`. Byte caps are on UTF-8 length, not `char` count.

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/mapping.rs crates/core/tests/mapping_validate.rs
git commit -m "feat(core): reject what sanitising cannot fix, store the rest verbatim"
```

---

### Task 3: The two-layer store, lookup, and collision detection

**Files:**
- Modify: `crates/core/src/mapping.rs`
- Test: `crates/core/tests/mapping_store.rs`

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces: `pub struct Mappings`; `impl Mappings { pub fn load(shipped: &str, local: Option<&Path>) -> Result<Mappings, MappingError>; pub fn lookup(&self, key: &str, bundle: &str) -> Option<&MappingRow>; pub fn len(&self) -> usize }`; `pub fn find_key_collisions(rows: &[MappingRow]) -> Vec<(String, String)>` returning `(key, bundle)` pairs held by rows with differing fields.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_single_row_matches_whatever_the_bundle_is() {
    // One row for a key -> returned for any bundle, including "".
}

#[test]
fn two_rows_for_one_key_are_disambiguated_by_bundle() {
    // Same key, different bundles, different series -> bundle selects; an
    // unmatched bundle returns None rather than guessing. D41.
}

#[test]
fn local_rows_outrank_shipped_rows() {
    // The local file is the only place a user can correct a wrong row.
}

#[test]
fn an_unreadable_local_file_is_an_error_not_a_silent_skip() {
    let err = Mappings::load("key\tseries\t…\n", Some(Path::new("/nonexistent/x.tsv")));
    assert!(err.is_err(), "the local file holds the user's own corrections");
}

#[test]
fn collisions_ignore_rows_that_merely_repeat_themselves() {
    // 158 of 2036 Humble keys span >1 bundle with identical titles: not a collision.
    assert!(find_key_collisions(&humble_rows()).is_empty());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core --test mapping_store`
Expected: FAIL

- [ ] **Step 3: Implement `Mappings`**

`load` validates every row (Task 2) and returns the first error with its line. A missing `local` path is `Ok`; a present-but-unreadable one is `Err`. Index as `HashMap<String, Vec<MappingRow>>`, local rows pushed ahead of shipped.

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/mapping.rs crates/core/tests/mapping_store.rs
git commit -m "feat(core): two-layer mapping store, bundle as the lookup tiebreaker"
```

---

### Task 4: The title parser

**Files:**
- Create: `crates/core/src/title.rs`
- Modify: `crates/core/src/lib.rs`, `xtask/src/lib.rs`
- Test: `crates/core/tests/title.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct ParsedTitle { pub series: String, pub volume: Option<u32>, pub subtitle: Option<String> }`; `pub fn parse_title(text: &str) -> ParsedTitle`; `pub fn split_at_boundary(text: &str) -> (&str, Option<u32>, &str)` — the boundary detection `xtask`'s grader also calls.

The grader in `xtask/src/lib.rs` returns normalised word vectors and discards the volume; `parse_title` preserves the user's string verbatim and keeps the volume. Both share `split_at_boundary`. Do not retrofit one into the other.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn parses_the_elfquest_example_from_the_spec() {
    let p = parse_title("The Complete ElfQuest Volume 1: The Original Quest TPB");
    assert_eq!(p.series, "The Complete ElfQuest");
    assert_eq!(p.volume, Some(1));
    assert_eq!(p.subtitle.as_deref(), Some("The Original Quest TPB"));
}

#[test]
fn preserves_case_and_punctuation_verbatim() {
    let p = parse_title("DIE: Loaded #1");
    assert_eq!(p.series, "DIE", "case is never folded; the grader's rule applies here too");
    let p = parse_title("Hack/Slash: Back to School");
    assert_eq!(p.series, "Hack/Slash");
}

#[test]
fn handles_every_humble_volume_form() {
    assert_eq!(parse_title("Harbinger, Vol. 2: Renegades").volume, Some(2));
    assert_eq!(parse_title("Love & Rockets Vol. IV").volume, Some(4));
    assert_eq!(parse_title("The Mask Omnibus Volume 1 (Second Edition)").volume, Some(1));
    assert_eq!(parse_title("X-O Manowar, Vol. 3:Planet Death").volume, Some(3));
    assert_eq!(parse_title("Minimum Wage Vol. 1 (#1-6)").subtitle, None);
}

#[test]
fn xtask_grader_still_passes_its_own_suite() { /* see Step 4 */ }
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core --test title`
Expected: FAIL

- [ ] **Step 3: Implement `title.rs` and repoint the grader**

Move the boundary logic from `xtask/src/lib.rs` into `title.rs`; `xtask` depends on `comic-renamer-core` already. Roman numerals: `IVXLCDM`, whole-token only — splitting `IVory` at `IV` would invent a volume.

- [ ] **Step 4: Run both suites**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core --test title && ~/.cargo/bin/cargo test -p xtask`
Expected: PASS, and all 8 existing grader tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/title.rs crates/core/src/lib.rs xtask/src/lib.rs crates/core/tests/title.rs
git commit -m "feat(core): the title parser, sharing boundary detection with the grader"
```

---

### Task 5: Status model and scan via lookup

**Files:**
- Modify: `crates/core/src/types.rs`, `crates/core/src/scan.rs`, `crates/cli/src/main.rs`
- Test: `crates/core/tests/scan.rs` (existing, extend)

`main.rs`'s `run_scan` calls `scan::scan(&opts, &corpus, &overrides, …)` today;
changing the signature breaks the workspace build until that call site moves to
`Mappings`. It is part of this task, not Task 7.

**Interfaces:**
- Consumes: Task 3's `Mappings`.
- Produces: `Status` is now `Mapped` | `Unmapped`; `pub fn scan(opts: &ScanOpts, mappings: &Mappings, progress: &mut dyn FnMut(Progress)) -> Result<ScanOutcome, ScanError>` where `pub struct ScanOutcome { pub header: PlanHeader, pub rows: Vec<PlanRow>, pub unmapped: Vec<PathBuf> }`.

`ReviewReason` and `Basis` are deleted. `PLAN_VERSION` bumps, because a plan written by the old resolver cannot be read by the new `apply`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_mapped_file_becomes_a_plan_row_and_an_unmapped_one_does_not() {
    // Two files, one in the mapping: rows.len() == 1, unmapped.len() == 1,
    // and the unmapped file's path appears in `unmapped`.
}

#[test]
fn scan_needs_no_corpus() {
    // scan's signature no longer mentions Corpus at all.
}

#[test]
fn a_plan_at_the_old_version_is_refused() {
    // plan::read on PLAN_VERSION - 1 -> Err, not a misparse.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core`
Expected: FAIL — plus pre-existing tests referencing `Status::Ready`/`Review` now fail to compile. Update them in Step 3; do not delete a test to make it compile.

- [ ] **Step 3: Implement**

Replace the `resolve` call in `scan.rs` with `mapping::key` then `Mappings::lookup`, keyed on the source folder's name as `bundle`. Mechanism selection, `required_bytes`, `download_bytes`, `SourceStat` and collision detection are unchanged.

- [ ] **Step 4: Run the whole suite**

Run: `~/.cargo/bin/cargo test --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/types.rs crates/core/src/scan.rs crates/core/tests/
git commit -m "feat(core): mapped or unmapped, and scan looks up instead of resolving"
```

---

### Task 6: The suggestion layer, per marketplace, corpus optional

**Files:**
- Modify: `crates/core/src/resolve.rs`, `crates/core/src/corpus.rs`
- Test: `crates/core/tests/suggest.rs`

**Interfaces:**
- Consumes: Task 4's `parse_title`.
- Produces: `pub struct Suggestion { pub series: String, pub volume: Option<u32>, pub subtitle: Option<String>, pub volume_count: Option<u32>, pub source: SuggestionSource }`; `pub enum SuggestionSource { Corpus, Filename, None }`; `pub fn suggest(stem: &str, marketplace: &str, corpus: Option<&Corpus>) -> Vec<Suggestion>`.

Per D53: `Filename` strategy title-cases a kebab/space stem and runs it through `parse_title`; `Corpus` keeps the existing ladder, with `ReviewReason::Ambiguous`'s candidate list becoming multiple `Suggestion`s rather than a refusal.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn no_corpus_still_suggests_and_never_errors() {
    let s = suggest("hammer-head-1_release-0001", "Global Comix", None);
    assert!(!s.is_empty(), "D49: absent corpus degrades to a filename guess, never an error");
}

#[test]
fn the_filename_strategy_is_wrong_for_die_loaded_and_that_is_expected() {
    let s = suggest("die-loaded-issue-1", "Sweet Shop", None);
    assert_ne!(s[0].series, "DIE",
        "the real answer is `DIE: Loaded #1`; this is why nothing is stored without a keystroke");
}

#[test]
fn ambiguity_becomes_several_suggestions_not_a_refusal() {
    // A stem the old ladder marked Review(Ambiguous) yields >1 Suggestion.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer-core --test suggest`
Expected: FAIL

- [ ] **Step 3: Implement `suggest`; make the corpus lazy**

`ensure_corpus` is called by the CLI only when a session has at least one unmapped file. A fetch failure logs and yields `SuggestionSource::None`.

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/resolve.rs crates/core/src/corpus.rs crates/core/tests/suggest.rs
git commit -m "feat(core): the resolver suggests now, and the corpus is optional"
```

---

### Task 7: The `map` command, orientation, and the non-TTY refusal

**Files:**
- Create: `crates/cli/src/map/mod.rs`
- Modify: `crates/cli/src/main.rs`
- Test: `crates/cli/src/map/mod.rs` `mod tests`

**Interfaces:**
- Consumes: Tasks 3, 5, 6.
- Produces: `pub fn run(source: PathBuf, dest: PathBuf, out: Option<PathBuf>, preset: String, mode: Mode) -> Result<(), String>`; `pub fn config_dir() -> Result<PathBuf, String>`; `pub fn orientation(unmapped: usize, total: usize) -> String`.

`map` takes `-o`, defaulting to `comic-renamer-plan.jsonl` as `scan` does — **never a temp directory**, because `undo` takes the plan path and derives the manifest beside it (D38, D45). A fully-mapped bundle skips the TUI, writes the plan, and reports that nothing needed mapping.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn config_dir_is_application_support_never_caches() {
    // With XDG_CONFIG_HOME unset, the path ends in "Application Support/comic-renamer".
    assert!(!config_dir().unwrap().to_string_lossy().contains("Caches"));
}

#[test]
fn orientation_names_the_count_and_leads_with_the_bundle_being_unknown() {
    let s = orientation(38, 40);
    assert!(s.contains("38"));
}

#[test]
fn a_non_tty_stdout_is_refused_with_a_message() {
    // Must not hang and must not emit escape codes into a pipe. Review Focus 5.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer map`
Expected: FAIL

- [ ] **Step 3: Implement the command and the TTY guard**

Guard with `std::io::IsTerminal` on stdout — std, no new dependency. Add the `clap` subcommand mirroring `scan`'s flags.

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/map/mod.rs crates/cli/src/main.rs
git commit -m "feat(cli): the map command, and a TUI that refuses a pipe"
```

---

### Task 8: The three-column approval table

**Files:**
- Create: `crates/cli/src/map/table.rs`
- Modify: `crates/cli/Cargo.toml` (add `ratatui`, `crossterm`)
- Test: `crates/cli/src/map/table.rs` `mod tests`

**Interfaces:**
- Consumes: Task 6's `Suggestion`.
- Produces: `pub struct Entry { pub stem: String, pub guess: Option<Suggestion>, pub final_fields: Option<ParsedTitle>, pub approved: bool }`; `pub struct Table { pub entries: Vec<Entry>, pub cursor: usize }`; `impl Table { pub fn render(&self, frame: &mut Frame); pub fn on_key(&mut self, k: KeyCode) -> Action }`; `pub enum Action { Continue, OpenDetail(usize), Done, Cancel }`.

Keys per the spec: `↑↓` row, `⏎` approve, `e` edit, `^A` approve all remaining, `^S` done. Pin `ratatui` and `crossterm` to exact versions compatible with `rust-version = "1.85"`; verify with `cargo check` before committing.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn approve_all_remaining_leaves_already_edited_rows_alone() {
    // ^A approves unapproved rows using their guess; a row with final_fields
    // set keeps them.
}

#[test]
fn a_row_with_no_guess_cannot_be_approved_by_enter() {
    // Enter on `guess: None` does not set approved; there is nothing to approve.
}

#[test]
fn render_shows_all_three_columns_and_the_approved_count() {
    // ratatui TestBackend: assert the buffer contains the filename, the guess
    // and the "N of M approved" counter.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer table`
Expected: FAIL

- [ ] **Step 3: Implement `Table` with `ratatui::backend::TestBackend` used in tests**

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/map/table.rs crates/cli/Cargo.toml Cargo.lock
git commit -m "feat(cli): the three-column approval table"
```

---

### Task 9: The detail editor and the rendered destination

**Files:**
- Create: `crates/cli/src/map/detail.rs`
- Test: `crates/cli/src/map/detail.rs` `mod tests`

**Interfaces:**
- Consumes: Task 4's `parse_title`, Task 8's `Entry`, `Preset::destination`.
- Produces: `pub struct Detail { pub text: String, pub parsed: ParsedTitle }`; `impl Detail { pub fn from_text(text: &str) -> Detail; pub fn on_input(&mut self, c: char); pub fn destination_line(&self, preset: &dyn Preset, ext: &str) -> String }`. Every mutation of `text` re-runs `parse_title`, so `parsed` is never stale.

The rendered destination is the safety mechanism: it goes through the real preset so a mis-parse is visible before anything is stored.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn typing_a_sentence_shows_the_parsed_fields() {
    let d = Detail::from_text("The Complete ElfQuest Volume 1: The Original Quest TPB");
    assert_eq!(d.parsed.series, "The Complete ElfQuest");
    assert_eq!(d.parsed.volume, Some(1));
}

#[test]
fn the_destination_line_is_rendered_by_the_real_preset() {
    let d = Detail::from_text("Hack/Slash: Back to School");
    let line = d.destination_line(&komga::Komga, "cbz");
    assert!(line.contains("Hack - Slash"), "the slash is sanitised by the preset, not stored mangled");
    assert!(!line.contains('/') || line.matches('/').count() == 1,
        "only the series/book separator");
}

#[test]
fn padding_comes_from_volume_count_not_from_the_volume() {
    // volume 2 with volume_count 100 renders Vol. 002 (D37, D44).
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer detail`
Expected: FAIL

- [ ] **Step 3: Implement `Detail`**

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/map/detail.rs
git commit -m "feat(cli): the detail editor, with the destination as the safety net"
```

---

### Task 10: Confirmation, the local write, the plan, and delegation to apply

**Files:**
- Modify: `crates/cli/src/map/mod.rs`
- Test: `crates/cli/src/map/mod.rs` `mod tests`

**Interfaces:**
- Consumes: Tasks 3, 8, 9; `plan::write`; the existing Task 13 disclosure in `main.rs`.
- Produces: `pub fn commit_session(entries: &[Entry], local: &Path) -> Result<usize, String>`; `pub fn intra_session_collisions(entries: &[Entry], preset: &dyn Preset) -> Vec<(String, String)>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn two_approved_rows_rendering_to_one_destination_are_reported_before_apply() {
    // "Usagi Yojimbo: 35 Years of Covers" and ": The Special Edition" with no
    // volume both render to one path. Must appear in the confirmation.
    // Review Focus 4.
    assert_eq!(intra_session_collisions(&entries, &komga::Komga).len(), 1);
}

#[test]
fn only_approved_rows_reach_the_local_file() {
    // An unapproved entry is absent from the written TSV.
}

#[test]
fn the_local_write_is_validated_before_it_lands() {
    // A row failing Task 2's validate() aborts the write; the file is unchanged.
}

#[test]
fn the_plan_is_written_where_undo_can_find_it() {
    // Default `comic-renamer-plan.jsonl` in the cwd, never a temp dir. D45.
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p comic-renamer map`
Expected: FAIL

- [ ] **Step 3: Implement**

Append approved rows to the local TSV, writing to a sibling temp file and renaming over it so an interrupted write cannot truncate the user's corrections. Then `scan` again with the updated `Mappings`, print the existing disclosure, and call `apply` on confirmation.

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/map/mod.rs
git commit -m "feat(cli): confirm, persist locally, plan, then delegate to apply"
```

---

### Task 11: Contribution via `gh pr create`

**Files:**
- Modify: `crates/cli/src/map/mod.rs`
- Test: `crates/cli/src/map/mod.rs` `mod tests`

**Interfaces:**
- Consumes: Task 10's approved entries.
- Produces: `pub fn contribution_tsv(entries: &[Entry]) -> String`; `pub fn contribute(tsv: &str, branch: &str) -> Result<ContributeOutcome, String>`; `pub enum ContributeOutcome { Opened(String), ManualSteps { path: PathBuf, commands: Vec<String> } }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_contribution_carries_no_filesystem_path() {
    let tsv = contribution_tsv(&entries);
    assert!(!tsv.contains('/') || !tsv.contains(std::env::var("HOME").unwrap().as_str()));
    // Rows are keyed by a normalised stem, so directory layout cannot leak. §7.2
}

#[test]
fn gh_absent_yields_manual_steps_rather_than_a_silent_failure() {
    match contribute(tsv, "b") { ContributeOutcome::ManualSteps { commands, .. } =>
        assert!(commands.iter().any(|c| c.contains("gh pr create"))), _ => panic!() }
}

#[test]
fn rows_the_user_declined_to_contribute_stay_local() { /* .. */ }
```

- [ ] **Step 2: Run to verify they fail**

Expected: FAIL

- [ ] **Step 3: Implement**

Detect `gh` with `Command::new("gh").arg("--version")`. Never pass a token; `gh` owns authentication.

- [ ] **Step 4: Run to verify they pass**

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/map/mod.rs
git commit -m "feat(cli): contribute a mapping with gh pr create"
```

---

### Task 12: Seed the shipped table

**Files:**
- Modify: `xtask/src/main.rs`
- Create: `data/mappings/humble.tsv`, `data/mappings/fanatical.tsv`, `data/mappings/global-comix.tsv`, `data/mappings/sweet-shop.tsv`
- Test: `crates/core/tests/shipped_mappings.rs`

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces: `cargo xtask seed` writing candidate TSVs from both fixtures.

Per D51, generated rows are **reviewed in the TUI before commit** — generation alone must not ship. Run `cargo xtask seed`, then `comic-renamer map` over the candidates, then commit the reviewed output.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn every_shipped_row_validates_and_no_key_collides() {
    let rows = parse_tsv(SHIPPED_HUMBLE).unwrap();
    assert!(rows.iter().all(|r| r.validate().is_ok()));
    assert!(find_key_collisions(&rows).is_empty());
}

#[test]
fn the_marketplace_fixture_is_fully_covered() {
    // All 12 rows of fixtures/marketplace-ground-truth.tsv resolve through
    // Mappings::lookup, including Hack/Slash and DIE: Loaded #1.
}

#[test]
fn shipped_files_are_embedded_not_read_from_the_cwd() {
    // include_str!, so an installed binary works from any directory.
}
```

- [ ] **Step 2: Run to verify they fail**

Expected: FAIL — `data/mappings/` is empty.

- [ ] **Step 3: Implement `seed`, then review and commit the output**

- [ ] **Step 4: Run the whole suite**

Run: `~/.cargo/bin/cargo test --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add xtask/src/main.rs data/mappings/ crates/core/tests/shipped_mappings.rs
git commit -m "feat(data): seed the shipped mappings, reviewed before commit"
```

---

### Task 13: The gate becomes three hard checks and two reports

**Files:**
- Modify: `xtask/src/main.rs`, `xtask/src/lib.rs`, `.github/workflows/verify.yml`
- Test: `xtask/src/lib.rs` `mod tests`

**Interfaces:**
- Consumes: Tasks 1, 2, 3, 12.
- Produces: `cargo xtask measure` reporting coverage and guess quality with **exit 0 regardless**; `pub fn rows_per_pr_ok(n: usize) -> bool`; `pub fn file_size_ok(bytes: usize) -> bool`; `gates_pass` and `Rates` deleted.

Per D50 the three thresholds dissolve. The hard checks are ordinary `cargo test` assertions from Tasks 1, 2 and 12, so CI needs only the existing test job plus a rows-per-PR check.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_planted_collision_fails_the_gate() {
    // Two rows, same key and bundle, different series -> find_key_collisions
    // returns it. This is what replaces §6.3's disambiguation apparatus.
}

#[test]
fn a_pr_over_the_row_cap_is_refused() {
    assert!(rows_per_pr_ok(2500));
    assert!(!rows_per_pr_ok(2501));
}

#[test]
fn a_mapping_file_over_two_mebibytes_is_refused() {
    assert!(!file_size_ok(2 * 1024 * 1024 + 1));
    // And every shipped file is well under it today.
    assert!(file_size_ok(SHIPPED_HUMBLE.len()));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `~/.cargo/bin/cargo test -p xtask`
Expected: FAIL

- [ ] **Step 3: Implement; delete `gates_pass` and `Rates`; add the CI job**

- [ ] **Step 4: Run the suite and the tool**

Run: `~/.cargo/bin/cargo test --workspace && ~/.cargo/bin/cargo xtask measure; echo "exit: $?"`
Expected: PASS, and `exit: 0`.

- [ ] **Step 5: Commit**

```bash
git add xtask/ .github/workflows/verify.yml
git commit -m "feat(xtask): hard checks on the mapping, reports on the guess"
```

---

## Task 14: Documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `docs/plans/2026-10-20-phase-1-implementation.md`

- [ ] **Step 1: Update `CLAUDE.md`** — the nine invariants gain the frozen key rule; the State section replaces Tasks 14/15 with this plan's status; the "Pick up here" section is rewritten; note that the resolver is now a suggestion engine.
- [ ] **Step 2: Update `README.md`** — `map` is the primary command; `xcode-select --install` and `gh` as prerequisites.
- [ ] **Step 3: Mark the old Phase 1 plan superseded** for Tasks 14–15.
- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md README.md docs/plans/
git commit -m "docs: the tool looks up titles now"
```
