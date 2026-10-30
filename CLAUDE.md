# Comic Renamer — working notes for agents and contributors

A command-line tool that renames comic files bought in distributor bundles
(Humble Bundle and similar) into the layout a comic library app expects. It
identifies each file, shows a reviewable plan, and changes nothing until the user
approves it.

**Read `docs/specs/2026-10-19-comic-renamer-design.md` before changing anything.**
Its §13 decision log records every call and why; D1–D68. If you think a decision
is wrong, read its entry first — most have survived an adversarial review and the
reasoning is written down. Say what's wrong with the reasoning rather than
re-deciding silently.

**Then read `docs/specs/2026-10-22-mapping-first-rebuild.md`.** The tool no longer
derives a title by resolving a filename against a corpus; it looks the filename up
in a recorded mapping. That design supersedes parts of the first (its §2 lists
which), and its decisions, D39–D53, are folded into the first document's §13.

**Then read `docs/specs/2026-10-24-session-scoped-undo-and-startup-menu.md`.**
The record of what a run did is now a file the tool writes into a directory it
owns, `undo` is keyed on a run rather than a plan, and no record is the
authority for a deletion. Its decisions, D54–D57, are folded into §13 and
amend D38.

---

## Invariants

Each of these was discovered the hard way, usually by something breaking. Each is
also the kind of thing a competent contributor would reasonably undo. **Do not,
without changing the spec first.**

1. **Presets are the only code that constructs paths.** Rule data never contains
   a path, a separator, or anything path-shaped. A bad rule may produce a wrong
   *name*; it must never produce a wrong *location*. (§6.11)
2. **`apply` recomputes destinations from metadata.** It never trusts a path
   string found in a plan file — plans get handed to AI assistants and attached
   to public PRs. (§6.9, D14)
3. **Size zero is never an error.** The fixture suite is built from generated
   zero-byte files. An "empty means corrupt" guard silently disables all of it.
   (§6.10, D21)
4. **Never commit the corpus binary.** Bump `corpus/manifest.json` instead. (§6.5, D19)
5. **The resolver never opens a file.** Filenames only, always. It is a
   suggestion engine now (invariant 10), but that does not loosen this. (§5)
6. **No-clobber is enforced at the syscall, not by a prior check.**
   `renamex_np(…, RENAME_EXCL)` on move, `clonefile`'s native `EEXIST`, `O_EXCL`
   on streams. A collision check followed by plain `rename(2)` is a data-loss
   bug, because `rename` overwrites silently. (§6.10, D24)
7. **Collision comparison is NFC-normalised and case-folded**, never a literal
   path string — `Hellboy/` and `hellboy/` are one directory on APFS and two
   series on a Linux server. Emitted paths are always NFC. (§6.10, D2)
   **The `bundle` half of this invariant retired on 2026-10-30 with the column
   it guarded** (D64, `docs/specs/2026-10-30-one-decisions-table.md` §2.2).
   `mapping::same_bundle` and all four of its call sites are gone, because a
   bundle is no longer part of a row's identity and so there is no bundle
   comparison left to get wrong. Read §2.2 before concluding that a guard was
   dropped carelessly: it was broken there once, until 2026-10-23, and each of
   the four sites gained a key-only test rather than simply losing its old one.
   **The path half above is untouched and is not negotiable.**
8. **Dataless detection reads `SF_DATALESS`.** Not `st_blocks == 0`, which
   misclassifies legitimate sparse files. (§6.10, D25)
9. **Evaluation input is distributor bundles only.** Not scene releases, not
   hand-renamed files. Tests use generated files, never anyone's real library.
   (§7, D12)
10. **The key rule is frozen: NFC, lowercase, strip non-alphanumerics, nothing
    else.** The key is effectively a hash of the filename, so changing the rule
    invalidates every mapping row ever contributed. It is a breaking data
    migration with a version bump, not a refactor. The pin is
    `key_is_frozen_at_2036_over_the_humble_fixture` in
    `crates/core/tests/mapping.rs`. Resist adding "just strip the download id" or
    "just drop `_hd`": deciding which parts of a filename are noise is inference,
    and a token that looks like noise in one marketplace is a title in another.
    (rebuild §5.3, D40)
11. **A record never authorises its own deletion — and since D58 no reverse
    deletes a library file at all.** Reversing a copy renames it into
    `<dest_root>/.comic-renamer-aside/<run-id>/`, leaf name only, through
    `Entry::rename_into`'s `RENAME_EXCL`. The record may say *which* file,
    never *that it is safe to displace*. That is still verified against the
    filesystem — the source it was copied from must still be there, with the
    same content — but the check now decides whether the reverse is *correct*,
    not whether it is *safe*: an adversarial review showed its source hash is
    check-then-act and cannot be made sound, because the bundle folder has no
    lock by design. So the destructive step went instead of the window
    shrinking. **Nothing empties the aside directory**; a reverse reports its
    path and count and the user deletes the files. A record is a map, not a
    warrant. (session-scoped-undo §2, D54, D58)

Also load-bearing, though not numbered: **nothing is stored without a keystroke.**
A guess is a typing aid. The mapping session writes only rows the user approved,
and `^A` (approve all remaining) is reported in the final confirmation. (rebuild §4)

### The gate dissolved, and what replaced it

§7's silent-false-positive gate existed because a wrong guess silently misfiled a
file. It was never measured against the implemented resolver until the rebuild
did so on 2026-10-22: **10.12%**, against the 0.48% the research had reported.
The decision was not to fix the resolver to meet the gate (D50). Nothing is
stored without approval now, so there is no silent false positive left to gate,
and the three thresholds were dissolved rather than re-derived.

`cargo xtask measure` reports and exits 0. Against the 2,223 verified pairs in
`fixtures/humble-ground-truth.tsv` (pulled from the maintainer's own Humble
library, where every download is published next to its real title):

```
located               91.6%   held-out, 2,717 names
series accuracy       62.9%   top suggestion's series alone
suggestion accuracy   48.9%   top suggestion against the verified title
```

**None of these is a gate. Do not turn one into a merge gate.** A wrong suggestion
costs the user some typing. If a number looks bad, the response is a better
suggestion, not a failed build.

The checks that do fail the build (`cargo xtask check-mappings` and the `rust`
job's `cargo test --workspace`):

- No two mapping rows share a key with different fields. Compare *fields*, not
  row counts: 158 of 2,036 Humble keys appear under more than one bundle, with an
  identical title each time.
- Every §6.11 per-field rule holds for every shipped row (rebuild §7.3).
- The frozen key rule, pinned at 2,036 keys (invariant 10).
- `data/decisions.tsv` is at most 4 MiB, on every push and PR (D67). The cap
  is set by the binary, not the file: the table is `include_str!`-embedded.
- A PR adds at most 2,500 rows. **This one is PR-only.** `check-mappings` diffs
  against the PR's base, so a direct push to `main` can add any number of rows.
  That is deliberate: the cap bounds contributed data, which arrives as a PR
  (§6.11 says "per PR"), and the conventions below sanction the maintainer
  pushing directly. Do not read the green `mappings` job on a push to `main` as
  having checked the row count.

**Grade against the ground truth carefully.** A first pass reported 2.65% and
every sampled failure was the grader's fault: Humble writes `&` where the corpus
writes "and", prefixes promo items with "Free Issue" (containing a zero-width
character), and inserts `Vol 1:` mid-title. Normalise both sides or you will
chase phantoms. The seed generator shares the grader's promo-prefix logic for the
same reason.

---

## Layout

```
docs/specs/      design documents          docs/plans/      implementation plans
docs/research/   reviews and experiments   fixtures/        test data, filenames only
corpus/          manifest.json — the pinned corpus. Never the .sqlite itself.
data/decisions.tsv    the shipped filename→decision table. One file, one row
                      per key, no `bundle` column (D63, D64)
data/overrides.toml   first-party Layer B title overrides
review/          the reviewed Humble table, decided but not shipped — see its README
xtask/           cargo xtask measure | seed | check-mappings [base-ref]
scripts/         checks and tools, runnable locally:
                 python3 scripts/check-manifest.py
                 python3 scripts/review-rows.py <out.json>   the review page's data
                 python3 scripts/export-decisions.py <dump> <out.tsv>
                 node scripts/check-review-page.cjs          its path mirror vs komga.rs
                 scripts/review-page.html                    the Humble review page itself
.github/         workflows: verify (every PR), corpus-refresh (monthly)
```

Dated `YYYY-MM-DD-<topic>.md`.

## State

Design approved. **Phase 0 is complete. Phase 1 is complete, and the tool was then
rebuilt around a recorded filename→title mapping.** The rebuild is
`docs/specs/2026-10-22-mapping-first-rebuild.md`, planned in
`docs/plans/2026-10-23-mapping-first-rebuild.md`, 14 tasks, all done. The old
Phase 1 plan's Task 14 (the gate) is superseded by it; its Task 15 (Homebrew) is
not, and is still open.

**The rebuild is merged.** 60 commits, fast-forwarded onto `main` and pushed on
2026-10-24 at `407fbbd`, after five adversarial review seats, a 16-item fix wave
and a mutation re-review of that wave. CI run 40061195019 is green.

`scan`, `map`, `apply` and `undo` all run from the CLI. **`map` is the primary
command for a bundle the tool has not seen**: it needs a terminal, shows each
unmapped filename beside a guess, and writes a plan only from rows the user
approved. **`scan` is the non-interactive path**: it plans the mapped files,
lists the unmapped ones with a guess for each, and saves nothing. `apply` and
`undo` are unchanged by the rebuild. The resolver is now a suggestion engine,
and the corpus is lazy and optional: a missing or undownloadable corpus means "no
guess", not an error (D49).

```
done   repo harness, CI, corpus manifest            (this file, .github/, corpus/)
done   D23  cloning a placeholder hydrates it -- move stays the default
done   D26  ground truth, 2,223 verified rows
done   D34  Komga convention, specified from source (sections 6.8.1, 6.8.2)
done   T1-T13  Phase 1: workspace, normalise, corpus, overrides, resolver, Komga
               preset, plan, sparse fixtures, no-clobber syscalls, scan, apply
               pre-flight, apply execution and undo, CLI
done   R1-R14  the rebuild: frozen key and TSV I/O, §6.11 validation, the
               two-layer store, the title parser, scan via lookup, suggestions per
               marketplace, `map`, the approval table, the detail editor,
               confirmation, contribution, seeding, the three hard checks,
               documentation
done   D59-D61  single issues are first class: an `issue` column, rendered as
               four padded digits with no `#`, derived where a filename or a
               sibling of the same verified title vouches for it
done   D62  a shouting series is settled by the corpus where case alone decides
               it, guessed and still reviewed where the corpus is silent
todo   T15  Homebrew — the release workflow and the formula are done and
               tested; three owner steps remain. Deferred behind the MVP
               on 2026-10-27. See "Task 15, Homebrew" below.
done   the review's 2,204 decisions are exported and committed to `review/`,
       validated through the real gates. Shipping them waits on the licence.
```

### Single issues, and the issue column (D59-D61)

Done on `main`, 18 commits ending `8609597` on 2026-10-26, planned in
`docs/plans/2026-10-26-issue-column.md`, 9 tasks. **Not pushed yet.**

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

### What the volume-gated suite does and does not protect

**Measured 2026-10-26, so nobody re-derives it.** The five
`COMIC_RENAMER_TEST_VOLUME` tests were run against this branch: **604 of 604
pass**, the first time every gated test has run locally here. The suite is 615
now, and those five have not been re-run since; D62 touches none of their
code. Three of the
five call `reverse_run` or `record_pending`, which is exactly the code the
issue column changed — and that does **not** mean they protect it. Each of
these record-format mutations **survives all five** and is caught by the
ungated suite:

```
PAD one comma short                  gated: survived   ungated: caught
subtitle read from the issue column  gated: survived   ungated: caught
the rebuild drops the issue          gated: survived   ungated: caught
```

Their rows are volumes and one-shots, which do not discriminate a column
layout. They discriminate the streamed path, the `placed` state and
abort-on-failure, exactly as this file already said — and nothing more. So a
change to the record *format* does not need the volume suite; a change to
cross-device *behaviour* does.

A disk image is the safer way to get one, and needs no `diskutil`:

```
hdiutil create -size 1400m -fs HFS+ -volname CRTEST /tmp/crtest.dmg
hdiutil attach /tmp/crtest.dmg
COMIC_RENAMER_TEST_VOLUME=/Volumes/CRTEST cargo test --workspace -- --ignored
hdiutil detach /Volumes/CRTEST && rm /tmp/crtest.dmg
```

`hdiutil create` takes a file path, never a device node, so it cannot be
pointed at real hardware. 1400 MiB clears the `max(1 GiB, 10%)` reserve that
otherwise refuses every run.

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

### What ships, and what does not

**Shipped coverage is 12 rows**, all in `data/decisions.tsv`: titles the
maintainer confirmed one at a time, from Fanatical (5), Sweet Shop (5) and
Global Comix (2). The per-marketplace files were merged on 2026-10-30 (D63);
the marketplace still structures `fixtures/marketplace-ground-truth.tsv`, which
is where that breakdown now lives. `cargo xtask seed` regenerates them from
`fixtures/marketplace-ground-truth.tsv`.

**The Humble table is generated and deliberately not shipped.** `cargo xtask seed`
also writes about 2,200 candidate rows to `target/seed/humble.tsv`, which is
untracked. It waits for the maintainer to review it, because D51 and the rebuild's
§4 forbid storing rows nobody approved, and committing 2,200 unreviewed rows on
the first commit would do exactly that. On its last run (2026-10-25) the
generator built 2,223 rows, rejected 1 (a short line), found 0 key collisions, and
de-duplicated to 2,204. Its own classification of the 2,223, after the issue
column (D59–D61) and the casing rule (D62) landed on 2026-10-26 and 2026-10-27:

```
1525   68.6%  clean
 189    8.5%  issue written in the verified title (#N)
 128    5.8%  corpus settled the series casing          (D62, not reviewed)
 117    5.3%  issue copied from a sibling of the same title
 111    5.0%  corpus confirms the series (colon kept, volume recovered)
  59    2.7%  issue taken from the filename (D61)
  54    2.4%  series casing guessed, corpus silent      (D62, reviewed)
  21    0.9%  bare trailing number in the series (not a volume marker)
  18    0.8%  volume marker swallowed into the subtitle (colon came first)
   1    0.0%  multiple colons
```

**"Clean" is an upper bound.** It means no heuristic fired, not that a human
agreed.

**The bare-trailing-number class fell from 197 to 21**, which is the issue
column's whole point: 59 rows had an issue number the filename vouched for,
and 117 more are other files of those same titles. Before that propagation
(D61 as amended) one comic landed in two Komga series folders —
`Hacktivist/Hacktivist 0001.cbz` beside `Hacktivist 1/Hacktivist 1.cbz` — and
nothing could report it, because the paths differ and so do the folder
collision keys.

**A derived issue is not a confirmed one.** `scripts/review-rows.py` keeps
both derived classes in the review pile. With D62's guessed class beside them
the page puts **266 rows** in front of a human, every one arriving with a
better proposal than before. One of the 59 is wrong in a way only a reader catches:
`"Shrine of the Monkey God!" in Kramers Ergot 9` is one story inside the ninth
book of a numbered anthology, and the rule makes the quoted story the series.
Routing derived rows out of that pile would have hidden it.

**The colon class was 129 rows and is now 18, because `seed` asks the corpus.**
Where a verified title ends in a volume marker and its stem carries a colon,
the generator looks the stem up: `Attack on Titan: Before the Fall` is one of
seven Attack on Titan series GCD records, with 17 issues, so the colon belongs
to the name and `Vol. 9` is the volume — not an arc of `Attack on Titan` with
the marker stranded in a subtitle. 111 of 127 such titles confirm;
`docs/research/2026-10-25-the-corpus-can-confirm-a-colon.md` measures it and
names the 16 that do not. Three things that lookup deliberately does not do,
each of which cost something to find:

- **It never consults `issue_count`.** Four `Battle Angel Alita: Last Order`
  rows hold volumes 16–19 of a run GCD records at 15. The corpus confirms the
  *series*, never the volume, which is independent support for D44.
- **It compares squashed names on both sides.** GCD's own `name_key` strips
  stopwords (`attackontitanbeforefall`, no "the"), and matching against that
  alone reported this very series as unconfirmed: 68 instead of 111.
- **It never touches the 12 shipped marketplace rows.** Those were confirmed
  title by title, and a keystroke outranks a lookup (D50, D51). `data/decisions.tsv`
  is byte-identical after a regeneration, which is the check that proves it.

A missing corpus is "no guess", not an error (D49): `seed` prints one line and
falls back to the parser for every row. `measure` still requires one.

**Tests:** `cargo test --workspace` runs 616 and ignores 9 (counted 2026-10-30
by `awk`-summing the `test result:` lines, which is the number to quote: 616
passed, 0 failed, 9 ignored; 615 before `scan` read the local layer, 613 before D-a and D-d, 612 before the review of the one-decisions-table
change, 610 before the change itself, 606
before the two fix commits of 2026-10-29 and 2026-10-30, 596 before the casing
rule, 537 before
the issue column, 534 before
the corpus-confirms-a-colon change, 530 before
D58, 510 before a four-seat adversarial review, 500 before the whole-branch
review's fix wave, 409 and 7 before session-scoped undo), with no compiler
warnings. **The 595 this file carried until 2026-10-27 was one short** — it was
summed by hand; the baseline measured mechanically on the same tree is 596.
**The whole suite is 625, and the nine ignored have not been run since
2026-10-26**, when the five volume-gated ones passed — the casing rule touches
none of their code. See "What the volume-gated suite does and does not
protect" above before assuming a change needs them. The nine `#[ignore]`d
tests need real hardware or the real corpus release, and **CI runs all nine**:
five are volume-gated (`COMIC_RENAMER_TEST_VOLUME`; four in `apply_exec.rs`,
one in `cli.rs`) and four are corpus tests (`COMIC_RENAMER_TEST_ZIP`, in
`corpus.rs`). The workflow sweeps `--ignored` over each of those three
binaries wholesale (`verify.yml` lines 67, 71 and 115), so no workflow change is
owed for the split. The seven-test claim was false until the
rebuild's Task 13: the workflow ran the three volume-gated tests, and its own
comment said the four corpus tests were not run. Task 13 added the `rust-corpus`
macOS job, which downloads the pinned release, checks its sha256 **and**
`sizeBytes`, and runs those four. The five volume tests run in the `rust` job
on a RAM disk, because a same-volume move is a rename and the streamed path, the
`placed` state and abort-on-failure are otherwise unreachable. This is read
from the workflow file. The four corpus tests were run locally against the pinned
release at the end of the rebuild (green, 54 s).

**Observed on GitHub at last**, when the rebuild merged to `main` on 2026-10-24:
run 40061195019, all six jobs green, including the first ever execution of
`rust-corpus` ("corpus tests against the real release") and `mappings`
("mapping caps (rows per PR, file size)"). Both had existed only on an unpushed
branch until then. **The green `mappings` job in that run did not check the row
count** — see the PR-only caveat above; it was a push to `main`.

- `COMIC_RENAMER_TEST_ZIP=<gcd-*.zip>` — four tests against the real corpus
  release: install-and-open, coverage rates, and the publisher hint's effect
  on real titles. Run by the `rust-corpus` job.
- `COMIC_RENAMER_TEST_VOLUME=<mount on another device>` — five tests that need
  a genuine second volume. A 1.3 GiB HFS+ RAM disk works:
  `hdiutil attach -nomount ram://2726297` then
  `diskutil eraseVolume "HFS+" CRTEST <dev>`. **Under ~1.1 GiB every run is
  refused by the free-space reserve**, which is how the reserve is specified.

### Two things about running the suite, learned the hard way

**`cargo test --workspace` stops after the first failing test binary.** So a
mutation run that reads its fail list off a plain invocation reads a
*truncated* one — every "and nothing else fails" claim made that way is
unverified. Use `cargo test --workspace --no-fail-fast`. Found 2026-10-30 by a
mutation seat whose first three runs under-reported; the claim in `6b907ab`
that reverting either fix "fails its own test and nothing else" was made the
wrong way and has not been re-checked. "Caught at all" is still sound, because
that only needs the test to fail.

**`nothing_is_lost_when_the_source_is_truncated_inside_the_gate` is not
deterministic under load.** The D58 race test (`undo_authority.rs`) passed 5 of
5 runs idle and failed **2 of 3** when run concurrently with other cargo work,
on pristine code. This file describes it as firing "inside the window by
construction rather than on a timed guess"; that is true of the trigger, not of
the surrounding timing. Pre-existing, nothing to do with the decisions table,
and worth knowing before it costs someone a session on a busy CI runner.

### CI checks neither formatting nor clippy

Both are by decision, and the tree is **not formatted**. Do not describe it as
formatted, and do not assume CI will catch a style or lint regression.

- `rustfmt` is not installed in the maintainer's toolchain: `cargo fmt` fails
  with "'cargo-fmt' is not installed for the toolchain" (checked 2026-10-23), and
  the tree has never been through it.
- `cargo clippy --workspace --all-targets` reports 9 warnings across three
  crates (checked 2026-10-24; it was 10 on 2026-10-23, and the difference is a
  deleted test that held one of the three needless-`clone` warnings): two
  `i64`→`i64` casts and one `u64`→`u64` cast in `fsops.rs` and `apply.rs`;
  "items after a test module" in `apply.rs` and `xtask/src/seed.rs`; two
  needless `clone` calls in `crates/core/tests/mapping.rs`; and a "very complex
  type" in each of `crates/cli/src/map/mod.rs` and
  `crates/core/tests/shipped_mappings.rs`. **Count the lint lines, not
  `grep -c '^warning'`**: that reports 16, because seven of those lines are
  `generated N warnings` summaries.
- `cargo fmt` would reformat files under `crates/core/src/preset/`, which the
  rebuild's briefs forbade touching. Adding a format gate means reformatting the
  whole tree in the same change, or CI goes red on arrival.

The compiler itself is warning-free: `cargo test --workspace` prints none.

### Packaging, all of it measured

**Nothing the binary needs is read from the working directory.** Both the
corpus manifest and `data/overrides.toml` are `include_str!`-embedded. The
overrides were not, and `Overrides::load(Path::new("data/overrides.toml"))`
resolved against the cwd — so an installed `comic-renamer` run from anywhere
but the repository root had no Layer B at all, which D27 says violates
anti-goal §4 by construction. Same binary, same input, repo root versus
`/tmp`: `The Walking Dead - Compendium One.pdf` versus `failed`. A
user-supplied overrides file stays Phase 3, when §6.11's caps on contributed
rule data exist to make one safe.

**The build needs a C toolchain, and the README did not say so.**
`rusqlite`'s `bundled` feature compiles SQLite from the amalgamation and
`ring` compiles C too, so both have build scripts that need `cc` — rustup
alone is not enough, and `xcode-select --install` is now a documented
prerequisite. It went unnoticed because the development machine has full
Xcode. Direct evidence: the Linux cross-check failed in exactly those two
build scripts.

One packaging claim measured rather than assumed, because the README now
makes it: **an older toolchain does not fail on `rust-version`.** On 1.84.1,
`cargo check` stops at `block-buffer-0.12.1` needing the `edition2024` Cargo
feature, long before it reads this workspace's `rust-version` — so the failure
reads as a broken dependency, not an old compiler. A
`rust-toolchain.toml` would fix it by forcing a specific download on everyone,
including people already on a newer stable, so the README says it in a
sentence instead.

Also unproven, and left that way: whether the crate fails to build on Linux.
`cargo check --target x86_64-unknown-linux-gnu` stops in `libsqlite3-sys` and
`ring` build scripts for want of a cross C toolchain, never reaching
`fsops.rs:538`'s `std::os::macos::fs::MetadataExt`. The macOS-only claim rests
on that import and on `libc::fclonefileat`, both Apple-gated, not on a build.

**The real toolchain floor is 1.88, and `rust-version` said 1.85.** Measured
2026-10-23 with `cargo metadata`: six locked dependencies declare 1.88 — `darling`
0.24.1 and its two macro crates, `instability` 0.3.14 (pulled in by `ratatui`),
`sha2` 0.11.1 and `zopfli` 0.8.4. The last two were in the lockfile before the
rebuild, so the declaration was already wrong before the TUI landed. This is the
same shape as the 1.84.1 failure above, and it is now the third time this repo
has declared a floor it never built against (1.80 → 1.85 → 1.88). `Cargo.toml`
says 1.88. The measurement shows which dependencies *declare* 1.88; it does not
show that 1.88 builds the workspace, which was not tried on a 1.88 toolchain.

Two environment notes: `cargo` lives in `~/.cargo/bin` and is not on a
non-interactive shell's PATH, and clippy is installed (see above for what it says).

Install is `cargo install --path crates/cli --locked` — note `crates/cli`,
not `.`, since the workspace root is not a package. Verified from three
directories after installing, and `cargo uninstall comic-renamer` round-trips.

The Phase 1 plan survived three adversarial reviews:
`docs/plans/2026-10-20-phase-1-implementation.md`, 15 tasks. Its Task 14 is
superseded by the rebuild plan; its Task 15 is not.

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

### Pick up here

**The tool was driven end to end for the first time on 2026-10-29/30, including
`map`.** Three commits came out of it and all three are pushed and green. What
that session established, and the two decided-but-unwritten items it left, are
in "The first real test run" immediately below. Read that before the older
entries; it supersedes nothing but it is the newest state.

**MVP first. The owner's call, 2026-10-27: Homebrew is a launch item, not an
MVP item.** The release workflow and the formula are done and tested, so that
work is parked rather than unfinished — "Task 15, Homebrew" below holds the
owner steps and every measurement, so none of it has to be re-derived. **The
tap repo now exists**: `bsns-casual/homebrew-tap`, public and empty, created
2026-10-30. The repository itself is **still private**, held deliberately — see
the licence entry.

### The first real test run, 2026-10-29/30

Nothing here was read from the code; all of it was run.

**The installed binary was six days and three feature waves stale** — Oct 22,
from `sites/temp-parser-test/comic-renamer/comic-renamer/crates/cli`, with no
`map` and no menu. Check `comic-renamer --help` against the command list before
trusting any manual test. `cargo install --path crates/cli --locked` takes ~24s.

**The whole pipeline works.** A 10-file bundle built from the 12 shipped
marketplace rows: `scan` 10/10 mapped, `apply --yes` 10 placed, `undo` 10
restored, library empty of files *and* series directories afterwards, record
swept, lock retained. All ten Komga paths correct — `Vol. 06` padded to two,
`0001` to four with no `#`, `/` rendered ` - `.

**`map` works too**, on 12 files from one real Humble bundle. The approval
table, the save to `<config>/mappings.tsv`, the contribution prep, the plan, the
disclosure's "Not ready, so left where they are: 1 of 12 rows", typed `MOVE`,
11 placed. It also ran *non-interactively* on a later folder where every file
was already mapped, confirming the narrower claim this file already makes about
when it needs a terminal.

**Three commits of fixes came out of it**, all pushed to `main`, all green:

- `6b907ab` — `scan`'s guess line joined every candidate's series with no
  dedup, so `popeyevolume1_oliveoylandhersweety` printed `Popeye` twenty times
  and `Pop-Eye` once: 230 characters saying one thing. Collapsed at the display
  in `series_line`, **not** in the resolver, because `map`'s picker renders
  volume/issue/subtitle too and `table::run_length` reads a `volume_count` off
  the same list. Same commit: `apply` exited **0** over a plan whose every file
  was unmapped, because the branch keyed on `rows.is_empty()` alone. It now also
  requires `facts.not_ready == 0`, leaving exit 0 to the genuinely empty plan
  `scan` writes for an empty folder. That is *not* D-a.
- `41a385b` — **the contribution channel produced pull requests that were red
  before anyone read them.** The printed commands copied the rows to
  `data/mappings/map-<bundle>-<secs>.tsv`; following them literally fails
  `every_tsv_in_data_mappings_is_embedded`, and `check-mappings` stays green so
  the `mappings` job would not have caught it either. The commands now *append*
  to the marketplace's own file, which needs no code change in the PR because
  `include_str!` picks the rows up at build time — verified by appending the
  real 11 rows to `fanatical.tsv`: 610 passed, 0 failed. A marketplace with no
  shipped file now prints no commands, because none could work; that is Humble,
  `map`'s **default**, and the channel opens by itself the day `humble.tsv`
  lands. `SHIPPED_MARKETPLACES` and `file_slug` in `mapping.rs` were the
  machinery. **Read this bullet as history, not state: D63/D66 deleted most
  of it the next day.** `SHIPPED_MARKETPLACES` is gone, the per-marketplace
  append became one unconditional target, the "not open for this
  marketplace" branch and its test are gone, and
  `every_tsv_in_data_mappings_is_embedded` is now
  `the_decisions_file_is_the_one_that_ships`. `file_slug` survives with one
  caller, `branch_for`.
- `0b04a00` — **`scan` names `map`, and the menu expands `~`** (D-c). Both were
  listed here as open until 2026-10-30, when running the released binary showed
  they were not: `scan` ends with `N files still need a name. To give them one:`
  and the whole `map` command, both roots shell-quoted from the plan header
  through `map::quote`, now `pub(crate)` rather than reimplemented. **At 100%
  unmapped `apply` is no longer offered at all**, which was the harm — it
  routed the user to a command that correctly places nothing and exits 1. The
  mixed case still prints both lines. `ask_path` expands a leading `~` against
  `$HOME`, after stripping quotes, so `"~/Comics"` expands too; `~user`, a
  tilde anywhere but the front, and `~` with no `$HOME` are left alone. Both
  halves are mutation-checked, and both were re-run against the installed
  0.1.0 binary on 2026-10-30.

**CI run 40976839595 is green on all six jobs**, `rust-corpus` and the
volume-gated job among them — so **the full 619 have all been executed** on this
work, not just the 610 that run locally. The nine ignored last ran 2026-10-26
before this.

#### Three things that looked like tool bugs and were keystrokes

Worth the pattern, not the particulars: **probe the resolver before reporting a
stored value as wrong.** All three were found by feeding the stem through
`guesses_for` in a throwaway `#[ignore]`d test against the real corpus cache.

- `21`'s subtitle stored as `The Story of Roberto Clement`. The resolver returns
  `sub="thestoryofrobertoclementepb"` — the raw unsplit stem, D15's
  `is_unsplit`, which the picker correctly flags. Deleting `pb` took the `e`.
  The corpus spells `Clemente` right.
- `Prince Valiant Vol. 1` stored with subtitle `1937-1939`. The resolver returns
  **no candidates at all** and no corpus record carries those years. Typed.
- `BTTM FDRS`. One candidate, `Btmmfdrs`, from the filename. **`scan` and `map`
  do not disagree about it** — an earlier claim in this session that they did was
  wrong. The correct title was typed.

Both wrong rows were corrected in the owner's `<config>/mappings.tsv` on
2026-10-30 and verified through `map`, which now plans
`21/21 - The Story of Roberto Clemente.cbz` and
`Prince Valiant/Prince Valiant Vol. 01 - 1937-1938.cbz`.

#### `scan` reads the user's own mappings — fixed 2026-10-30

It passed `None` for the local layer behind a `shortcut:` saying the file
"arrives with the `map` command". **Measured rather than inferred**: the same
two files in the same folder were "2 unmapped" to `scan` and "every file is
already mapped" to `map`, so a title approved in a session was invisible to the
non-interactive path — the one a re-run or a script uses. `scan` now loads the
same pair `map`'s `scan_once` does, and a missing file is `None` rather than an
error. Pinned by `scan_reads_the_users_own_mappings`, the sole guard.

#### One decision taken, not yet implemented

- **Duplicate candidates: dedup upstream, highest `volume_count` wins** (owner,
  2026-10-30). Confirmed on two stems: Popeye returns 21 candidates that
  collapse to 2 distinct series, Willie & Joe returns 2 identical in *every*
  field the picker renders. So the `series_line` fix does nothing for `map` and
  the dedup has to move into the resolver, on every field except `volume_count`.
  Measured first, because it bounds the stakes: the width is `max(2, digits)`,
  so `4` and `65` both give two — **the arbitrary pick is invisible below volume
  100.** Highest was chosen as the only option that cannot under-pad a real long
  run.

**0. The review is decided, exported and committed.** 2,204 of 2,204, now at
`review/humble-decisions.tsv` with its own README and the five not-a-comic rows
beside it — 2,031 rows in the shipped shape since 2026-10-30, down from 2,199
eight-column ones. Validated by embedding it and running the real suite: 613
passed, 0 failed. What remains is the licence question, which gates
moving it into `data/decisions.tsv`. **It is no longer a `git mv`, and a `git mv`
would break every user's load**: the file still has the pre-D64 eight columns,
which `parse_tsv` refuses outright, and its 2,199 rows hold 2,031 distinct keys,
so 158 are repeats that `find_key_collisions` now refuses. Measured 2026-10-30:
zero of those 158 disagree on fields 1-7, so collapsing them discards nothing.
`review/README.md` carries the four real steps. Do not trust an older summary of
this that says "one `include_str!` and an array size" — there is one of each
now, and two data transformations before them.

**The artifact's store is no longer the only copy.** If the review is reopened,
re-dump the collection and re-run `scripts/export-decisions.py`; the export is
sorted by key and bundle, so a regeneration is a readable diff.

**-1. The casing rule is merged, pushed and green.** Three commits went to
`main` on 2026-10-27 — `bf4ac06` (hide skipped), `a8d8b47` (the all-caps class
and confirm-what-is-shown) and `5e5b494` (D62, the corpus casing). **CI run
40454635024 is green on all six jobs**, `rust-corpus` in 1m32s among them, so
the one branch that cannot be unit-tested here has been executed against the
real release. Locally: 606 passing, 9 ignored, `check-mappings` green,
`check-review-page.cjs` green, no compiler warnings, the shipped table
byte-identical after a regeneration. The docs commit `9d097f6` that followed
was still queued when it was last looked at; it changes no code.

The job most worth reading is `rust-corpus`, for the opposite reason to last
session: D62's corpus branches cannot be unit-tested here, because the cache
holds `gcd.sqlite` and not the zip the job downloads. `casing_outcome` is
split out from the lookup precisely so the *decision* is testable without a
corpus — but `corpus_casing`, which reads `Corpus::lookup`, is exercised only
by `cargo xtask seed` against the local cache.

**CI run 40283202212 for the issue column came back green on all six jobs**,
including `rust-corpus` in 1m47s, which was the open question at the previous
close. The only annotations are advisory: `actions/checkout@v4` targets the
deprecated Node 20 and is being forced onto Node 24, and `ubuntu-latest`
migrates to Ubuntu 26 from 2026-10-19. Neither fails anything yet.

**The issue column before it** is 19 commits, `747b080..84a0439`, pushed
2026-10-26. `cargo xtask seed` leaves the shipped table byte-identical except
the four rows that legitimately improved (`Hammer Head`, `Minotaur`,
`DIE: Loaded`, `Glarien: Origins` each carried `#1` as subtitle text).

**D62 leaves one gap, and it is the owner's to close.** The Humble path has no
equivalent of the fifth column in `fixtures/marketplace-ground-truth.tsv`, so a
ruling made on the review page does not reach the generator. Four titles are
already in that position — the owner typed `BLAME!`, `DIE`, `CURSE WORDS` and
`ODY-C`, and the corpus proposes `Blame!`, `Die`, `Curse Words`, `Ody-C`. The
stored decisions win today, because the page fills a decided row from the
store and the new proposal reaches it only through `↺`. A later `DIE` volume
in a new bundle would arrive proposing `Die` again.

**The "two likely typos" this file used to record here are not typos.** Checked
2026-10-30: neither `Exquiste Corpses` nor `Front Knife` exists anywhere in
`review/`, `data/` or `fixtures/`. The committed export has `Exquisite Corpses`
(rows 714–715) and `First Knife` (row 766), both spelled correctly, against
Humble's own `EXQUISITE CORPSES, VOL. 1` and `FIRST KNIFE` — so they are D62
casing decisions behaving correctly. The note described the artifact store
before the export, or it was never right. Do not go looking for them.

The licence gate was checked before pushing and is **not** crossed: the
shipped table carries only Fanatical, Global Comix and Sweet Shop bundles,
`target/seed/humble.tsv` is gitignored, and no Humble row reached
`data/mappings/`. Individual Humble titles appear in tests and docs as before,
which D31 already sanctions as test data.

One thing to carry forward about the review that produced those commits:
**four seats found nine real defects and three of them were in my own new
tests.** A test that passes for the wrong reason is the recurring shape here —
`!refused.is_empty()` satisfied by an unrelated refusal, a non-digit typed on
an empty field where the parse fails either way, a helper that did not assert
it had broken what it claimed to break. Run the mutation, do not reason about
it.

**0. Two data-safety findings, closed, merged and green.**
`docs/research/2026-10-23-two-pre-existing-data-safety-findings.md` describes
both. The first was a data-loss path: a hand-written manifest beside a forged
plan made `undo` delete a library file no run placed. The second needed no
adversary: a second terminal running `scan` substituted a plan under a
confirmation already given. Session-scoped undo addresses them with invariant 11
(D54, D58) and by reading the plan once and executing from that copy. Merged to
`main` on 2026-10-25 at `aad24f6`, and **CI run 40163809503 is green on all six
jobs** — the first time every job has run on this work.

One note on how it got there, because the shape recurs: the merge went out with
the volume job red. `cargo test --workspace` reported 534 then and **skips the
nine `#[ignore]`d tests**, so "534 passed" was never the whole suite, and one of those
nine asserted the pre-D58 truth that a reversed copy leaves the library empty.
Four visible tests were updated when D58 landed; the invisible one was not,
because that round's brief did not require the RAM-disk run. **Quote the gated
total, or run the volume suite** — 606 of 615 as of 2026-10-27, and the nine
ignored last ran on 2026-10-26.

The first fix then needed a second one. An adversarial seat showed D54's own
gate was check-then-act on the source and lost the file outright when raced —
`1 restored · 0 refused`, exit 0, a 400 MB payload in neither place, on the
first attempt. D58 replaced the unlink with a rename into the aside directory,
which is why invariant 11 now reads the way it does. The race is reproduced
deterministically in
`nothing_is_lost_when_the_source_is_truncated_inside_the_gate`
(`crates/core/tests/undo_authority.rs`): it triggers off the *destination's*
atime, which nothing before the gate touches, so the writer fires inside the
window by construction rather than on a timed guess. The
design's §7 left one question open that was deliberately not planned: sweeping
records written by an install that predates the sweep. No release has shipped,
so none exists.

### Three cuts the issue column deferred, 2026-10-26

Each was raised by a review seat, graded, and left undone on purpose. None has
a user-visible effect; all three are signature or naming work.

**P-a. `record_pending` takes eleven positional arguments and should take
`&Resolved`.** `apply.rs` builds one a line earlier and `undo.rs` reassembles
one at the rebuild, so four parameters and the
`#[allow(clippy::too_many_arguments)]` would go. The ponytail seat argued the
deferral was wrong because this branch "paid twice" — `PAD`, `COLUMNS`, the
roots-row literal and `pending_fields`' indices all shifted for one field. That
is the **CSV's positional format**, not the signature, and `&Resolved` would
not change it. Worth its own commit; 22 call sites on a destructive path at the
end of a long branch was the wrong moment.

**P-b. `show` takes four positionals at seven call sites** and should take
`&ParsedTitle`. Declined because five of the seven would then convert a
`Suggestion` through `as_parsed`, cloning two Strings per row per render frame
in a TUI, to save six lines.

**P-c. `pad_issue` is a one-line `format!` with one caller.** Declined: its
doc comment carries D60's "why four digits, why no `#`", which is this repo's
house style for a decided thing.

### Four decisions waiting on the owner — two of them now settled

None blocks development and none blocks a release. Each was ruled on during the
session-scoped-undo work and deliberately left for the owner, with the reasoning
recorded here so it does not have to be re-derived. **D-b and D-c were answered
on 2026-10-30; D-d needs more context, written below; D-a is reframed.**

**D-a. Settled 2026-10-30 and implemented: exit non-zero only when a skip is
not "already in the library".** `ApplyResult` carries `unresolvable`, counted
from the collisions whose `existing` is `None`, and `run_apply` returns an
error when it is non-zero.

**The rule applies at two sites, and the second was found by running the
binary rather than the tests.** D-a's entry described the post-apply path, and
fixing only that left the *pre-prompt* path (`facts.files == 0`) erroring
whenever any row existed, regardless of kind — so **re-running an
already-migrated bundle exited 1**, which is the single most ordinary second
run there is and the exact case D-a says is zero. `Facts` carries
`unresolvable` now and both paths read the same rule. The three cases,
measured through the installed binary:

```
places 1 of 1                 exit 0
all rows already in library   exit 0     <- was 1
every file unmapped           exit 1
```

**Two existing tests asserted the old contract and were updated, not deleted.**
`applying_a_copy_twice_refuses_and_leaves_the_destination_alone` asserted
non-zero for the idempotent re-run — every other assertion in it stands, only
the exit changed. `a_run_that_places_nothing_does_not_offer_an_undo` used to
say "the exit code is zero here, and this test does not assert otherwise"; it
asserts now, and a mutation treating already-in-library as unresolvable fails
it and nothing else.

The partial run is the case that was genuinely reported as success — places 400
of 401, abandons one to a collision no re-run can fix. Pinned by
`a_partial_run_that_abandons_a_row_to_a_human_exits_non_zero`, the sole guard.

The original entry follows, for the reasoning.

**D-a (original). `apply`'s exit code for a consented run that places nothing.** A run that
*can* place nothing is caught before the prompt by `facts.files == 0`, skips the
mode block, and exits non-zero — and since `6b907ab` that is true of the rowless
plan too. A run that passes the typed confirmation and *then* places nothing,
because every row collided at execution, exits **zero**.

**Reframed 2026-10-30, after the owner asked why zero is wrong here: for one of
the three collision kinds it is not wrong, and that is the useful answer.**
`Facts::gather`'s `describe` has three arms:

- `existing: Some(_)` — "is already in the library". §6.10 calls this the common
  case, not the rare one. Every row skipped for this reason means the file is
  *already where the plan wants it*, so the library **does** match the plan and
  the run is an idempotent no-op. **Exit 0 is correct**, and this file's stock
  justification ("a caller cannot tell 'the library matches the plan' from 'it
  does not'") simply does not apply: it can, and it does match.
- `row_ids.len() > 1` — "N rows want one name". Unresolvable without a human;
  one of those rows will never be placed.
- the remaining arm — "cannot be placed".

The latter two are genuine failures. So the decision is not "0 or 1" but
**whether `apply` distinguishes the kinds**, which today it cannot: it exits 0
over all three. The narrow change is to exit non-zero only when some skip was
*not* `existing: Some(_)`. Still the owner's call, because it is a user-visible
change to the one contract automation depends on.

**D-b. Settled 2026-10-30: `reversible` keeps counting what the record
recorded.** No code change. The known consequence stands and is accepted: a
partial reverse over-reports, because the number describes the record rather
than the library. The adjacent trap is unchanged and still load-bearing — the
count must stay derived from `expected()`, never parallel to it, or a crash in
move mode's one-syscall window produces a record every automatic path reads as
empty and the next run deletes.

**D-c. Settled 2026-10-30 and implemented: `ask_path` always expands `~`.**
In `0b04a00`, as `expand_tilde` behind `ask_path`. The reason the gap
mattered: a shell expands `~` before `scan` ever sees it, but a menu
prompt does not, and the menu is the entry point for people who do not use
flags — which is exactly the population that types `~/Comics`.

**D-d. Settled 2026-10-30, option 3, and implemented: the menu reports every
interrupted run.** The retention policy is untouched — D57's exception still
keeps each interrupted record that has rows to put back — and the screen stops
under-reporting them. `r` and `l` still act on the newest only, so this is
wording rather than behaviour, which is why it was the cheap option. The pin is
`the_menu_says_how_many_interrupted_runs_are_waiting`, which also asserts `r`
still picks the newest.

The entry below is the reasoning, including the two options not taken.

**D-d (original). D57's "one record per pair" wording.** The sweep now exempts a record that
is `interrupted && reversible > 0`, so retention is one finished record **plus one
per un-reversed partial run**, with no constant cap. Measured: three
interrupted-with-rows records for one pair gives "swept 1; records left 4", and
`first_interrupted` surfaces only the newest, so the rest are reachable only
through `undo --list`. The behaviour is defensible and documented as a D57
exception; the entry's own summary sentence no longer describes it.

**More context, written 2026-10-30 at the owner's request.** The whole policy is
`record::supersede`, and it is nine lines. For every record in the store other
than this run's own, in order:

```
skip if it is this run's record            (keep.file_name())
skip if it will not summarise              (a stray file cannot abort a sweep)
skip if either root differs                (another bundle, or another library)
skip if interrupted && reversible > 0      (the D57 exception — the menu's `l` promises it stays)
skip if another process holds it           (is_live)
otherwise delete it
```

Called from `apply` only when this run actually created a record, and only under
the library lock the caller already holds. Both halves are load-bearing and the
comment there says why: `supersede` deletes every record for the pair *other
than the one named*, so calling it after a run that recorded nothing names
nothing and takes the lot — run `apply` twice, collide on every row the second
time, and 400 placed files stop being reversible. The lock is what makes
`is_live` followed by `remove_file` something other than check-then-act on that
exact file.

**What is actually being decided.** Only the fourth line. Keep it and retention
is unbounded in one specific way: N interrupted runs over one bundle leave N
records, and `first_interrupted` offers only the newest, so the older ones are
reachable only by someone who thinks to run `undo --list`. Drop it and the menu's
`l` key breaks its promise — `l` says "leave this record alone" and the next
`apply` on that bundle would delete it. Three options, none of them obviously
right, which is why it is here:

- **Keep as is.** Unbounded, but every retained record has rows that can still
  be put back. Nothing is ever lost; the cost is clutter and a menu that
  under-reports what is there.
- **Cap it** at, say, the newest three interrupted records per pair, sweeping
  older ones. Bounds the store, and silently discards a way back for the oldest.
- **Surface them instead of capping** — have the menu say "3 interrupted runs
  for this bundle" rather than offering only the newest. Keeps every record,
  fixes the under-reporting, costs a line of menu wording and nothing else.

The third is the one that makes the current behaviour honest rather than
changing it, and it is the cheapest. The remaining wording fix either way: D57's
own summary sentence should say "one finished record per pair, plus any
interrupted run that still has rows to put back".

### Owner's calls on the backlog, 2026-10-25

- **The Homebrew formula is a release blocker, and the code side is now done.**
  It is for people who do not want to run `cargo`, so
  `cargo install --path crates/cli --locked` working is not a substitute, and
  **no release ships without it.** As of 2026-10-27 the release workflow exists
  and the formula has been installed and audited locally; what remains is three
  owner steps (public repo, a tag, a tap repo named `homebrew-tap`). Deferred
  behind the MVP by the owner on 2026-10-27. See "Task 15, Homebrew" below.
- **Kavita is dropped until a user asks.** The research in
  `docs/research/2026-10-23-kavita-from-source.md` stands and the finding is real
  — 890 of 2,223 rows would become one-book series — but nobody is waiting on it.
  Do not spend Phase 2 effort here unprompted.
- **The fault-injection facility is not worth building as a facility.** See the
  entry below.

### The three reasoned-not-tested claims, and why one facility is the wrong shape

An implementer proposed that a single fault-injection seam — fail one write
inside `perform` after the destination exists — would close three `shortcut:`
claims at once: the `mechanism` column's one read-path state, `perform`'s
ordering shortcut, and "`placed` survives a failed unlink". That framing is
optimistic, and the branch that proposed it also produced the techniques that
make a facility unnecessary.

- **"`placed` survives a failed unlink" needs no injection.** `chmod` the source
  parent to `0500` after the clone and `unlinkat` fails with `EACCES`. Existing
  machinery, one fixture.
- **The ordering claim — verify before unlink — is a perturbation**, which
  `a_destination_that_changes_during_the_stream_back_is_not_unlinked` already
  does. Extending it is cheaper than a seam.
- **Only the interrupted clone genuinely needs process control**, and D58's race
  test shows how: trigger off a filesystem property nothing else touches — it
  uses the destination's atime — so the kill lands inside the window by
  construction rather than on a timed guess. Subprocess plus trigger, no
  production-code seam.

A `#[cfg(test)]` hook inside `perform` would put a test seam in the one function
whose ordering is load-bearing, which is the opposite of what this project does
elsewhere. **Do these three individually, when next touching `perform`, with the
techniques above.** None guards a current bug; all three guard against a future
regression, so none is urgent.

Then, two things that gate a release but block no development.

**1. The licence for publishing Humble-derived titles. Deferred by the owner on
2026-10-25** — saved for later, not dropped. It still gates publishing the table
and blocks no development.

**Direction given 2026-10-30: "whatever suits our needs most while staying open
source."** That is a direction, not yet a resolution, and two things found while
acting on it are worth recording because they narrow the question:

- **GCD-derived fields are already mixed into the Humble rows**, so the share-
  alike question is not separable. 128 rows had their series casing settled by
  the corpus (D62) and 111 had a colon confirmed by it — 239 of 2,223 rows carry
  a field the corpus decided. `data/LICENSE` ties rule data to GCD under
  CC BY-SA 4.0, and BY-SA propagates. So **keeping all of `data/` under
  CC BY-SA 4.0 with attribution to GCD is the answer that needs no new
  decision**; a looser licence for the Humble rows is not available without
  stripping the corpus's contribution back out. Code stays MIT, which the
  `licence` CI job already checks.
- **A licence cannot grant rights nobody has**, and the open part was never
  really "which licence" — it is whether redistributing a 2,199-row compilation
  pulled from Humble's order API is permitted. That is Humble's terms of
  service, not copyright: individual titles are short factual strings and US law
  gives thin protection to factual compilations, while the EU has a separate
  database right. Nobody here is qualified to settle that and it should not be
  guessed at in this file.

**The repository went public on 2026-10-30, and the Humble table was then
excised from history at the owner's instruction.** Sequence, because the order
is the point:

1. Public at 05:15, with the decided table in HEAD and in history.
2. Four minutes later, 0 forks, 0 watchers, 0 stars, and clone traffic of
   327/2 uniques — which is CI checkouts plus this machine, not third parties.
3. Made **private** for the rewrite, because a force-push on a public repo
   leaves the old objects fetchable by SHA until GitHub garbage-collects, so
   the window would otherwise have stayed open.
4. `git filter-branch` over all 220 commits removing
   `review/humble-decisions.tsv` and its sidecar, force-pushed, re-tagged,
   public again.

**Every commit SHA changed.** CLAUDE.md's twelve SHA references were
mechanically translated by matching commit subjects old-to-new (220 of 220
mapped unambiguously) and each one verified to resolve on `main`. **Commit
messages were not and cannot be** — a message citing another commit by SHA now
cites a commit that does not exist, and fixing that would require a second
rewrite that invalidated the SHAs again. Treat any SHA inside a commit message
dated 2026-10-30 or earlier as pre-rewrite.

What this does and does not achieve: the table is out of this repository and
out of its history. It was publicly readable for roughly four minutes with no
evidence of outside access, and `fixtures/humble-ground-truth.tsv` **still
holds 2,223 Humble titles** and is staying — `cargo xtask measure` and three
test binaries read it, and D31 sanctions filenames and titles as test data.
So "no Humble data is public" was never the outcome on offer; "the decided
2,031-row table is not published" is.

My earlier recommendation of "public without the Humble table" was made without
checking history and was not executable as stated. Recorded so nobody repeats
it.

**2. The Humble table review is complete: 2,204 of 2,204 decided**, counted
from the artifact's store on 2026-10-27 with 0 orphans, 0 malformed rows and 5
marked not-a-comic. 266 of them went through the review pile one at a time; the
remaining 1,784 were approved as proposed in one batch, at the owner's
instruction, after the corpus had settled the casing (D62).

**It is no longer in the repo**, and not in its history either — removed
2026-10-30, see the entry above. It was 2,031 rows in the shipped shape (2,199
before the D64 transformations), 680 series, 205 KB. `review/README.md` records
what it was, where it can be regenerated from, and the two transformations a
regenerated export still needs. `scripts/export-decisions.py`
made it from a dump of the artifact's `decisions` collection; it judges nothing
and never recomputes a key, because the frozen key rule has one implementation
and `review-rows.py` already had to delete a Python copy of it.

**It was validated through the real gates**, on 2026-10-27, by copying it to
`data/mappings/humble.tsv` and adding it to `SHIPPED_FILES`: 606 passed, 0
failed, covering `MappingRow::validate` over every row, `find_key_collisions`
over the joined table, and the frozen key pin. Both changes were reverted.

**That validation predates D63 and so does the recipe it implied. Do not
follow it.** `data/mappings/` is gone and gitignored, and **`SHIPPED_FILES`
does not exist in any code** — one `const SHIPPED` inside `shipped_mappings()`
replaced it, so there is no array and no size to bump.

**The two data transformations are done, 2026-10-30.** The file is 2,031 rows
in the shipped seven-column shape: `bundle` dropped, 168 repeated keys
collapsed, **zero disagreements across all seven fields** — D64's load-bearing
measurement, confirmed on the real decided table. Shipping it is now
`tail -n +2 review/humble-decisions.tsv >> data/decisions.tsv` and a `git rm`.
Checked by doing exactly that and reverting: the joined 2,043-row table gives
613 passed, 0 failed, at 132 KB against a 4 MiB cap, with zero key overlap
against the 12 rows already shipped.
**`review/README.md` is the accurate copy of this** — an earlier version of
this file said it "still describes the old shape", which was false and taught
agents to distrust the one correct document.

**`review/` is deliberately not `data/`.** A file under `data/` is
`include_str!`-embedded, which is an act of shipping, and
`the_decisions_file_is_the_one_that_ships` fails on arrival to say so — **at
the top level only.** A `.tsv` in a *subdirectory* of `data/` is seen by
neither that test nor `cargo xtask check-mappings`: an adversarial seat planted
this very 205 KB file at `data/mappings/humble.tsv` on 2026-10-30 and both
gates stayed green, with a positive control at `data/stray.tsv` failing
correctly. `.gitignore` now refuses to track such a file, and the walkers are
being made recursive. Until they are, that is the hole, and it sits exactly
where the licence question lives.

Three things the finished review established, each measured rather than assumed:

- **Every key agrees with itself.** 0 keys whose rows disagree on a field,
  which is exactly what `find_key_collisions` refuses, so the table would
  survive a contribution as it stands.
- **No series is spelled two ways.** Eight were, across 58 rows, and all eight
  were normalised to the corpus's spelling on 2026-10-27 — including a bare
  `Attack on Titan:` with a trailing colon on 20 rows, and four spellings of
  `B.P.R.D. Hell on Earth`. Each pair would have produced two Komga folders.
  The genuine sub-series keep their colons, as the corpus records them.
- **Seven rows were approved against the generator and with the owner.**
  `East Of West` and `W0rldtr33` were proposed, but the owner had already typed
  `East of West` and `w0rldtr33` for other rows of the same series, so his
  spelling was applied to the rest. A keystroke outranks a lookup (D50, D51),
  and that holds for the owner's own earlier keystroke too.

**`map` is not the tool for it, and the owner chose a page instead.** `map`'s
`Entry` holds a filename and the resolver's candidates and never reads
`fixtures/humble-ground-truth.tsv`, so a `map` session would ask for 2,204
titles to be re-decided from 48.9%-accurate guesses while discarding the
verified titles `seed` already built the rows from. It is also 38 sessions,
one per bundle, because a contribution carries exactly one bundle name.

The review runs on **`scripts/review-page.html`**, published privately as an
artifact. Each card shows the filename, its bundle, the verified Humble title,
three prefilled fields and the Komga path they produce. Decisions live in the
artifact's own store, one document per 100 rows, and `scripts/review-rows.py`
builds the page's data from `cargo xtask seed`'s output. To regenerate and
republish, run `cargo xtask seed`, then `review-rows.py`, then publish
`scripts/review-page.html` **with the existing artifact's URL** — a different
file path would make a new artifact and orphan the decisions.

Three things that page learned the hard way, all fixed:

- **Writes merge, never replace.** It rebuilt each 100-row block from its own
  mirror and wrote it whole, so one failed read at load plus one Confirm
  destroyed the other 99 decisions.
- **A decision's identity is checked, not assumed.** The store is keyed on a
  row's ordinal in `seed`'s output; each decision also carries its row's key
  and bundle, and a mismatch is reported as an orphan rather than painted over
  the wrong file.
- **`scripts/check-review-page.cjs`** pins its path renderer against
  `komga.rs` with 13 cases copied from `preset_komga.rs`. The page mirrors the
  preset in JavaScript, which is not an invariant 1 violation — it constructs
  no path the tool acts on — but an unpinned mirror that drifts shows the
  wrong path to someone making 2,204 decisions.

Two controls changed on 2026-10-27, both from using it:

- **Hide skipped is its own checkbox.** "Hide confirmed" used to hide skipped
  rows too, and only on the per-row paint: `sweepHidden` tested
  `classList.contains("done")` and a skipped row carries `skipped`, so the row
  vanished on a repaint and came back on the next sweep — which `renderMore`
  runs for every batch. Both paths share one `hidden(node)` predicate now.
- **The scan section's bulk confirm acts on the rendered slice**, not the whole
  stack, so one press can no longer confirm two thousand rows nobody has
  scrolled past. `Show more` disarms it, because growing the shown set
  invalidates the count the press was agreed against, and the button stays
  after a batch instead of hiding.

**Invariant 7's bundle half matters here**: 158 of the table's 2,036 keys
appear under more than one bundle, so this is the table where a literal bundle
comparison would have started costing titles.

Still measured and **deliberately not implemented**: a whole-title corpus
lookup confirms 12 of the bare trailing numbers (`RoboCop 2`,
`B.P.R.D.: 1948`). It changes no field, only the report, and that class is 21
rows now rather than 197, so it would change nothing about the review's shape.

Then, in rough priority:

**Kavita is not a Komga-with-a-different-template.** The research in
`docs/research/2026-10-23-kavita-from-source.md` found Kavita is
filename-driven where Komga is folder-driven, so the Komga layout fragments a
Kavita library: **890 of 2,223 Humble rows (40%) would become one-book series**,
because Kavita's last series pattern is a catch-all and a filename with no volume
marker becomes its own series name. It also withdrew §8's stated reason for
PDF→CBZ conversion ("Kavita files every issue-number-less file as a Special");
that is true only for Book libraries. The backlog entries in the design's §8 and
§9 now say so. Conversion may still be worth doing, but not for that reason, and
writing PDF metadata is the more direct fix for both problems. **All of this is
read from source and was not confirmed against a running Kavita**, which the
research flags itself.

### Task 15, Homebrew: before launch, after the MVP

**The code side is done and tested. Three steps are the owner's, and none of
them is work I can do.** Deferred on 2026-10-27 so the MVP lands first; the
measurements below are recorded so nobody re-derives them.

**Done 2026-10-30: v0.1.0 is released and the tap is populated.** The formula
is at `bsns-casual/homebrew-tap`, `brew audit --strict` and `brew style` are
clean, and `brew install bsns-casual/tap/comic-renamer` was run end to end —
`lipo -archs` reports `x86_64 arm64`, `brew test` passes, and the binary runs
from `/`, which is the `include_str!` claim holding in a packaged build.

**One thing the plan did not anticipate, and users will hit it:** current
Homebrew refuses a third-party tap until it is trusted. Without
`brew trust bsns-casual/tap` the install fails with "Refusing to load formula …
from untrusted tap". The README leads with that line. The `sha256` in the
formula is GitHub's own published asset digest rather than a locally recomputed
hash, so it cannot drift from what brew fetches.

### Removing the `brew trust` line — a future improvement, researched 2026-10-30

The README's install begins with `brew trust --formula …`, and users will ask
why. **It is not a tap misconfiguration and there is nothing to fix in the
formula.** Homebrew 6.0.0 introduced tap trust and applies it to every
non-official tap; this machine is on 7.0.11, so it is settled behaviour.

**The only way to remove the line is `homebrew/core`**, where there is no tap
and no trust: `brew install comic-renamer`. Four requirements, checked against
Homebrew's own docs rather than recalled:

- **Already satisfied.** Open source under a DFSG-compatible licence —
  GPL-3.0-or-later qualifies — and a stable, immutable tag, which `v0.1.0` is.
- **Notability is the blocker, and it is not an engineering problem.** ≥30
  forks, ≥30 watchers or ≥75 stars; **for a self-submission by the repo owner,
  ≥90 / ≥90 / ≥225.** The repo was at 0/0/0 on release day. An automated audit
  check reports this verbatim. (One issue report also mentions a 30-day repo-age
  rule, which does not appear in the policy pages — unverified.)
- **The prebuilt binary has to go, and that objection then evaporates.** Core
  generally refuses binaries: a formula "must either build from source or
  install portable, platform-independent output". The reason this repo ships a
  prebuilt universal2 binary is that `depends_on "rust" => :build` makes a user
  download a toolchain for a 6.9 MB binary — **but core's CI builds from source
  and publishes bottles**, so users still get a prebuilt download. In core the
  formula gets simpler, not worse.
- **macOS-only is arguable, not disqualifying.** Core requires passing CI on
  supported macOS *and* Linux, but the policy allows a platform restriction "if
  upstream doesn't support a platform and the remaining support is useful and
  maintainable". `renamex_np`, `clonefile` and `std::os::macos::fs` are
  Apple-only with no fallback, so that argument is available — it just has to
  be made.

**Do not reach for `HOMEBREW_NO_REQUIRE_TAP_TRUST=1`.** It exists as a
transitional opt-out, Homebrew says it is being removed, and asking users to
export a variable that disables a supply-chain check is worse than one trust
line.

**Narrow trust is already what the README uses.** `brew trust --formula
<tap>/<formula>` rather than `brew trust <tap>`, because whole-tap trust grants
every present *and future* formula in it and Homebrew's docs prefer the narrow
form. If aliases are ever added, note the open bug where a fully-qualified
install by alias is refused and `brew trust` takes the alias literally — trust
has to be granted against the canonical name.

Sources: docs.brew.sh/Tap-Trust, docs.brew.sh/Package-Acceptance-Policy,
docs.brew.sh/Acceptable-Formulae, brew.sh/2026/06/11/homebrew-6.0.0,
github.com/Homebrew/brew/issues/24184.

`.github/workflows/release.yml` fires on a `v*` tag, builds both macOS slices
on one arm64 runner, `lipo`s them into a universal2 binary, attaches the
tarball to the release, and prints the complete formula with its url and
sha256 filled in. `workflow_dispatch` runs everything but the publish.

What the owner has to do, in order:

1. ~~**Make the repo public**~~ — **done 2026-10-30.** A formula fetches a plain
   HTTPS tarball, so this was the blocking step. It also decided the question
   D31 left open — whether Humble titles in the tests and docs should be
   world-readable — in the affirmative, knowingly. See the licence entry above
   for what the owner was told first.
2. **Tag `v0.1.0`.** The workflow refuses a tag that disagrees with
   `crates/cli/Cargo.toml`.
3. ~~**Create `bsns-casual/homebrew-tap`**~~ — **done 2026-10-30**, public and
   empty: `https://github.com/bsns-casual/homebrew-tap`. That exact repo name,
   because brew expands `user/tap/name` to `github.com/user/homebrew-tap` and
   the whole point is that `brew install bsns-casual/tap/comic-renamer` is the
   entire command. Named `homebrew-comic-renamer` the user would type the name
   twice.

So **steps 1 and 2 remain**, and step 1 is held on the licence entry above
rather than on anything technical.

**A prebuilt binary, not a source build, and not a cask.** Each leg measured
2026-10-27:

```
cold release build     20.8s     binary 6.9 MB, tarball 6.3 MB
x86_64 cross-compile    17.0s    on an arm64 host, bundled SQLite and ring included
brew install            1s       from a local tap over a file:// tarball
```

- The compile is nothing; `depends_on "rust" => :build` is the whole cost,
  because brew would download a toolchain to produce 6.9 MB. `cargo install
  --path crates/cli --locked` already serves people who want to build it.
- The cross-compile working on one host is why there is one job and why the
  formula needs no `on_arm`/`on_intel` blocks.
- **A cask would be worse, not better.** `brew install --cask` quarantines, so
  an unsigned binary needs notarization and a paid Apple account. A formula's
  tarball is fetched with curl and is not quarantined.

**The standing warning was honoured, by a different route than it names.** The
formula could not be `--build-from-source`-tested, because it ships a prebuilt
binary and has no source path. It was tested the way such a formula can be:
`brew tap-new bsns-casual/tap`, a `file://` url, a real `brew install`. It
installs in 1s, runs from an unrelated directory — the Layer B bug
`include_str!` closed, re-checked against the installed binary — and
`brew test` passes. **`brew audit --strict` found two real problems on the
first cut**: an explicit `version` is redundant, since brew reads it from the
asset filename, and the line sat in the wrong position. The emitted text now
passes `brew style` and `brew audit --strict` clean. The test install and the
tap were removed again.

Two things to know when picking this up: `gzip` records an mtime, so a rebuilt
tarball hashes differently and will not match a formula already in the tap; and
this machine has a `cargo install`ed binary in `~/.cargo/bin` that brew's
`/opt/homebrew/bin` copy would shadow.

### Known gaps

- **A mapped row is never questioned, and there is no path to correct one.** A
  key is a hash of a filename, so two different books with one filename
  (`batman.cbz`) get one decision — and `lookup` answers confidently for both.
  Nothing cross-checks a mapped row against what the resolver would have
  guessed, `map` shows *unmapped* files only, and nothing prints the path to the
  local file a user would have to hand-edit. **Pre-existing, not introduced by
  D64**: a single-row key has always answered regardless of the folder. What
  D64 removed was an accidental safety net in the two-row state, where `lookup`
  returned `None` and the file came back as unmapped with a guess. Found by an
  adversarial seat on 2026-10-30; the corrected reasoning is in that spec's D64.
  Unreachable today — 12 shipped rows, no duplicate keys — and the fix is a
  feature, not a patch.
- **The corpus download prints nothing while it runs.** `corpus.rs` has no
  `print!` or `eprint!` anywhere, so a first `map` or `scan` over an unmapped
  Humble file stops dead for the length of a 33.7 MB fetch (35,341,426 bytes
  per the manifest, 98 MB unpacked) with no output at all. Measured 2026-10-30
  against a scratch `XDG_CACHE_HOME`: 1.8 s on a fast connection, which is why
  this was never noticed here. The README documents the download accurately at
  its "A `map` or `scan` run with an unmapped Humble file" paragraph — the gap
  is only that nobody has the README open while a command appears to hang, and
  `--marketplace <anything but Humble>` skips it. One line of output fixes it;
  `ensure_corpus` already knows the size from the manifest and returns early
  when the cache is warm, so the message belongs on the miss path only. Found
  while writing a testing guide for an outside tester, not by a test.
- **Only five guesses are ever shown.** `COLLISIONS_SHOWN` is 5 and caps the
  *rows* `suggest_for` prints, so a 2,200-file bundle reports five guesses and
  the rest silently. Deliberate, and it is why the `Popeye` line was the one
  that surfaced the dedup bug.
- **§6.11's caps: §7.4 was amended to match the code, 2026-10-30.** Field
  validation (forbid-list, 120-byte cap) runs where a row enters — on load, in
  `map`, in `seed` — and the row and file-size caps run in CI. `apply` does
  *not* re-run `MappingRow::validate`, and both specs now say why: it recomputes
  every destination from metadata through the preset and never trusts a path
  from the plan (invariant 2), which defends against a tampered plan more
  completely than re-checking fields would. `rendered_budget_ok` is called only
  by tests and cannot fail for Komga, since `sanitize` truncates at 255 bytes;
  kept for Phase 2 presets.
- **`volume_count` is user-supplied** and sets the zero-padding width (D44, which
  replaced D37's corpus lookup). A wrong value produces a wrong *name*, never a
  wrong *location*.
- **The resolver treats `issue_count` as a hard volume bound, and the corpus is
  incomplete.** `corpus_ladder`'s step 3 refuses a series with fewer entries
  than the file's volume number. `Battle Angel Alita: Last Order` is recorded at
  15 issues and the bundle holds volumes 16–19, so four real rows get no
  suggestion. `measure` reports 93 of 2,223 rows (4.2%) as "corpus refused
  (language or volume)" — an upper bound, since that bucket mixes the two
  causes. Splitting it is the cheap next measurement; it costs typing, never
  data, which is why nothing was changed here.
- `located`: *Domestic Girlfriend* and *Altair* were absent from the corpus when
  Task 12 was written — manga romanisation, the gap §6.2.1 gives Layer B — and
  `mirkaandolfosmercy_vol1` is the reverse match direction, where §6.1 specifies
  the opposite. Not re-checked since the rebuild. Making matching bidirectional is
  a spec change with real false-positive risk.
- The free-space reserve is `max(1 GiB, 10%)` taken literally from the plan,
  so **no run is possible on a volume with under ~1.1 GiB free**. Correct per
  spec, harsh in practice.
- `ApplyResult.failed` and `Collision.existing` **are both read now** (T13).
  One caveat survives: `apply` abandons the run on the first row that fails, so
  `ApplyResult.failed` is still structurally always 0 — it is read but it cannot
  yet be non-zero.

Phase 0b — the 13-app research spike — runs *parallel* to Phase 1 and gates
Phase 2 only. Nothing in Phase 1 waits on it. Its Kavita result is in, per above.

Volume numbers are **zero-padded to a per-series width** (D37) — the digit count
of that series' highest volume, floored at two. Since the rebuild the width is
stored on each mapping row as `volume_count` (D44), because deriving it from the
rows in a growing table would rename files already placed. Komga's comparator is
numeric and does not need the padding, but `ls`, Finder and most scripts sort
lexically, where `Vol. 10` would precede `Vol. 2`. A fixed two-digit width breaks
silently at volume 100 and manga runs pass it.

## Conventions

- **Commit and push directly to `main`.** The maintainer's standing instruction;
  no PR needed for ordinary work. Exceptions are called out explicitly.
  Automation is the one thing that must not push to `main` — `corpus-refresh`
  opens a pull request instead, because a corpus bump needs a human.
- A `corpus/manifest.json` bump **requires the coverage delta in the PR body** —
  it changes resolution behaviour for every user. (§6.5)
- Don't claim something works without running it. This project has twice been
  wrong about a packaging claim that failed on contact with a real build.

## Ponytail

This repo uses ponytail discipline for ordinary coding work: **the smallest
change that fully solves the task**, and a reply a busy human understands in one
read. Invoke with `/ponytail` (levels `lite`, `full`, `ultra`);
`/ponytail-review` for a change, `/ponytail-audit` for the repo.

When you take a shortcut deliberately, mark it:

```rust
// shortcut: single-threaded apply; parallelism needs the lockfile first
```

`/ponytail-debt` collects those into a ledger, so a deferred decision stays
visible instead of becoming folklore.

### Where ponytail does not apply

**The eleven invariants above are not subject to minimisation, and neither is the
test discipline around them.** This is the one place where the simplest change
that passes the tests is the wrong change, and the failure is not hypothetical —
a safety review of this project's own implementation plan found exactly it.

Concretely, a minimising instinct will want to:

- replace `renamex_np(…, RENAME_EXCL)` with plain `rename` — simpler, one less
  `extern`, and every happy-path test still passes. It also silently destroys a
  file, which is invariant 6.
- drop the `assert_unchanged()` tree-snapshot harness as ceremony. It is the only
  thing standing between "the function returned `Err`" and "no files moved," and
  those are not the same claim.
- collapse the two sparse fixtures into one. They measure different things
  (invariant 8) and one cannot do both jobs.
- delete a test whose assertion looks redundant. Several here exist to pin an
  *ordering* — verify before unlink — that no type signature enforces.

Rule of thumb: **minimise the code, never the proof.** If a change makes the
diff smaller by making a destructive path less observable, it is not a ponytail
change, it is a regression wearing one.
