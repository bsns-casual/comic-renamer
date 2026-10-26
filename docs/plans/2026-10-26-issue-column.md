# Single issues as a first-class column — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a mapping row its own `issue` column so a single issue renders as
`<Series>/<Series> 0007.cbz`, zero-padded to four digits, with no `#` in the name.

**Architecture:** `issue: Option<u32>` is threaded through the four structures that
already carry `volume` — `MappingRow`, `Resolved`, `PlanRow` and the undo record's
`Fields` — and the Komga preset renders it between the volume marker and the
subtitle. Nothing about `volume` changes. Three data formats gain a column, so each
one's header or version is bumped in the task that owns it.

**Tech Stack:** Rust (workspace, edition 2024, `rust-version` 1.88); no new dependencies.

**Spec:** `docs/specs/2026-10-19-comic-renamer-design.md`, as amended by Task 1 of
this plan (§2 assumption 1, §6.8.1, §6.11, and decisions D59–D61 in §13).

## Global Constraints

- **Presets are the only code that constructs paths** (invariant 1). The issue
  rendering lives in `crates/core/src/preset/komga.rs` and nowhere else.
- **`apply` recomputes destinations from metadata** (invariant 2, D14). No task may
  make `apply` read a path string from a plan.
- **A record never authorises its own deletion** (invariant 11, D54/D58). Task 5
  adds a column to the record; it changes no gate and no reverse semantics.
- **The key rule is frozen** (invariant 10): NFC, lowercase, strip
  non-alphanumerics. No task in this plan touches `key()`. The pin
  `key_is_frozen_at_2036_over_the_humble_fixture` must stay green at 2,036 keys.
- **Minimise the code, never the proof.** Do not delete or weaken a test to make a
  signature change compile; extend it.
- Issue numbers are plain `u32`, ceiling **9999**, matching the existing
  `MAX_VOLUME_COUNT` precedent in `crates/core/src/mapping.rs`.
- Rendered order is **series, volume, issue, subtitle** — §6.8.1: "Filename sort
  order *is* reading order."
- No release has shipped, so **no migration code** for old-format files. A stale
  file must fail with a legible parse error naming the expected header.
- The tree is **not** formatted and CI runs neither `rustfmt` nor `clippy`. Do not
  reformat files you touch.

## Review Focus

Five conditions the convention implies that no task's happy path exercises. Each
line names the task that owns its test.

1. **Issue 0.** Real comics ship a `#0`. `Some(0)` must validate and render `0000`,
   not be treated as absent. — Task 3.
2. **An old-format mapping file or undo record.** Both now fail to parse. The error
   must name the expected header rather than panic or silently drop rows. — Tasks 2 and 5.
3. **Volume, issue and subtitle all set.** `Love and Rockets Vol. I #1 Facsimile
   Edition` is all three at once. — Task 3.
4. **A series name that ends in digits, carrying an issue.** `EC Cruel Universe 2`
   issue 1 must render distinctly from `EC Cruel Universe` issue 20001. — Task 3.
5. **Two rows differing only by issue.** They must be two destinations and one
   series folder, so collision detection groups them without refusing them. — Task 2.

---

### Task 1: Spec — single issues are in scope

**Files:**
- Modify: `docs/specs/2026-10-19-comic-renamer-design.md` (§2 assumption 1 at line 46;
  §6.8.1 table at lines 387–411; §6.11 field rules; §13 decision log, after D57)

No code and no tests. The deliverable is that an implementer of Tasks 2–9 can read
the convention off the spec.

- [ ] **Step 1: Rewrite §2 assumption 1**

It currently reads "**No issue numbers.** Every file is a collected edition —
omnibuses, `vol1`, archives, one-shots. Nothing maps to `{series} - #{issue}`."
Replace it with the measured truth: of 2,204 generated Humble rows, 1,305 carry a
volume, 151 are single issues (Humble wrote the `#` itself), and 748 are books with
neither. State that single issues are in scope for the MVP and that Sweet Shop,
GlobalComix and DriveThru Comics sell them directly.

- [ ] **Step 2: Amend the §6.8.1 table**

Change the row "A volume-numbered file with no issue number is an ordinary book" so
it no longer implies the corpus has no issues; keep its "No Special bucket"
consequence, which is still true and still Kavita-contrast. Add one row stating the
issue rendering: `<Series> [Vol. NN] IIII[ - Subtitle]`, issue padded to four digits,
no `#` in the name. Cite the existing row "**We pad anyway** (D37), because every
other tool sorts lexically" as the reason.

- [ ] **Step 3: Add the §6.11 rule for `issue`**

A numeric field with a ceiling of 9999, like `volume_count`. It is not subject to
the 120-byte field cap, which applies to text fields.

- [ ] **Step 4: Add D59, D60 and D61 to §13**

Follow the existing entry style: bold one-sentence ruling, then the reasoning.

- **D59 — Single issues are in scope for the MVP.** §2's "no issue numbers" was an
  assumption about one marketplace's catalogue, taken as a property of the problem.
  It is false: 151 of 2,204 Humble rows are single issues, and the marketplaces
  queued behind Humble sell them as their primary unit. Record that the premise was
  wrong rather than quietly widening scope.
- **D60 — An issue is its own column, padded to a fixed four digits, with no `#` in
  the name.** The subtitle could already hold `#7`, and 151 rows do, but a free-text
  subtitle gets none of D37's padding, so `#1 #10 #2` sorts wrongly everywhere but
  Komga. A fixed width needs no second count column, unlike `volume_count` (D44),
  because four digits covers every real run — the largest issue number in the corpus
  is 666. The `#` is dropped because it sorts and displays badly outside Komga.
- **D61 — `seed` derives an issue from the filename only when the filename
  corroborates the title.** Where the stem records `issue<N>` and the verified title
  ends in that same bare `N`, the number is an issue and not part of the series name.
  59 rows qualify. Where the filename does not corroborate, the trailing number stays
  where the parser put it: the corpus confirms some of them as whole titles
  (`RoboCop 2`), so inferring from the number alone would cost real titles.

- [ ] **Step 5: Commit**

```bash
git add docs/specs/2026-10-19-comic-renamer-design.md docs/plans/2026-10-26-issue-column.md
git commit -m "docs: single issues are in scope, and how an issue is rendered (D59-D61)"
```

---

### Task 2: `issue` on `MappingRow` and the TSV

**Files:**
- Modify: `crates/core/src/mapping.rs` (`HEADER` line 14, `MappingRow` line 36,
  `resolved()` line 48, the parse at lines 86–101, `to_tsv` line 113, `validate`)
- Modify: `data/mappings/fanatical.tsv`, `data/mappings/global-comix.tsv`,
  `data/mappings/sweet-shop.tsv` — header line only; all 12 rows get an empty `issue`
- Test: `crates/core/tests/mapping.rs` (round-trip and header), and
  `crates/core/tests/mapping_validate.rs` (the ceiling and the field rules) — this
  repo keeps those two surfaces in separate files; follow that split

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `MappingRow { key, series, volume: Option<u32>, issue: Option<u32>,
  subtitle, lang, volume_count, bundle }`; `const MAX_ISSUE: u32 = 9999`;
  `HEADER = "key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count\tbundle"`.
  `issue` sits at column index 3, so `subtitle`, `lang`, `volume_count` and `bundle`
  shift to 4, 5, 6 and 7, and the column count is 8.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_row_with_an_issue_round_trips_through_the_tsv() {
    let row = MappingRow {
        key: "beeandpuppycatissue1".into(), series: "Bee and PuppyCat".into(),
        volume: None, issue: Some(1), subtitle: None, lang: "en".into(),
        volume_count: None, bundle: "Humble BOOM! Bundle".into(),
    };
    let back = load_layer_for_test(&to_tsv(&[row.clone()]));
    assert_eq!(back.unwrap(), vec![row]);
}

#[test]
fn a_seven_column_file_is_refused_by_name() {
    let old = "key\tseries\tvolume\tsubtitle\tlang\tvolume_count\tbundle\nk\tS\t\t\ten\t\tB\n";
    let err = load_layer_for_test(old).unwrap_err();
    assert!(format!("{err:?}").contains("issue"), "the error must name the new header: {err:?}");
}

#[test]
fn an_issue_past_the_ceiling_is_refused() {
    let mut row = issue_row(10_000);
    assert!(row.validate().is_err());
    row.issue = Some(MAX_ISSUE);
    assert!(row.validate().is_ok());
}

#[test]
fn a_row_may_carry_a_volume_and_an_issue_together() {
    // Love and Rockets Vol. IV #9: 17 rows in the Humble table are both.
    let mut row = issue_row(9);
    row.volume = Some(4);
    assert!(row.validate().is_ok());
}

#[test]
fn two_rows_differing_only_by_issue_are_two_destinations_in_one_series() {
    // Review Focus 5. Uses the existing collision helper in this file.
    let rows = [issue_row(1), issue_row(2)];
    assert!(find_key_collisions(&rows).is_empty());
    let dests: Vec<_> = rows.iter().map(|r| Komga.destination(&r.resolved(), "cbz")).collect();
    assert_ne!(dests[0], dests[1]);
    assert_eq!(dests[0].parent(), dests[1].parent());
}
```

Add a local `issue_row(n: u32) -> MappingRow` helper beside the existing fixtures in
this file. `load_layer_for_test` is whatever this file already uses to parse a layer;
reuse it rather than adding a second path.

The last three tests go in `mapping_validate.rs`; the first two in `mapping.rs`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test mapping --test mapping_validate`
Expected: FAIL — `MappingRow` has no field `issue`.

- [ ] **Step 3: Add the field, the column and the ceiling**

In `crates/core/src/mapping.rs`: add `pub issue: Option<u32>` to `MappingRow` after
`volume`; put `issue` in `HEADER` after `volume`; change the parse's column count to
8 and read `issue: num(f[3], line, "issue")?`, shifting the later indices; write it
in `to_tsv` in the same position; pass it through `resolved()`; add
`const MAX_ISSUE: u32 = 9999` beside `MAX_VOLUME_COUNT` and reject an issue above it
in `validate`. Do not add a cross-field rule between `volume` and `issue`.

- [ ] **Step 4: Add the empty column to the 12 shipped rows**

Each file's header gains `issue`, and each data line gains an empty field in position
4. None of the 12 confirmed rows is an issue, so every value is empty.

- [ ] **Step 5: Run the suite**

Run: `cargo test --workspace`
Expected: PASS. `crates/core/tests/shipped_mappings.rs` enforces §6.11 over the
shipped rows and must stay green; `check-mappings` needs no change, because it checks
file size and rows-per-PR only, both column-agnostic.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/mapping.rs crates/core/tests/mapping.rs data/mappings
git commit -m "feat(mapping): an issue is its own column (D60)"
```

---

### Task 3: `Resolved.issue` and the Komga rendering

**Files:**
- Modify: `crates/core/src/types.rs` (`Resolved` line 47)
- Modify: `crates/core/src/preset/komga.rs` (`destination` line 63, beside `pad` line 107)
- Modify: `crates/core/src/resolve.rs` (three `Resolved` literals, lines 120, 182, 199)
- Test: `crates/core/tests/preset_komga.rs`

**Interfaces:**
- Consumes: `MappingRow::issue` from Task 2.
- Produces: `Resolved { series, volume, issue: Option<u32>, subtitle, run, status, record }`;
  `pub fn pad_issue(issue: u32) -> String` in `preset::komga`, fixed width 4.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_issue_renders_padded_to_four_digits_with_no_hash() {
    assert_eq!(dest(res("Bee and PuppyCat", None, Some(1), None)),
               PathBuf::from("Bee and PuppyCat/Bee and PuppyCat 0001.cbz"));
}

#[test]
fn issue_zero_is_a_real_issue() {
    // Review Focus 1.
    assert_eq!(dest(res("Spawn", None, Some(0), None)),
               PathBuf::from("Spawn/Spawn 0000.cbz"));
}

#[test]
fn a_volume_precedes_an_issue_and_both_precede_the_subtitle() {
    // Review Focus 3: Love and Rockets Vol. I #1 Facsimile Edition.
    assert_eq!(dest(res("Love and Rockets", Some(1), Some(1), Some("Facsimile Edition"))),
               PathBuf::from("Love and Rockets/Love and Rockets Vol. 01 0001 - Facsimile Edition.cbz"));
}

#[test]
fn a_series_ending_in_digits_stays_distinct_from_a_longer_issue_number() {
    // Review Focus 4.
    assert_ne!(dest(res("EC Cruel Universe 2", None, Some(1), None)),
               dest(res("EC Cruel Universe", None, Some(20_001), None)));
}

#[test]
fn a_row_with_no_issue_renders_exactly_as_before() {
    assert_eq!(dest(res("Saga", Some(3), None, None)),
               PathBuf::from("Saga/Saga Vol. 03.cbz"));
}
```

Add `res(series, volume, issue, subtitle) -> Resolved` and `dest(r) -> PathBuf`
helpers beside the file's existing fixtures.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test preset_komga`
Expected: FAIL — `Resolved` has no field `issue`.

- [ ] **Step 3: Add the field and render it**

Add `pub issue: Option<u32>` to `Resolved` after `volume`, and `issue: None` to the
three literals in `resolve.rs` (Task 6 gives two of them a real value). Add
`pad_issue` to `komga.rs`, formatting to width 4 unconditionally — it takes no `run`,
because D60 fixes the width. In `destination`, append the padded issue after the
volume marker and before the subtitle, separated by a single space, using the same
`stem.push_str` sequence the volume already uses.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/types.rs crates/core/src/preset/komga.rs crates/core/src/resolve.rs crates/core/tests
git commit -m "feat(komga): render a four-digit issue between the volume and the subtitle"
```

---

### Task 4: `PlanRow.issue` and plan version 3

**Files:**
- Modify: `crates/core/src/types.rs` (`PlanRow`)
- Modify: `crates/core/src/plan.rs` (`PLAN_VERSION` line 11 and its doc comment)
- Test: `crates/core/tests/plan.rs` (extend `round_trips_every_field` at line 94)

**Interfaces:**
- Consumes: `Resolved::issue` from Task 3.
- Produces: `PlanRow { …, issue: Option<u32>, … }`; `PLAN_VERSION = 3`.

- [ ] **Step 1: Extend the round-trip test and add a version test**

Give `round_trips_every_field` a non-`None` `issue` — the test at line 21 already
documents why a field must be non-default there — and add:

```rust
#[test]
fn a_version_2_plan_is_refused() {
    let err = read_v2_fixture().unwrap_err();
    assert!(matches!(err, PlanError::UnsupportedVersion { found: 2, expected: 3 }), "{err:?}");
}
```

Build the v2 fixture by writing a header with `"version": 2`, matching however this
file already constructs plan text.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test plan`
Expected: FAIL — no field `issue`.

- [ ] **Step 3: Add the field and bump the version**

Add `pub issue: Option<u32>` to `PlanRow` after `volume`; set `PLAN_VERSION = 3` and
extend its doc comment with what v3 adds, in the style of the existing v2 note.

- [ ] **Step 4: Run the suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/types.rs crates/core/src/plan.rs crates/core/tests/plan.rs
git commit -m "feat(plan): rows carry an issue; plan version 3"
```

---

### Task 5: The undo record carries the issue

**Files:**
- Modify: `crates/core/src/undo.rs` (`COLUMNS` line 24, `PAD` line 27, `Fields`
  line 247, the row writer around line 802, the destination rebuild at lines 402–410)
- Test: `crates/core/tests/undo_authority.rs` (the reverse), and
  `crates/core/tests/undo_schema.rs` (the column list and an old record) — this repo
  keeps the record's schema tests separate from its authority tests

This is the task where a miss is silent: the rebuild at line 402 constructs a
`Resolved` from the record's own columns and hands it to `preset.destination`. An
issue that does not travel with the row rebuilds the wrong leaf, and the reverse then
reports the file as not a destination this plan produces.

**Interfaces:**
- Consumes: `Resolved::issue` from Task 3.
- Produces: `COLUMNS` with `issue` after `volume`; `Fields { series, volume,
  issue: Option<u32>, subtitle, run, extension }`; `PAD` with eight commas.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_reverse_rebuilds_the_destination_of_an_issue_row() {
    // The record is the only input: no plan is read. If `issue` does not travel
    // with the row, the rebuilt leaf is "Curse.cbz" and the reverse refuses.
    let h = harness_placing(/* series */ "Curse", /* volume */ None, /* issue */ Some(3));
    let report = reverse_run(&h.lock, &h.record_path).unwrap();
    assert_eq!(report.restored, 1, "{report:?}");
    assert!(!h.dest.join("Curse/Curse 0003.cbz").exists());
}
```

`reverse_run` is `comic_renamer_core::apply::reverse_run(lock: &Lock, record: &Path)
-> Result<UndoReport, ApplyError>` — it takes the record's path, not a run id, and
the preset comes from the record's own `preset` column. Extend the file's existing
record-writing harness to take an issue rather than adding a second one.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p comic-renamer-core --test undo_authority`
Expected: FAIL — no field `issue`.

- [ ] **Step 3: Thread the column through**

Add `issue` to `COLUMNS` immediately after `volume`; add the matching comma to `PAD`
(seven becomes eight); add `issue: Option<u32>` to `Fields`; write and read it in the
row writer and parser at the same position; pass `issue: f.issue` into the `Resolved`
at line 402. Change no gate, no status handling and no `expected()` arm.

- [ ] **Step 4: Verify an old record fails legibly**

In `undo_schema.rs`, assert that `COLUMNS` contains `issue` between `volume` and
`subtitle`, that `PAD` has one comma per padded column, and that a record whose
header is the old `COLUMNS` string is reported as unreadable rather than panicking or
dropping rows. Review Focus 2.

- [ ] **Step 5: Run the suite, including the volume-gated tests**

Run: `cargo test --workspace`
Then, on a real second volume (see CLAUDE.md for the RAM-disk recipe):
`COMIC_RENAMER_TEST_VOLUME=/Volumes/CRTEST cargo test --workspace -- --ignored`
Expected: PASS, 546 of 546. Quoting the ungated count alone is how a stale
`#[ignore]`d test shipped once already.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/undo.rs crates/core/tests/undo_authority.rs
git commit -m "fix(undo): a record carries the issue, so a reverse rebuilds the right leaf"
```

---

### Task 6: The parser and resolver produce an issue

**Files:**
- Modify: `crates/core/src/title.rs` (`ParsedTitle`, `split_at_boundary` line 36,
  `drop_trailing_issue` line 82, and the doc comment at lines 29–32 that currently
  specifies the subtitle behaviour)
- Modify: `crates/core/src/resolve.rs` (the `-issue-N` suffix handling at lines
  125–141, `trailing_number_to_issue`)
- Test: `crates/core/tests/title.rs`, `crates/core/tests/resolve.rs`

**Interfaces:**
- Consumes: `Resolved::issue` from Task 3.
- Produces: `ParsedTitle { series, volume, issue: Option<u32>, subtitle }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_trailing_hash_number_becomes_the_issue_not_the_subtitle() {
    let p = parse_title("Red Sonja #7");
    assert_eq!((p.series.as_str(), p.volume, p.issue, p.subtitle), ("Red Sonja", None, Some(7), None));
}

#[test]
fn a_volume_and_a_trailing_issue_both_survive() {
    let p = parse_title("Love and Rockets Vol. IV #9");
    assert_eq!((p.series.as_str(), p.volume, p.issue), ("Love and Rockets", Some(4), Some(9)));
}

#[test]
fn a_collected_editions_issue_range_is_still_dropped() {
    // 31 titles carry a range; it describes contents, not one issue.
    let p = parse_title("Alex + Ada Vol. 1 (#1-5)");
    assert_eq!((p.series.as_str(), p.volume, p.issue, p.subtitle), ("Alex + Ada", Some(1), None, None));
}

#[test]
fn a_sweet_shop_issue_suffix_sets_the_issue() {
    // resolve.rs already decodes `-issue-3`; it must now land in the field.
    let s = suggest_for_test("young-shadow-the-watchdogs-issue-3", "Sweet Shop");
    assert_eq!(s.first().and_then(|s| s.issue), Some(3));
}
```

`suggest_for_test` stands for however `crates/core/tests/resolve.rs` already builds a
`Context` and calls the suggester; reuse that helper instead of adding one.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer-core --test title --test resolve`
Expected: FAIL — no field `issue`.

- [ ] **Step 3: Parse the issue into its own field**

Add `pub issue: Option<u32>` to `ParsedTitle`. In `split_at_boundary`, where a
trailing `#N` currently becomes the subtitle, parse `N` into `issue` and leave the
subtitle empty; keep `drop_trailing_issue` dropping ranges such as `(#1-6)`
unchanged, and keep a `#` inside a name surviving. Update the doc comment at lines
29–32, which specifies the old behaviour and is now wrong. In `resolve.rs`, make the
`-issue-N` suffix set `issue` instead of embedding `#N` in the text.

- [ ] **Step 4: Run the suite and the accuracy report**

Run: `cargo test --workspace`
Then: `cargo xtask measure`
Expected: tests PASS. `measure` exits 0 and reports `located`, `series accuracy` and
`suggestion accuracy`. Record the three numbers in the commit message. **None of them
is a gate** — a changed number is information, not a failure. The floor in
`report_coverage_over_the_heldout_set` is 85.0% located with a `total > 2000` guard;
leave it where it is.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/title.rs crates/core/src/resolve.rs crates/core/tests
git commit -m "feat(title): a trailing issue number lands in the issue field"
```

---

### Task 7: `map`'s detail editor edits the issue

**Files:**
- Modify: `crates/cli/src/map/detail.rs` (`enum Field` line 55, the Tab cycle at
  lines 96–106, `edit_field` line 108, `show` at line 78)
- Modify: `crates/cli/src/map/mod.rs` (the two `volume_count` construction sites,
  lines 111 and 801)
- Test: the `mod tests` in `crates/cli/src/map/detail.rs`

**Interfaces:**
- Consumes: `ParsedTitle::issue` from Task 6.
- Produces: `Field::Issue`, ordered between `Field::Volume` and `Field::Subtitle`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn tab_reaches_the_issue_between_the_volume_and_the_subtitle() {
    assert_eq!(next_field(Some(Field::Volume)), Some(Field::Issue));
    assert_eq!(next_field(Some(Field::Issue)), Some(Field::Subtitle));
}

#[test]
fn typing_digits_into_the_issue_field_sets_the_issue() {
    let mut d = detail_for("Curse");
    d.focus(Field::Issue);
    for c in "12".chars() { d.edit_field(Field::Issue, Some(c)); }
    assert_eq!(d.parsed().issue, Some(12));
}
```

Match the file's existing test helpers; `next_field` and `focus` stand for whatever
it already calls them.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p comic-renamer --lib map::detail`
Expected: FAIL — no variant `Issue`.

- [ ] **Step 3: Add the variant**

Add `Issue` to `Field`, place it in the Tab cycle after `Volume`, and give
`edit_field` an `Issue` arm that mirrors the `Volume` arm's digit handling. Include
the issue in `show`, so the sentence box displays the name the preset will produce.
Set `issue` at the two `map/mod.rs` construction sites.

- [ ] **Step 4: Run the suite**

Run: `cargo test --workspace`
Expected: PASS. The flagged-row guard has five clauses, each pinned by its own test
(see CLAUDE.md); all five must stay green. If one fails, the guard is comparing the
wrong representation again — fix the guard, not the test.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/map
git commit -m "feat(map): the detail editor edits the issue field"
```

---

### Task 8: `seed` derives an issue from a corroborating filename

**Files:**
- Modify: `xtask/src/seed.rs` (`build_row` and the `classify` classes)
- Test: the `mod tests` in `xtask/src/seed.rs`

**Interfaces:**
- Consumes: `MappingRow::issue` (Task 2), `ParsedTitle::issue` (Task 6).
- Produces: `fn issue_from_stem(stem: &str, title: &str) -> Option<u32>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_filename_that_corroborates_the_trailing_number_gives_an_issue() {
    assert_eq!(issue_from_stem("beeandpuppycat_issue1_1406925491", "Bee and PuppyCat 1"), Some(1));
    assert_eq!(issue_from_stem("protocol_orphans_issue4_1406925491", "Protocol: Orphans 4"), Some(4));
}

#[test]
fn a_bare_trailing_number_with_no_corroborating_filename_is_left_alone() {
    // D61: the corpus confirms some of these as whole titles.
    assert_eq!(issue_from_stem("10dance_1", "10 Dance 1"), None);
    assert_eq!(issue_from_stem("altair_arecordofbattles_vol1", "Altair: A Record of Battles Vol. 1"), None);
}

#[test]
fn a_filename_issue_that_disagrees_with_the_title_is_left_alone() {
    assert_eq!(issue_from_stem("fairyquest_issue1_1406925491", "Fairy Quest Outlaws"), None);
    assert_eq!(issue_from_stem("robocop_vol3_laststand_issue4-optimized", "RoboCop: Last Stand"), None);
}

#[test]
fn a_title_humble_already_hashed_needs_no_filename_rule() {
    // The parser handles these; issue_from_stem must not double-count.
    assert_eq!(issue_from_stem("archie_issue635", "Archie #635"), None);
}

#[test]
fn a_derived_issue_strips_the_number_from_the_series() {
    let row = build_row("Bee and PuppyCat 1", "beeandpuppycat_issue1_1406925491",
                        "Humble BOOM! Bundle", None, None).unwrap();
    assert_eq!((row.series.as_str(), row.volume, row.issue), ("Bee and PuppyCat", None, Some(1)));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p xtask`
Expected: FAIL — `issue_from_stem` not defined.

- [ ] **Step 3: Implement the rule**

`issue_from_stem` returns `Some(n)` only when the stem matches
`issues?[-_]?(\d+)` (case-insensitive), the verified title ends in that same bare
number, and the title contains no `#` — the parser owns the hashed ones. Build the
match without a regex crate: the workspace has none and this needs no dependency.
In `build_row`, when `issue_from_stem` fires, strip the trailing number and any
trailing separator from whichever of series or subtitle ends with it, and set
`issue`. The corpus and the fifth-column branches keep precedence as they are:
a keystroke and a lookup both outrank this rule (D50, D51).

- [ ] **Step 4: Add the class to `classify` and regenerate**

Add a class for a derived issue so the run's report shows the count, and keep "bare
trailing number" for the ones left alone.

Run: `cargo xtask seed`
Expected: `data/mappings/*.tsv` byte-identical except each file's header gains
`issue` (Task 2 already wrote that, so a regeneration must produce no further diff —
this is the check that `seed` still leaves confirmed rows alone). The report should
show about 59 derived issues moving out of the 208-row bare-trailing-number class.
Record the real class table in the commit message.

- [ ] **Step 5: Commit**

```bash
git add xtask/src/seed.rs data/mappings
git commit -m "feat(seed): derive an issue where the filename corroborates it (D61)"
```

---

### Task 9: The review artifact reflects the column

**Files:**
- Modify: the row generator for the review page (currently
  `/private/tmp/.../scratchpad/build_rows.py`; move it to `xtask` or `scripts/` if it
  is to survive, which is the owner's call)
- Modify: the published review page's `index.html` and `rows.json`
- Artifact: https://claude.ai/artifact/D1s5PBZRUdGxD2qt1aixRZ

The page is a review tool, not shipped product code, so it gets no Rust tests. Its
path preview mirrors `komga.rs` and must be re-checked against it.

- [ ] **Step 1: Regenerate `rows.json` with the issue field**

Each row gains `iss` (the proposed issue, empty or a number). Re-derive the flags:
rows whose issue is now set should leave the `bare trailing number` class.

- [ ] **Step 2: Add the Issue input and update the preview**

A fourth text input between Volume and Subtitle. Update `komgaPath` in the page to
render `{series}[ Vol. NN] IIII[ - subtitle]` with `pad_issue`'s fixed width 4 and no
`#`. Re-run the node harness that checks the preview against the preset's expected
output, extended with the Task 3 cases.

- [ ] **Step 3: Repoint "Make issue" at the new field**

The button currently moves a trailing number into the subtitle as `#N`. It must now
move it into the issue field and leave the subtitle alone. The stored decision gains
`iss`, so any decision already saved under the old shape must be re-confirmed — state
that on the page rather than migrating silently.

- [ ] **Step 4: Publish and verify the store**

Publish the same `file_path`, then one `ArtifactData` read of `decisions` to confirm
the new shape round-trips.

- [ ] **Step 5: Commit whatever moved into the repo**

```bash
git add scripts xtask
git commit -m "chore(review): the review page carries the issue column"
```

---

## Not in this plan

- **Kavita.** Dropped until a user asks (owner, 2026-10-25).
- **A per-series issue width.** D60 fixes four digits, so there is no `issue_count`
  column and no equivalent of D44's reasoning to re-derive.
- **Migrating old-format files.** No release has shipped. A stale mapping file or
  undo record fails with a legible parse error, which Tasks 2 and 5 pin.
- **Publishing the Humble table.** Still gated on the licence question, deferred by
  the owner on 2026-10-25.
- **The three reasoned-not-tested claims** in `perform`. Unrelated, and CLAUDE.md
  says to do them individually when next touching that function.
