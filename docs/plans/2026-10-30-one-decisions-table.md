# Plan: one decisions table

Implements `docs/specs/2026-10-30-one-decisions-table.md` (D63–D68). Nine tasks.
Read §2.1, §2.2 and §2.3 of the spec before starting any of them — they are the
corrections a self-review forced, and each one is a trap.

**Order matters.** T2 breaks every reader of the format, so T1 produces the new
file first and T3–T6 follow immediately. The tree is expected to be red between
T2 and T6; it must be green at every commit boundary, so T2–T6 land as one
commit if they cannot each stand alone.

---

## T1. The data

Produce `data/decisions.tsv`: every row from the three marketplace files,
collapsed to one row per key, `bundle` column removed, sorted by key.

- 12 rows in, 12 rows out — all 12 keys are unique, so nothing collapses here.
- Delete `data/mappings/fanatical.tsv`, `global-comix.tsv`, `sweet-shop.tsv`.
- Convert the owner's `<config>/mappings.tsv` in place: 11 rows, drop field 8.
  Without this his own corrections stop loading the moment T2 lands, and two of
  those rows were hand-corrected on 2026-10-30.

**Check:** the new file has 7 tab-separated fields on every line and a header
that matches the new `HEADER` exactly.

## T2. The format

`crates/core/src/mapping.rs`.

- `MappingRow` loses `bundle`.
- `HEADER` becomes `key\tseries\tvolume\tissue\tsubtitle\tlang\tvolume_count`.
- `parse_tsv` rejects an 8-field line **naming what it expected and what it
  found**, per the issue-column precedent: no migration, but no silent
  misreading either. A row whose 8th field is a bundle must not be read as a
  `volume_count` of something.
- `MappingRow::validate` drops its `bundle` rule; `validate_bundle` is deleted.

**Check:** a test feeds a real 8-column line and asserts the error names both
counts. This is the one test that protects every user who has a local file.

## T3. Identity

- `Mappings::lookup(key)` — one argument. The `[only] => Some(only)` arm becomes
  the only arm.
- `find_key_collisions` keys on `key` alone; its message drops "in bundle".
- `mapping::same_bundle` is **deleted**.
- The four sites in spec §2.2 become `key == key`.

**Check, and this is the task's whole point.** Spec §2.2: deleting `same_bundle`
deletes its tests, and the dissolution must not be indistinguishable from the
2026-10-23 regression it replaces. Each of the four sites gains a test that a
lookup or a merge matches on the key alone — including, explicitly, that two rows
whose *old* bundles differed only in case now resolve to one row rather than
being refused. `mapping_store.rs:53`'s "never silently pick" test is rewritten,
not deleted: with one row per key there is nothing to pick between, and the test
should say that.

## T4. Embedding

- `SHIPPED_FILES` becomes one `include_str!("../../../data/decisions.tsv")`.
  `shipped_mappings()` loses its loop and its "sharded by marketplace" comment.
- `SHIPPED_MARKETPLACES` and `shipped_mapping_path` are deleted (D66), with
  `every_shipped_marketplace_has_a_file_and_the_reverse`.
- `every_tsv_in_data_mappings_is_embedded` becomes
  `the_decisions_file_is_the_one_that_ships`: still the guard that a stray TSV
  under `data/` does nothing silently.

## T5. `map` and contribution

- `approved_rows` groups on key; `commit_session` merges on key and **replaces
  in place** (spec §2.3) — newest decision wins, never appends a duplicate key.
- `contribution_tsv` drops the column and its "a contribution never carries a
  path" guard, which existed only to validate a bundle.
- The printed commands append to `data/decisions.tsv` unconditionally. The
  "contributions are not open for this marketplace" branch goes, with
  `a_marketplace_with_no_shipped_file_prints_no_commands`.

**Check:** a second session over one folder that re-decides a file leaves one
row, with the newer fields, and the file still loads. That is the failure
CLAUDE.md's invariant-7 note warns about arriving by a different road.

## T6. `seed`

- Writes `data/decisions.tsv` with 7 columns for the shipped rows.
- **Leaves its Humble intermediate and sidecar at 8 columns, unchanged**, and
  leaves their filenames alone (spec §2.1, O-2). `scripts/review-rows.py` reads
  those eight fields in order and `sys.exit`s otherwise; 2,204 stored decisions
  are identified by `(key, bundle)`.
- `dedup_identity` keeps working on the generator's own tuple.

**Check:** `cargo xtask seed` leaves `data/decisions.tsv` byte-identical, and
`python3 scripts/review-rows.py` still produces the review page's data without
erroring.

## T7. CI

- `mappings` job: file cap 2 MiB → 4 MiB (D67), path `data/mappings/*.tsv` →
  `data/decisions.tsv`.
- The 2,500-rows-per-PR cap and its PR-only nature are unchanged.

**Check:** run `cargo xtask check-mappings` and the workflow's own shell locally.

## T8. Docs

- CLAUDE.md: invariant 7 loses its bundle half and gains a pointer to spec §2.2;
  the layout block, the "What ships" section, and the test counts.
- README: wherever it names a marketplace file.
- Fold D63–D68 into the first design's §13.

## T9. Adversarial review

Four seats, per this repo's habit: one ponytail for over-build, one on data
safety, one on the format break, one on the tests-that-pass-for-the-wrong-reason
problem that has caught three of my own tests in this project already. Then
mutation-check every new test by hand: revert the behaviour, confirm that test
and only that test fails.

**Specific mutations to run, because each has a plausible silent survivor:**

1. `parse_tsv` accepts 8 fields and ignores the last → T2's test must fail.
2. `lookup` returns `None` for a key with one row → something must fail.
3. `commit_session` appends instead of replacing → T5's test must fail.
4. `find_key_collisions` never reports → the frozen-key pin must fail.
5. The file cap stays 2 MiB → T7's check must notice.

---

## Out of scope, deliberately

- Renaming `--marketplace` (spec O-1). Separate change, after this one.
- Publishing `review/humble-decisions.tsv`. Untouched, still private, still
  gated on the licence question (spec §6).
- Un-embedding the table. Revisit past 4 MiB.
- `scan` reading the user's local mappings. A marked shortcut and a separate
  decision the owner has not made.
