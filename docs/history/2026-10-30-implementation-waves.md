# How the code got here — the finished implementation waves

Moved out of `CLAUDE.md` on 2026-10-30 so it stops loading into every session.
Nothing here is edited; it is the record as it stood when each wave landed.
Live guidance stayed in `CLAUDE.md` — the invariants, the known gaps, and the
techniques for running the suite.

Covered here: the issue column (D59–D61), session-scoped undo and the startup
menu (D54–D58), Task 13's departures from the Phase 1 text, and the rebuild's
final adversarial review.

---

**The rebuild is merged.** 60 commits, fast-forwarded onto `main` and pushed
on 2026-10-24 at `407fbbd`, after five adversarial review seats, a 16-item fix
wave and a mutation re-review of that wave. CI run 40061195019 is green. Every
SHA in this repository older than 2026-10-30 predates the history rewrite of
that date — see the Humble-table history file.

---

### Single issues, and the issue column (D59-D61)

Done on `main`, 18 commits ending `8609597` on 2026-10-26, planned in
`docs/plans/2026-10-26-issue-column.md`, 9 tasks. Pushed 2026-10-26 as
`747b080..84a0439`; CI run 40283202212 green on all six jobs.

**§1.1's first assumption was wrong.** It said "no issue numbers; every file
is a collected edition", measured against one Dark Horse bundle and
generalised into a property of the problem. Across the generated Humble table
1,305 rows carry a volume, **151 are single issues**, and 748 are books with
neither — and Sweet Shop, GlobalComix and DriveThru sell single issues as
their primary unit. Volumes and one-shots already worked, so the correction
cost one column rather than three features.

What it changed, each read from the code or run:

- **Four data shapes gained the field**: the mapping TSV (8 columns), `Resolved`,
  `PlanRow` (plan **v2 → v3**, and a v2 plan is refused rather than read as
  issue-less), `ParsedTitle`, `Suggestion`, and the undo record's CSV
  (**16 → 17 columns**). No migrations, by decision — no release has shipped,
  and every stale format now fails naming what it expected.
- **The Komga leaf is `<Series>[ Vol. NN][ IIII][ - Subtitle]`** (D60). Four
  digits, fixed, no run width: the ceiling is known and low, so there is no
  `issue_count` column. No `#`, because it sorts and displays badly outside
  Komga and buys nothing inside it.
- **The corpus and override suggestion paths pass no issue**, with the reason
  at each site: the corpus ladder reads a tokenised stem whose tokeniser has
  only `Token::Volume`, and Layer B's TOML has no issue field. A suggestion
  with no issue costs typing, never data.
- `cargo xtask measure` is **unchanged**: 91.6 / 62.9 / 48.9. Expected — the
  grader compares titles, and this adds a field it does not read.

Two things inserting a column broke that nothing would have caught:

- **The undo record's roots row** emitted 16 columns while its reader took
  `preset` and `planned` at literal indices 14 and 15. It surfaced as fifteen
  unrelated fixtures failing with `unknown preset ""`. All seventeen indices
  `undo.rs` hard-codes are pinned now by one test that names the cause.
- **`seed`'s de-duplication tuple** had been every semantic field, so it could
  fold an exact repeat but never a pair that disagreed — one line before
  `find_key_collisions`, the gate that exists to refuse such a pair. Measured
  unreachable (19 repeated `(key, bundle)` pairs, all with identical titles),
  which is why it needed a test rather than a reader. It is `dedup_identity`
  now.

### Session-scoped undo and the startup menu

Done on branch `worktree-session-scoped-undo`, 9 tasks, planned in
`docs/plans/2026-10-24-session-scoped-undo.md`. What it changed, each read from
the code or run on 2026-10-24:

- **The record is tool-written.** One file per run in
  `<config_dir>/runs` (`~/Library/Application Support/comic-renamer/runs`, or
  under `$XDG_CONFIG_HOME`), named `.comic-renamer.<unix-nanos>.<pid>.<seq>.record`
  and created on the first row that executes. It carries what a preset needs to
  rebuild each destination (series, volume, subtitle, run, extension), plus the
  preset id and the planned row count, so `reverse` needs no plan. The library
  lock is a file in the same directory named from a hash of the canonical
  destination root; it stays after the run ends.
- **`undo` is keyed on a run, not a plan** (D56). `comic-renamer undo` reverses
  the most recent run that no process still holds **and that has a row to put
  back** — the same gate the menu's offer uses. `undo --list` shows them, and
  `undo <run-id>` picks one, ungated: the user chose that run. Passing the plan
  path is a hard error (exit 1), which a test pins. `undo` has **no `--yes`**:
  it has no prompt to skip, and `clap` rejects the flag (exit 2).
- **"Interrupted" means the record does not account for everything it
  planned**, which is deliberately *not* "some row is `pending`". `apply`
  abandons the run on the first row that fails and writes that row `failed`, or
  leaves it `placed` when the destination landed and the unlink did not, so the
  likeliest real failure leaves no `pending` row. Two signals, in
  `undo::summarise`: a row that is not `done`, and fewer finished rows than
  `planned`. Neither subsumes the other; the comment there says which state
  each one reaches.
- **`apply` no longer prints the record's path.** It prints that `undo` puts
  the run back. A printed path invites being handed back in.
- **`comic-renamer` with no arguments opens a menu**, and refuses without a
  terminal exactly as `map` does. Keys: `k` declines a completed run's record
  and redraws, `l` leaves an interrupted run's record and redraws, and **`q`
  is the only key that ends the session and the only one that declines this
  session's run** (D55). An earlier cut let `l` and `k` exit, which gave `k` the
  power to delete a session run and left `l` unable to. **Item 4 owns this
  session's run alone** — that record is excluded from the two offers above, or
  the completed offer called this session's own `apply` "an earlier run" and
  `k` deleted what item 4 still pointed at.
- **`Trust::InSession` was cut during planning.** The design promised a fast
  reverse within the writing session. Verification is unconditional instead.
  See the 2026-10-24 design's §3.
- **Retention is one record per `(source_root, dest_root)` pair** (D57), and
  that is the policy the owner may want different. One exception: an
  *interrupted* record that still has rows to put back is never superseded,
  because the menu's `l` promises it stays. One with nothing to put back is
  swept, so a bundle failing on its first row leaves one record, not one per
  attempt. `apply` prints how many earlier runs the sweep made unreversible.

  **A record's `reversible` count is whatever `expected()` can identify, not the
  rows marked `placed` or `done`.** The two disagreed until an adversarial review
  on 2026-10-24: `expected()` has `(Pending, Rename) => source_identity`, because
  a rename moves the inode so the recorded source identity still describes the
  destination — so a crash in move mode's one-syscall window, the **default**
  mode, left a reversible row that the count called zero. `undo` then exited 0
  saying nothing was restored, the menu drew no offer, and the next run of the
  same bundle deleted the record, because D57's interrupted exception also reads
  that number. Derive the count from `expected()`; do not parallel it.
- **Both `apply` and `reverse_run` check that the `&Lock` they hold names the
  library they are about to touch.** `reverse_run` did not, which made the
  obligation a convention on exactly the path a reverse mutates a library from.

### Task 13's departures from the written text

This section is Phase 1 history. It speaks of `ready` and `review` rows; the
rebuild replaced that status model with `mapped` and `unmapped` (rebuild §6.2).
The departures themselves still stand.

Three, all deliberate, none hidden. Each is pinned by a test in
`crates/cli/src/main.rs`'s `mod tests`.

- **`--hardlink` is not advertised** in the disclosure. §6.10's example block
  lists it; D27 defers it out of Phase 1, so printing it would offer a flag
  `clap` rejects.
- **Typed confirmation runs on every `apply`**, not only "on first run and
  whenever the plan is cross-volume" (§6.10). Nothing persists a notion of
  first run, and the cheapest home for one — a marker beside the corpus — is
  state whose only purpose is to ask less. Stricter than the spec rather than
  looser. `--yes` is the explicit non-interactive consent; EOF reads as a
  refusal. The marker is the upgrade path if the prompt ever annoys.
- **The `--copy` advertisement states the whole payload**, not §6.10's literal
  "0 GiB extra disk". `apply::required_bytes` counts Clone rows, because
  `select_mechanism` picks Clone without probing and execution falls back to
  streaming — so a free-copy promise would be refused by the gate that had
  just described it. The first smoke run said "0 bytes free needed" for a
  2.9 MiB copy, which is how this was found.

And two additions §6.10 does not ask for, both found by mutation-testing the
CLI's own tests:

- The disclosure states **how many rows are not ready and stay where they
  are**. §6.9 measures one to two rows in three needing a human, so a block
  that counted only the ready rows would leave a third of the bundle in the
  source with no word about why.
- It **names every collision and why**, and excludes those rows from the
  headline count. `Collision.existing` is no longer unread. Before this,
  merging into a library that already held the file — which §6.10 calls the
  common case, not the rare one — printed "0 placed · 1 skipped" and nothing
  else, over a disclosure that had just promised to place it.

A run that can place nothing prints the reasons, skips the mode block and the
prompt entirely, and **exits non-zero**. A prompt over a run that will move
nothing teaches the user that the prompt does not mean anything, and an exit
code that is zero either way cannot tell a caller "the library matches the
plan" from "it does not".

**The CLI's tests are mutation-tested**, which is how the three findings above
were found. 14 mutations, 13 caught. The survivor — `Facts::gather` reporting
zero cloud placeholders — is unprovable locally: `SF_DATALESS` is
kernel-managed, `chflags` cannot set it, so no generated fixture is dataless.
It carries a `shortcut:` comment saying so. Re-run with
`python3 scripts/mutate-cli.py`, which is committed — it substitutes one
string at a time, runs `cargo test -p comic-renamer`, and reverts.

One conformance bug the departures list nearly hid: the disclosure advertised
`--copy` while `scan` only took `--mode copy`, so §6.10's own example block
named a flag `clap` rejected — the same defect the `--hardlink` guard exists to
prevent, one line below it. The guard's test looked for the literal string
"hardlink" and nothing else. It now scrapes every `--flag` out of a real
disclosure and asserts some command accepts it, and `scan` takes `--move` and
`--copy` as §6.13 writes them.

**The README's `undo comic-renamer-plan.jsonl` was the third instance of that
defect class**, found 2026-10-24 when session-scoped undo made the binary reject
a plan path, with a test passing that exact string and asserting a non-zero
exit. The first two were `--copy` and the `--hardlink` guard. The third was
found by running every command the README shows, not by editing the line that
was known to be wrong; do that whenever a command's shape changes. The README's
claim that `map` refuses without a terminal was also too broad: with every file
already mapped it ran with stdout piped, and it refuses only when a file needs
a name.

Phase 1 history: `undo` took the **plan** path, per D38, and the Phase 1 plan's
Task 13 still writes `undo <undo-manifest.csv>`. D56 has since reversed D38:
`undo` takes a run id, and a plan path is rejected.

### The rebuild's final review, and what it found

Five adversarial seats read the 44-commit branch, one fix wave closed 16
findings, and a mutation re-review of that wave established which of its new
tests can actually fail. Worth knowing before trusting anything below.

**The flagged-row guard was bypassable five different ways**, found one at a
time across the review rounds: a whitespace-only edit; parser round-trip drift
via `^R`; a trailing space the preset later strips, making the stored name
byte-identical to the squashed one; a no-op `pick`; and finally the *ambiguous*
class, where `acceptable`'s prefill clause was deletable with the whole suite
green. Each is now pinned by a test that fails when its clause goes. The
pattern is worth remembering: every one of the five was a guard that compared
the wrong representation — raw fields where it needed the rendered path, or the
rendered path where it also needed to know the user had typed something.

**`report_coverage_over_the_heldout_set` had zero assertions and CI ran it.**
It now holds a floor of 85.0% with a `total > 2000` guard so a fixture that
fails to load cannot pass vacuously. The floor **has been executed**: 91.6%
located over 2,717 held-out names, 6.6 points of headroom, and raising the
floor to 95.0 fails with the real number. It is not a gate in §7's sense. Do
not raise it to meet a measurement.

**Two findings are pre-existing and were deliberately not fixed here**, in
`docs/research/2026-10-23-two-pre-existing-data-safety-findings.md`: `undo` can
delete a library file no run ever placed and report success, and `apply`'s typed
confirmation gates a plan it re-reads afterwards. `undo.rs` is untouched by the
rebuild, so both were reachable on `main` at the time. The first was reproduced
and executed. **Both are closed by session-scoped undo**, in the section of that
name above.

