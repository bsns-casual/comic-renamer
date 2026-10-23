# Comic Renamer — working notes for agents and contributors

A command-line tool that renames comic files bought in distributor bundles
(Humble Bundle and similar) into the layout a comic library app expects. It
identifies each file, shows a reviewable plan, and changes nothing until the user
approves it.

**Read `docs/specs/2026-10-19-comic-renamer-design.md` before changing anything.**
Its §13 decision log records every call and why; D1–D53. If you think a decision
is wrong, read its entry first — most have survived an adversarial review and the
reasoning is written down. Say what's wrong with the reasoning rather than
re-deciding silently.

**Then read `docs/specs/2026-10-22-mapping-first-rebuild.md`.** The tool no longer
derives a title by resolving a filename against a corpus; it looks the filename up
in a recorded mapping. That design supersedes parts of the first (its §2 lists
which), and its decisions, D39–D53, are folded into the first document's §13.

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
scripts/         CI checks, runnable locally: python3 scripts/check-manifest.py
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
todo   T15  Homebrew — see below
```

### What ships, and what does not

**Shipped mapping coverage is 12 rows**, in `data/mappings/fanatical.tsv` (5),
`global-comix.tsv` (2) and `sweet-shop.tsv` (5): titles the maintainer confirmed
one at a time. `cargo xtask seed` regenerates them from
`fixtures/marketplace-ground-truth.tsv`.

**The Humble table is generated and deliberately not shipped.** `cargo xtask seed`
also writes about 2,200 candidate rows to `target/seed/humble.tsv`, which is
untracked. It waits for the maintainer to review it, because D51 and the rebuild's
§4 forbid storing rows nobody approved, and committing 2,200 unreviewed rows on
the first commit would do exactly that. On its last run (2026-10-23) the
generator built 2,223 rows, rejected 1 (a short line), found 0 key collisions, and
de-duplicated to 2,204. Its own classification of the 2,223:

```
1896   85.3%  clean
 197    8.9%  bare trailing number in the series (not a volume marker)
 129    5.8%  volume marker swallowed into the subtitle (colon came first)
   1    0.0%  multiple colons
```

**"Clean" is an upper bound.** It means no heuristic fired, not that a human
agreed. The other 14.7% are known-bad decompositions, such as
`Attack on Titan: Before the Fall Vol. 9` becoming series "Attack on Titan" with
subtitle "Before the Fall Vol. 9" and no volume. Review them in a `map` session
before any of this is committed.

**Tests:** `cargo test --workspace` runs 380 and ignores 7 (counted 2026-10-23),
with no compiler warnings. The seven `#[ignore]`d tests need real hardware or the
real corpus release, and **CI runs all seven**. That claim was false until the
rebuild's Task 13: the workflow ran the three volume-gated tests, and its own
comment said the four corpus tests were not run. Task 13 added the `rust-corpus`
macOS job, which downloads the pinned release, checks its sha256 **and**
`sizeBytes`, and runs those four. The three volume tests run in the `rust` job
on a RAM disk, because a same-volume move is a rename and the streamed path, the
`placed` state and abort-on-failure are otherwise unreachable. This is read
from the workflow file. The four corpus tests were run locally against the pinned
release at the end of the rebuild (green, 54 s). No run on GitHub was observed
when this was written.

- `COMIC_RENAMER_TEST_ZIP=<gcd-*.zip>` — four tests against the real corpus
  release: install-and-open, coverage rates, and the publisher hint's effect
  on real titles. Run by the `rust-corpus` job.
- `COMIC_RENAMER_TEST_VOLUME=<mount on another device>` — three tests that need
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
- `cargo clippy --workspace --all-targets` reports 10 warnings across three
  crates (checked 2026-10-23, clippy on rustc 1.99.1): two `i64`→`i64` casts and
  one `u64`→`u64` cast in `fsops.rs` and `apply.rs`; "items after a test module"
  in `apply.rs` and `xtask/src/seed.rs`; three needless `clone` calls in
  `crates/core/tests`; and a "very complex type" in each of
  `crates/cli/src/map/mod.rs` and `crates/core/tests/shipped_mappings.rs`.
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

`undo` takes the **plan** path, per D38. The Phase 1 plan's Task 13 still
writes `undo <undo-manifest.csv>`; D38 reversed that and the plan document was
never updated.

### Pick up here

Two things are open and gate a release. Neither blocks development.

**1. The licence for publishing Humble-derived titles.** `data/LICENSE` ties
rule data to the Grand Comics Database under CC BY-SA 4.0. The Humble titles came
from Humble's order API, and D31 committed them as *test* data, with no URLs,
prices or account data. Shipping them as the product's dataset in a public
repository is a different use, and nobody has answered whether it is allowed.
**This gates release, not development.** Building and reviewing the table locally
does not wait on it; publishing it does. (D51, rebuild §10)

**2. The Humble table review.** See "What ships" above. The generator's output
sits in `target/seed/humble.tsv`, and the 14.7% in known-bad decomposition
classes need correcting by hand before anything is committed. A `map` session is
the intended tool for it.

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

**The nine invariants above are not subject to minimisation, and neither is the
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
