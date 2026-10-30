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
docs/research/   reviews and experiments   docs/history/    settled work, moved
                                                            out of this file
fixtures/        test data, filenames only
corpus/          manifest.json — the pinned corpus. Never the .sqlite itself.
data/decisions.tsv    the shipped filename→decision table. One file, one row
                      per key, no `bundle` column (D63, D64)
data/overrides.toml   first-party Layer B title overrides
review/          a README only. The reviewed Humble table is not here and not
                 in this repository's history — read it before looking.
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

**Released.** v0.1.0 is tagged and published, the repository is public, and
`brew install bsns-casual/tap/comic-renamer` works — after `brew trust`, which
current Homebrew requires of every third-party tap. Design approved, Phase 0
complete, Phase 1 complete, and the tool then rebuilt around a recorded
filename→title mapping: `docs/specs/2026-10-22-mapping-first-rebuild.md`,
planned in `docs/plans/2026-10-23-mapping-first-rebuild.md`, 14 tasks, all
done. The old Phase 1 plan's Task 14 (the gate) is superseded by it.

`scan`, `map`, `apply` and `undo` all run from the CLI. **`map` is the primary
command for a bundle the tool has not seen**: it needs a terminal, shows each
unmapped filename beside a guess, and writes a plan only from rows the user
approved. **`scan` is the non-interactive path**: it plans the mapped files,
lists the unmapped ones with a guess for each, and saves nothing. The resolver
is a suggestion engine, and the corpus is lazy and optional: a missing or
undownloadable corpus means "no guess", not an error (D49).

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
done   D54-D58  session-scoped undo, the startup menu, and the aside directory
done   D59-D61  single issues are first class: an `issue` column, rendered as
               four padded digits with no `#`, derived where a filename or a
               sibling of the same verified title vouches for it
done   D62  a shouting series is settled by the corpus where case alone decides
               it, guessed and still reviewed where the corpus is silent
done   D63-D64  one decisions table, no `bundle` column
done   D-a, D-c, D-d  the three owner decisions that needed code
done   T15  Homebrew. Release workflow, universal2 binary, formula, tap, and a
               v0.1.0 release that was installed and run end to end.
done   the Humble review: 2,204 of 2,204 decided. The table is deliberately not
       in this repository or its history (2026-10-30) — see `review/README.md`.
```

**Tests: 616 passed, 0 failed, 9 ignored**, re-measured 2026-10-30 by
`awk`-summing the `test result:` lines of `cargo test --workspace
--no-fail-fast`. That is the number to quote. The nine ignored need real
hardware or the real corpus release and last ran on 2026-10-26; CI runs all
nine. See "What the volume-gated suite does and does not protect" before
assuming a change needs them.

### Pick up here

Nothing is blocked. The open work is the backlog below — a resolver dedup the
owner has decided but nobody has implemented, three deferred signature
cleanups, three reasoned-not-tested claims, and the known gaps.

The settled work moved out of this file on 2026-10-30. It is not deleted; it
is in `docs/history/`, and each file says what it covers:

- `docs/history/2026-10-30-implementation-waves.md` — how the code got here.
  The issue column, session-scoped undo, Task 13's departures from the Phase 1
  text, the rebuild's final adversarial review.
- `docs/history/2026-10-30-the-humble-table-and-licensing.md` — why the big
  table is not here. The generator's output and its classification, the
  review, both licences, and the history rewrite.
- `docs/history/2026-10-30-release-test-runs-and-closed-decisions.md` — what
  was actually run. The tester guide, the first end-to-end runs and the three
  fixes they produced, every Homebrew measurement, and the full reasoning for
  D-a through D-d including the options not taken.

Read the relevant one before re-deriving a number or reopening a decision.
Several of them record a measurement that cost a session to get.

### What ships, and what does not

**Shipped coverage is 12 rows**, all in `data/decisions.tsv`: titles the
maintainer confirmed one at a time, from Fanatical (5), Sweet Shop (5) and
Global Comix (2). The per-marketplace files were merged on 2026-10-30 (D63);
the marketplace still structures `fixtures/marketplace-ground-truth.tsv`, which
is where that breakdown now lives, and `cargo xtask seed` regenerates the
shipped table from it.

**The Humble table is generated and deliberately not shipped.** `cargo xtask
seed` writes about 2,200 candidate rows to `target/seed/humble.tsv`, which is
untracked and gitignored. The 2,031-row decided version is not in this
repository or its history; `review/README.md` is the live document for it, and
`docs/history/2026-10-30-the-humble-table-and-licensing.md` is why.

### What the volume-gated suite does and does not protect

**Measured 2026-10-26, so nobody re-derives it.** The five
`COMIC_RENAMER_TEST_VOLUME` tests were run against this branch: **604 of 604
pass**, the first time every gated test has run locally here. The suite was 615
then and is 616 now; those five have not been re-run since, and nothing that
landed after touches their code. Three of the
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

### CI does not run on a markdown-only change

Added 2026-10-30, because the org was running short of Actions minutes.
`verify.yml` carries `paths-ignore` for `'**.md'` and `.gitignore` on **both**
triggers. The cost it was spending: **three of the six jobs are
`macos-latest`** — `rust`, `mappings` and `rust-corpus`, read from the
`runs-on` lines — which GitHub bills at **ten times** the Linux rate, and two
of them download the 34 MB corpus. A repository whose history is mostly docs
was paying all of that to re-prove something no prose can change.

Four things to know before touching it:

- **A commit touching markdown *and* anything else still runs everything.** The
  filter skips a run only when every changed path matches, which is why the
  commit that added the filter ran CI itself: run 41013142054 on `0db891b`, all
  six green including `rust-corpus` and the volume-gated job.
- **Nothing load-bearing is filtered.** `data/`, `crates/`, `corpus/`,
  `fixtures/`, `xtask/`, `scripts/` and `.github/` all still trigger, so
  `check-mappings`, the row caps and the file-size cap keep their teeth. Do not
  extend the list to `review/` on the grounds that nothing reads it: the gates
  are what would catch a mistake there, and `review/` is where a Humble table
  would re-enter the repository.
- **`paths-ignore` on `pull_request` means these checks never report at all on
  a docs-only PR.** Marking any of them a required status check would leave
  such a PR waiting forever. There is no branch protection and no ruleset on
  `main` today — checked against the API on 2026-10-30, not assumed. Add
  protection and this becomes a real trap.
- `[skip ci]` in a commit message was the alternative and was not chosen: it is
  per-commit and forgettable, where a path filter cannot be.

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
`docs/plans/2026-10-20-phase-1-implementation.md`, 15 tasks, all now closed —
its Task 14 superseded by the rebuild plan, its Task 15 (Homebrew) shipped
with v0.1.0.

## Open backlog

### Decided, not implemented

- **Duplicate candidates: dedup upstream, highest `volume_count` wins** (owner,
  2026-10-30). Confirmed on two stems: Popeye returns 21 candidates that
  collapse to 2 distinct series, Willie & Joe returns 2 identical in *every*
  field the picker renders. So the `series_line` fix does nothing for `map` and
  the dedup has to move into the resolver, on every field except `volume_count`.
  Measured first, because it bounds the stakes: the width is `max(2, digits)`,
  so `4` and `65` both give two — **the arbitrary pick is invisible below volume
  100.** Highest was chosen as the only option that cannot under-pad a real long
  run.

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

### Two standing owner calls

- **Kavita is dropped until a user asks.** The research in
  `docs/research/2026-10-23-kavita-from-source.md` stands and the finding is
  real — 890 of 2,223 rows would become one-book series — but nobody is
  waiting on it. Do not spend Phase 2 effort here unprompted.
- **The fault-injection facility is not worth building as a facility.** The
  three claims above are the reason; do them individually instead.

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
