# Comic Renamer — working notes for agents and contributors

A command-line tool that renames comic files bought in distributor bundles
(Humble Bundle and similar) into the layout a comic library app expects. It
identifies each file, shows a reviewable plan, and changes nothing until the user
approves it.

**Read `docs/specs/2026-10-19-comic-renamer-design.md` before changing anything.**
Its §13 decision log records every call and why; D1–D27. If you think a decision
is wrong, read its entry first — most have survived an adversarial review and the
reasoning is written down. Say what's wrong with the reasoning rather than
re-deciding silently.

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
5. **The resolver never opens a file.** Filenames only, always. (§5)
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

### The gate is real, and here are its numbers

§7's silent-false-positive gate is **measured**, not estimated:
`fixtures/humble-ground-truth.tsv` holds 2,223 verified filename→title pairs
pulled from the maintainer's own Humble library, where every download is
published next to its real title.

```
ready                66.1%
silent false pos      0.48%   <- the gate: hold at or below 0.5%
```

**Grade against it carefully.** A first pass reported 2.65% and every sampled
failure was the grader's fault: Humble writes `&` where the corpus writes "and",
prefixes promo items with "Free Issue" (containing a zero-width character), and
inserts `Vol 1:` mid-title. Normalise both sides or you will chase phantoms.

---

## Layout

```
docs/specs/      design documents          docs/plans/      implementation plans
docs/research/   reviews and experiments   fixtures/        test data, filenames only
corpus/          manifest.json — the pinned corpus. Never the .sqlite itself.
scripts/         CI checks, runnable locally: python3 scripts/check-manifest.py
.github/         workflows: verify (every PR), corpus-refresh (monthly)
```

Dated `YYYY-MM-DD-<topic>.md`.

## State

Design approved. **Phase 0 is complete. Phase 1 Tasks 1-13 are complete.**
`scan`, `apply` and `undo` all run from the CLI, and the whole round trip is
verified against a real bundle, a real second volume and a real Komga-shaped
HTTP server. **The tool is usable** — Tasks 14 (the gate) and 15 (Homebrew)
are the remainder, and neither blocks a user.

```
done   repo harness, CI, corpus manifest            (this file, .github/, corpus/)
done   D23  cloning a placeholder hydrates it -- move stays the default
done   D26  ground truth, 2,223 verified rows; the gate is measured at 0.48%
done   D34  Komga convention, specified from source (sections 6.8.1, 6.8.2)
done   T1   workspace skeleton; `cargo check --workspace --all-targets` clean
done   T2   normalise + residue          T8   sparse fixtures + TreeGuard
done   T3   corpus, verified fetch       T9   no-clobber syscalls + collisions
done   T4   Layer B overrides            T10  scan + a CLI that runs it
done   T5   resolver ladder              T11  apply pre-flight (refusals only)
done   T6   Komga preset                 T12  apply execution + undo
done   T7   JSONL plan + checksum       T13  CLI: apply, undo, disclosure
todo   T14  the gate — see below      T15  Homebrew — see below
```

**200 tests pass** (`cargo test --workspace`), plus seven `#[ignore]`d ones that
need real hardware. **CI runs all of them** — `.github/workflows/verify.yml`
has a macOS job that mounts a RAM disk, because a same-volume move is a rename
and the streamed path, the `placed` state and abort-on-failure are otherwise
unreachable.

- `COMIC_RENAMER_TEST_ZIP=<gcd-*.zip>` — four tests against the real corpus
  release: install-and-open, coverage rates, and the publisher hint's effect
  on real titles.
- `COMIC_RENAMER_TEST_VOLUME=<mount on another device>` — two tests that need
  a genuine second volume, because a same-volume move is a rename and cannot
  reach the streamed path. A 1.3 GiB HFS+ RAM disk works:
  `hdiutil attach -nomount ram://2726297` then
  `diskutil eraseVolume "HFS+" CRTEST <dev>`. **Under ~1.1 GiB every run is
  refused by the free-space reserve**, which is how the reserve is specified.

Two environment notes: `cargo` lives in `~/.cargo/bin` and is not on a
non-interactive shell's PATH, and **clippy is now installed** —
`cargo clippy --workspace --all-targets` is clean for `crates/cli`, and leaves
five warnings in `crates/core` plus three unused-variable warnings in `xtask`'s
Task 14 stubs. `rust-version` was bumped 1.80 → 1.85: `ErrorKind::CrossesDevices`
has needed 1.85 since Task 12, so the old claim could never have built.

The Phase 1 plan is written and survived three adversarial reviews:
`docs/plans/2026-10-20-phase-1-implementation.md`, 15 tasks.

### Task 13's departures from the written text

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
It carries a `shortcut:` comment saying so. Re-run with the script in the
session scratchpad, or write your own; it is fifteen string substitutions.

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

**Task 14, the gate — and it does not pass today.** `cargo xtask measure` is
still `todo!()`. Before wiring it into CI, settle the targets:

```
located 91.6%   gate >= 92.3%   <- fails
ready   63.6%   gate >= 66.0%   <- fails
```

Re-deriving those two numbers against the implemented ladder is an owner
decision, not a refactor: lowering a quality gate to match the code is how a
gate stops meaning anything, and D28's withdrawn tiebreaker is a legitimate
reason the ready rate moved. The silent-false-positive rate — the one that
actually matters — has **not** been re-measured against the implemented
resolver at all; 0.48% is the research number.

**Task 15, Homebrew.** Do not write the formula blind. This repo has twice
been wrong about a packaging claim that failed on contact with a real build,
and a formula that cannot be `brew install --build-from-source`-tested in the
session that writes it is the third instance waiting to happen.

### Known gaps, all spec-level

- `located`: *Domestic Girlfriend* and *Altair* are absent from the corpus —
  manga romanisation, the gap §6.2.1 gives Layer B. And
  `mirkaandolfosmercy_vol1` is the reverse match direction, where §6.1
  specifies the opposite; making matching bidirectional is a spec change with
  real false-positive risk.
- `ready`: largely the cost of implementing D28 correctly. Rows the original
  measurement picked with the withdrawn earliest-edition tiebreaker now go to
  review. **Re-derive the 66% target against the implemented ladder** before
  Task 14 turns it into a merge gate.
- The free-space reserve is `max(1 GiB, 10%)` taken literally from the plan,
  so **no run is possible on a volume with under ~1.1 GiB free**. Correct per
  spec, harsh in practice.
- Three §6.11 items stay unenforced on purpose — total path budget,
  rule-count cap, file-size cap. All three govern *contributed* rule data,
  which is Phase 3; `data/overrides.toml` is first-party and CI validates it.
- `ApplyResult.failed` and `Collision.existing` are unread today. Both are
  Task 1 skeleton fields, and `Collision.existing` is what Task 13's
  disclosure table needs to say what is already at a destination. Deferred,
  not dead.

Phase 0b — the 13-app research spike — runs *parallel* to Phase 1 and gates
Phase 2 only. Nothing in Phase 1 waits on it.

Volume numbers are **zero-padded to a per-series width** (D37) — the digit count
of that series' highest volume, from the corpus `issue_count`, floored at two.
Komga's comparator is numeric and does not need it, but `ls`, Finder and most
scripts sort lexically, where `Vol. 10` would precede `Vol. 2`. A fixed two-digit
width breaks silently at volume 100 and manga runs pass it.

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
