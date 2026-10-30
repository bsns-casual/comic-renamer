# Release, the first real test runs, and the decisions that closed

Moved out of `CLAUDE.md` on 2026-10-30. What stayed there: the tool is
released as v0.1.0, `brew install bsns-casual/tap/comic-renamer` works after
`brew trust`, and D-a through D-d are all settled and implemented.

This file holds the measurements behind those sentences, so none of it has to
be re-derived: what was actually run on 2026-10-29/30, the three fixes that
came out of it, every Homebrew measurement, and the full reasoning for each of
the four owner decisions including the options not taken.

---

## Handing it to a tester, and the first end-to-end runs

### Handing it to a tester, 2026-10-30

A walkthrough was written for an outside tester on an Intel Mac, published as
a private artifact at <https://claude.ai/artifact/3vKBzvec6f668ac5PhowF3>.
**Republish that slug rather than making a second one**, for the reason the
review page already records: a new artifact orphans the old link.

**Intel is answered, and the answer is measured rather than reasoned.** The
published v0.1.0 tarball is genuinely universal — `lipo -archs` gives
`x86_64 arm64` and its sha256 matches GitHub's own asset digest. The two
slices carry different load commands, which is worth knowing because `otool`
looks like it is lying: arm64 has `LC_BUILD_VERSION minos 11.0`, x86_64 has
the older `LC_VERSION_MIN_MACOSX version 10.12`, so grepping for `minos`
across the fat binary finds only one of them. **The x86_64 slice was
executed**, under Rosetta on this machine: `scan`, `apply` in move mode,
`apply` in copy mode (the `clonefile` path) and `undo`, all correct. That is
the binary proven, not the hardware, and the README's "Developed and tested
on Apple Silicon" is deliberately left alone as the conservative claim.

**The technique that made the guide's numbers right: `XDG_CONFIG_HOME` and
`XDG_CACHE_HOME` both pointed at a scratch directory.** The same 18-file
bundle is 15 mapped / 3 unmapped on the maintainer's machine and
**14 / 4 on a clean one**, because `<config>/mappings.tsv` holds the Prince
Valiant rows typed on 2026-10-29. Any claim about what a stranger sees has to
be made under a scratch config, or it is a claim about this machine.

The bundle itself is 18 generated files: 14 hit the 12 shipped keys (two
titles ship in two formats), and 4 are unmappable on purpose. The guide pastes
it as one `for` loop, checked in both `bash` and `zsh`.

Three findings, all now recorded where they belong:

- The corpus download prints nothing — Known gaps, with the numbers.
- `scan` does name `map`, and D-c is implemented — both were stale here and
  were fixed in `21b1f4d`.
- CI was running six jobs, three of them `macos-latest`, on every docs commit
  — fixed in `0db891b`, recorded above under "CI does not run on a
  markdown-only change", and **proved both ways**: `0db891b` touched
  `.github/` and ran all six green (run 41013142054), `add702e` was markdown
  only and triggered nothing.

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


---

## The four owner decisions, and the reasoning behind each

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


---

## Homebrew: every measurement, and the route off the `brew trust` line

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

