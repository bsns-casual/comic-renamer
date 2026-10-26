# Comic Renamer — working notes for agents and contributors

A command-line tool that renames comic files bought in distributor bundles
(Humble Bundle and similar) into the layout a comic library app expects. It
identifies each file, shows a reviewable plan, and changes nothing until the user
approves it.

**Read `docs/specs/2026-10-19-comic-renamer-design.md` before changing anything.**
Its §13 decision log records every call and why; D1–D57. If you think a decision
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
   **This covers the `bundle` field too**, through `mapping::same_bundle`, and
   it was broken there until 2026-10-23: every comparison of a bundle was
   literal, so two rows for one key whose bundles differed only in case passed
   `find_key_collisions` as two bundles and `lookup` then chose between them on
   the case the user typed. Five call sites route through `same_bundle` now, and
   the one that matters least obviously is `commit_session`'s merge — folding
   the gate without folding that would make a second session on the same folder
   append a row pair the gate refuses, so the user's own corrections file would
   stop loading. `xtask seed`'s de-duplication tuple is deliberately still
   literal: no fixture pair exists to fold, and a generator that produced one
   now fails at the gate.
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
- Each file under `data/mappings/` is at most 2 MiB, on every push and PR.
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
data/mappings/   the shipped filename→title table, one TSV per marketplace
data/overrides.toml   first-party Layer B title overrides
xtask/           cargo xtask measure | seed | check-mappings [base-ref]
scripts/         checks and tools, runnable locally:
                 python3 scripts/check-manifest.py
                 python3 scripts/review-rows.py <out.json>   the review page's data
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
2026-10-24 at `1eb028a`, after five adversarial review seats, a 16-item fix wave
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
todo   T15  Homebrew — see below
```

### Single issues, and the issue column (D59-D61)

Done on `main`, 18 commits ending `8c5254f` on 2026-10-26, planned in
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
pass**, the first time every gated test has run locally here. Three of the
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

**Shipped mapping coverage is 12 rows**, in `data/mappings/fanatical.tsv` (5),
`global-comix.tsv` (2) and `sweet-shop.tsv` (5): titles the maintainer confirmed
one at a time. `cargo xtask seed` regenerates them from
`fixtures/marketplace-ground-truth.tsv`.

**The Humble table is generated and deliberately not shipped.** `cargo xtask seed`
also writes about 2,200 candidate rows to `target/seed/humble.tsv`, which is
untracked. It waits for the maintainer to review it, because D51 and the rebuild's
§4 forbid storing rows nobody approved, and committing 2,200 unreviewed rows on
the first commit would do exactly that. On its last run (2026-10-25) the
generator built 2,223 rows, rejected 1 (a short line), found 0 key collisions, and
de-duplicated to 2,204. Its own classification of the 2,223, after the issue
column (D59–D61) landed on 2026-10-26:

```
1707   76.8%  clean
 189    8.5%  issue written in the verified title (#N)
 117    5.3%  issue copied from a sibling of the same title
 111    5.0%  corpus confirms the series (colon kept, volume recovered)
  59    2.7%  issue taken from the filename (D61)
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
both derived classes in the review pile, so the page still puts 212 rows in
front of a human — the same count as before, every one of them now arriving
with a better proposal. One of the 59 is wrong in a way only a reader catches:
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
  title by title, and a keystroke outranks a lookup (D50, D51). `data/mappings/`
  is byte-identical after a regeneration, which is the check that proves it.

A missing corpus is "no guess", not an error (D49): `seed` prints one line and
falls back to the parser for every row. `measure` still requires one.

**Tests:** `cargo test --workspace` runs 595 and ignores 9 (counted 2026-10-26
by summing the `test result:` lines: 595 passed, 0 failed, 9 ignored; 537 before
the issue column, 534 before
the corpus-confirms-a-colon change, 530 before
D58, 510 before a four-seat adversarial review, 500 before the whole-branch
review's fix wave, 409 and 7 before session-scoped undo), with no compiler
warnings. **The five volume-gated tests were run on 2026-10-26 and pass, so
604 of 604 is the whole-suite number** — see "What the volume-gated suite does
and does not protect" above before assuming a change needs them. The nine `#[ignore]`d
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

**-1. The issue column is merged and pushed.** 19 commits,
`b1a9c2f..7428246` on `main`, pushed 2026-10-26. 595 of 604 passing with the
volume suite run and clean, `check-mappings` and `check-review-page.cjs` both
green, no compiler warnings. `cargo xtask seed` leaves `data/mappings/`
byte-identical except the four rows that legitimately improved (`Hammer Head`,
`Minotaur`, `DIE: Loaded`, `Glarien: Origins` each carried `#1` as subtitle
text).

**CI run 40283202212: five of six jobs green at session close, and
`rust-corpus` was still running.** Confirmed green: the `rust` job including
its volume-gated tests, `mappings`, the corpus manifest, the licence check and
the held-out fixtures. **`rust-corpus` is unverified** — it downloads the
pinned release and runs the four `COMIC_RENAMER_TEST_ZIP` tests, which cannot
be run locally here because the cache holds `gcd.sqlite` and not the zip.
Those four go through `suggest_with` and the publisher hint, which Task 6
touched when `Suggestion` gained its issue field, so it is the one job this
work could plausibly have broken without a local signal. First thing to check:
`gh run view 40283202212`.

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
`main` on 2026-10-25 at `55407a3`, and **CI run 40163809503 is green on all six
jobs** — the first time every job has run on this work.

One note on how it got there, because the shape recurs: the merge went out with
the volume job red. `cargo test --workspace` reported 534 then and **skips the
nine `#[ignore]`d tests**, so "534 passed" was never the whole suite, and one of those
nine asserted the pre-D58 truth that a reversed copy leaves the library empty.
Four visible tests were updated when D58 landed; the invisible one was not,
because that round's brief did not require the RAM-disk run. **Quote the gated
total, or run the volume suite** — 595 of 604 as of 2026-10-26.

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

### Four decisions waiting on the owner

None blocks development and none blocks a release. Each was ruled on during the
session-scoped-undo work and deliberately left for the owner, with the reasoning
recorded here so it does not have to be re-derived.

**D-a. `apply`'s exit code for a consented run that places nothing.** A run that
*can* place nothing is caught before the prompt by `facts.files == 0`, skips the
mode block, and exits non-zero — that part is accurate. A run that passes the
typed confirmation and *then* places nothing, because every row collided at
execution, exits **zero**. By this file's own reasoning that is wrong: "an exit
code that is zero either way cannot tell a caller 'the library matches the plan'
from 'it does not'." It was left alone because it is pre-existing and a
user-visible change to the one contract automation depends on, which is not a
thing to alter inside a plan about undo records.

**D-b. What `reversible` should count.** It counts rows the record *recorded* as
reversible, not rows still in the library, so a partial reverse over-reports.
Changing it changes the menu's wording and `undo --list`'s output, so it is a
semantics decision rather than a fix. Note the related trap, already closed: the
count must stay derived from `expected()`, because the two disagreed once and a
crash in the default mode's one-syscall window then produced a record every
automatic path read as empty and the next run deleted.

**D-c. Whether the menu expands `~`.** `ask_path` strips quotes and does not
expand `~`; the doc comment claiming otherwise has been corrected. A shell
expands `~` before `scan` ever sees it, but a menu prompt does not — and the menu
is the entry point for people who do not use flags, which is exactly the
population that types `~/Comics`. The gap is real; how much path syntax a prompt
should parse is the open question.

**D-d. D57's "one record per pair" wording.** The sweep now exempts a record that
is `interrupted && reversible > 0`, so retention is one finished record **plus one
per un-reversed partial run**, with no constant cap. Measured: three
interrupted-with-rows records for one pair gives "swept 1; records left 4", and
`first_interrupted` surfaces only the newest, so the rest are reachable only
through `undo --list`. The behaviour is defensible and documented as a D57
exception; the entry's own summary sentence no longer describes it.

### Owner's calls on the backlog, 2026-10-25

- **The Homebrew formula is a release blocker.** It is for people who do not want
  to run `cargo`, so `cargo install --path crates/cli --locked` working is not a
  substitute. Not to be fixed now; **no release ships without it.** The standing
  warning holds: do not write it in a session that cannot run
  `brew install --build-from-source`, because this repo has been wrong about a
  packaging claim three times.
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
**1. The licence for publishing Humble-derived titles.** `data/LICENSE` ties
rule data to the Grand Comics Database under CC BY-SA 4.0. The Humble titles came
from Humble's order API, and D31 committed them as *test* data, with no URLs,
prices or account data. Shipping them as the product's dataset in a public
repository is a different use, and nobody has answered whether it is allowed.
**This gates release, not development.** Building and reviewing the table locally
does not wait on it; publishing it does. (D51, rebuild §10)

**2. The Humble table review. In progress, 15 of 2,204 decided.** See "What
ships" above for the class table.

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

**Task 15, Homebrew.** `cargo install --path crates/cli --locked` works and is
what the README documents, so the tool is installable today; the formula is the
convenience, not the gate. Do not write it blind. This repo has been wrong about
a packaging claim three times now, and a formula that cannot be
`brew install --build-from-source`-tested in the session that writes it is the
next instance waiting to happen. The formula must state Rust 1.88 and the C
toolchain.

### Known gaps

- **`scan` does not name `map`.** The rebuild's §6.1 says that when `scan` finds
  unmapped files it "reports the count and names `map`". Run 2026-10-23 on a
  folder with one unmapped file, `scan` printed the count, listed the file with a
  guess, and wrote a plan, and the word `map` appears nowhere in the output. The
  README documents what the code does, not what §6.1 says.
- **§6.11's caps are enforced in CI, and only partly at `apply`.** Field
  validation (forbid-list, 120-byte cap) runs when a mapping is loaded, in `map`,
  and in `seed`. The row and file-size caps run in CI. `rendered_budget_ok`
  exists but only tests call it, and for Komga it cannot fail because `sanitize`
  truncates every component at 255 bytes; it is kept for Phase 2 presets. The
  rebuild's §7.4 says all three are "re-validated at `apply`", and a search on
  2026-10-23 found no call to `MappingRow::validate` or `rendered_budget_ok` in
  `apply.rs`. `apply`
  protects itself a different way: it recomputes every destination through the
  preset, never trusting a path in the plan (invariant 2). Whether that satisfies
  §7.4's intent is the owner's call.
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
