# Session-scoped undo, and a startup menu

Status: **proposed**, 2026-10-24. Revised the same day after a builder check,
which found five blocking defects in the first draft; this version records what
it changed. Owner's decisions are marked. Not yet planned or built.

Supersedes D38 and §6.13's amendment. Reference for the interaction model:
[Mole](https://github.com/tw93/Mole).

---

## 1. Why

Two findings, written up in
`docs/research/2026-10-23-two-pre-existing-data-safety-findings.md`, share one
cause: **the record of what a run did is a file the user hands to the tool.**

- `undo` deletes a library file no run ever placed and reports `restored: 1,
  refused: []`. The destination is recomputed from the plan, which invariant 2
  already treats as untrusted, and the identity is read from the manifest, which
  arrives by naming convention in the same directory. The identity check
  compares the file against a number whoever wrote the manifest chose.
  Reproduced and executed: the deleted file was 33 bytes against a 5-byte
  source, so no clone or stream of that source produced it.
- `apply`'s typed `MOVE`/`COPY` confirmation gates a plan it re-reads afterwards.
  Nothing holds the lock across the prompt, so a second terminal running `scan`
  substitutes a different plan under a confirmation given for the first one. No
  adversary required.

### D38 was right about the manifest and wrong about the remedy

D38 moved the anchor from the manifest to the plan, reasoning that "whoever edits
it supplies the roots *and* the identities those roots are checked against." That
is correct, and it is equally true of the plan. Moving the anchor from one
user-supplied file to another relocated the defect.

**The first draft of this document then made the same mistake a third time.** It
proposed moving the record into a private directory and called that ownership. A
builder check pointed out that the startup sweep *enumerates* that directory, so
a planted record is reached without anyone naming a path — and `config_dir()`
honours `XDG_CONFIG_HOME` with only an absolute-path check, so the directory
itself is relocatable. Ownership by location is not ownership.

The pattern is worth naming, because three designs in a row have fallen into it:
**a record cannot establish its own trustworthiness, wherever it is stored.**
What breaks the loop is not a better location but refusing to let the record be
the authority for the one irreversible act.

---

## 2. The decisions

**D54 — A record locates files; it never authorises deleting one.**
Reversing a *copy* deletes a library file, and that is the only irreversible
half. The tool verifies against the filesystem instead of against the record:
the destination may be removed only when the file it was copied from is still
present at the recorded source **and has the same content**. A planted record
cannot make the tool delete anything the planter did not already hold a copy of.

Reversing a *move* needs no such check. It renames the destination back, through
`renamex_np(RENAME_EXCL)`, which refuses rather than clobbering (invariant 6).
The residual exposure from a forged record is therefore bounded to *relocating*
one file from the library into the source folder under a name of the forger's
choosing, both roots being the user's own, and `plain_relative` plus
`single_component` keep it to a single component inside `source_root`.

**D55 — The record's lifetime is keyed to whether undo was offered and
declined, not to the process.** Quitting the menu is declining the undo you were
just shown, so the menu deletes its records on quit. A subcommand `apply` keeps
its record, as today. No path silently loses undo, and no flag is needed.

*Consequence to state plainly:* "undo only works while the program is running"
therefore describes the **menu**, not the subcommands. The first draft applied it
uniformly and the builder check was right that this silently removed undo from
an interactive `comic-renamer apply` where the user had typed `MOVE`.

**D56 — `undo` stays a subcommand, keyed on a run rather than a plan.** D55
keeps a subcommand's record, so there has to be a non-interactive way to reach
it; `comic-renamer undo` reverses the most recent reversible run,
`undo --list` names them, and `undo <run-id>` picks one. `--yes` applies as it
does to `apply`.

An argument is safe here *because* of D54. The first draft removed `undo`'s
argument on the theory that accepting one was the danger. It was not — the
danger was the record being the authority. Once the filesystem check decides
whether a deletion is permitted, naming a record cannot authorise anything,
which is the same reasoning that lets a test seam take a path. A run id is also
not a path: it selects from records the tool wrote and enumerated.

**Both surfaces stay**, as Mole has them: `comic-renamer` with no arguments opens
a menu, and `scan`, `map`, `apply` and `undo` remain subcommands with `--yes`
for scripting.

### Proposed invariant 11

> **A record never authorises its own deletion.** Reversing a copy removes a
> library file; the record may say *which* file, never *that it is safe to
> remove*. That is verified against the filesystem — the source it was copied
> from must still be there, with the same content. A record is a map, not a
> warrant.

Stated this way it is about authority rather than reachability, which is what
makes it robust: it stays true however the record is reached, so a contributor
may freely add a path-taking seam for tests without reopening anything. The
first draft's version — "the undo record is never a path the user supplies" —
was already false via `XDG_CONFIG_HOME`, and forbade the wrong thing.

---

## 3. The record

One file per run under `config_dir()?.join("runs")`, created on first use
(nothing currently creates `config_dir()` itself). `config_dir()` already honours
`XDG_CONFIG_HOME`, refuses a relative or empty `HOME`, and is Application
Support rather than Caches so the system does not reclaim it (D42).

Names follow `temp_basename`'s pid-plus-counter *shape* rather than calling it,
plus a wall-clock stamp so the menu can say when the run happened. The builder
check suggested a `.partial` suffix as well; that reason does not survive the
move, because the records directory is under `config_dir()` and is never a
folder `scan` walks, so nothing here can be mistaken for a comic. Uniqueness matters because the lock
is per-library and two terminals can legitimately apply two different plans into
two different libraries at once.

**Creation stays lazy**, on the first row that actually executes. That is a
deliberate fix, not an accident: creating it up front meant a run that refused
every row still wrote a header-only manifest, and `O_EXCL` then rejected the
plan forever. Unique filenames make the collision impossible now, but the
behaviour is kept and the launch offer is gated on a non-empty record, so a run
that placed nothing produces no offer. The first draft's lifecycle table said
"`apply` starts | created" and silently reverted this.

### The schema has to grow

**This is the blocking defect in the first draft.** `reverse` takes
`recomputed: &[PathBuf]`, built by the caller from the *plan's* rows through the
preset (`apply.rs:366-371`). The manifest's nine columns are
`seq,status,mode,mechanism,source_name,dest_relative,size,inode,mtime_ns` —
nothing a preset can recompute from. So "the format is today's CSV, unchanged"
and "the next launch can reverse an interrupted run" cannot both hold: with the
plan gone there is nothing to recompute and `reverse` cannot even be called.

The record must carry what `preset.destination(&resolved, &extension)` needs,
which is exactly `series`, `volume`, `subtitle`, `run` and `extension` per row —
`status` is always `Mapped` on this path and `record` always `None`. The roots
row gains the **preset id** and the **planned row count**, the latter so the
menu can say "312 of 400" at all; `record_pending` is per row, so a crash
otherwise leaves a numerator with no denominator.

`reverse` then recomputes destinations itself from the record and drops the
`recomputed` parameter. It still goes through the preset, because invariant 1
makes presets the only code that constructs paths and that is independent of who
owns the record.

**The plan stays untrusted.** It remains the reviewable artifact that goes to
assistants and PRs, and `apply` keeps recomputing every destination from
metadata (invariant 2).

### The lock has to move, not disappear

`Lock::acquire(plan)` opens `<plan>.lock`, and its own doc comment says "`undo`
takes this same lock, which Task 11 always required and which two separate lock
files silently defeated." A run-keyed `undo` would take no such lock and walk
straight back into that.

The lock moves to the **canonical destination root**, which is what both commands
actually mutate, as a file in the records directory named from that root rather
than inside the user's library. This is deliberately *broader* than today: two
different plans targeting one library now serialise, where today they do not.

### Live records versus abandoned ones

A record being present does not mean its run is over — the first draft's "a
record that survives means the process died mid-run" is simply false, under D55
and also whenever a delete fails. Without a way to tell, a second session's
sweep could offer to reverse a run still in progress.

`flock` answers it, for the reason the existing `Lock` comment already gives:
the kernel releases an `flock` when its owner dies, which is why `Lock` uses it
over `O_EXCL`. A run holds an `flock` on its own record; the sweep takes a
non-blocking `flock` on each record it finds and skips every one it cannot get.

### Deletion is a step, never a destructor

The delete on the quit path is an explicit step. It must **not** be a `Drop`
impl: a panic unwinds, and that is precisely the case where the record has to
survive.

### Lifecycle

| event | record |
|---|---|
| first row that executes | created, `flock` taken, roots row fsynced |
| each row | `pending` → `placed`/`failed` → `done`, fsynced per transition |
| run ends, menu session continues | kept; undo offered in the menu |
| user reverses it | reversed, then deleted |
| quit the menu | deleted — quitting is declining (D55) |
| subcommand exits | kept (D55) |
| process dies | survives, `flock` released by the kernel |
| a later launch finds one | `flock`-tested, then offered if non-empty |
| a new run supersedes it | the older record for the same root pair is deleted |

**D57 — a kept record is retained as the most recent per
`(source_root, dest_root)` pair.** Nothing in the first draft pruned a
subcommand's record, so the directory grew without bound. Keying retention to the
root pair means two different bundles into one library each keep their undo,
because they touch disjoint files and reversing either is safe; re-running the
same bundle into the same library supersedes its own previous record, which is
the case where reversing the older one out of order would be a footgun. Records
are a few tens of kilobytes, so one per bundle ever imported is not a storage
concern.

*This is the one policy in this round the owner may well want different* — a
retention age, or a count, are both defensible alternatives.

### What verification costs, and when it is skipped

Content-verifying both sides of every copied row means reading two files per row.
For a 300-file bundle that is real time, and it is the right price for an
explicit recovery action rather than a hot path.

It is skipped for a reverse **within the session that wrote the record**, where
the in-memory copy is trustworthy and today's identity check is sufficient. Only
a record read back from disk needs the filesystem check, which keeps the common
case — "I just ran this and want it undone" — as fast as it is now.

---

## 4. The second finding, and the API change it implies

Read the plan once, hold it, show the disclosure computed from that copy, take
the confirmation, execute from the same copy.

**The first draft called this "the natural shape rather than a change", which
understated it.** `check` and `apply` each take `&Path` and acquire the lock
internally, so moving to "caller reads and holds, callee takes the held plan"
changes a public signature, not a call.

The builder check put this at 39 call sites. **It was right, and the correction
printed here on 2026-10-24 was wrong.** That correction claimed 7, from a grep
for the qualified paths `apply::apply`, `apply::check` and friends. The largest
caller imports them instead — `crates/core/tests/apply_exec.rs:7` is
`use comic_renamer_core::apply::{apply, undo, undo_path};` — so every one of its
calls is unqualified and the grep could not see a single one. That file is 898
lines, carries 48 matches for the changing names, and its `Fixture` is built on
`undo_path` at line 68, which this design deletes.

Counted across the tree: `apply_exec.rs` 48, `apply_refusals.rs` 9,
`apply.rs` 7 (definitions and internal calls), `main.rs` 4, `undo.rs` 1. The
order of magnitude is the builder check's, not the correction's.

The method error is the point worth keeping: a grep for a qualified path cannot
count callers that import the name, and reporting its output as "counted" gave a
wrong number more authority than the estimate it overrode. The scope is real —
a plan for this has to name `apply_exec.rs`, because 898 lines of destructive-path
tests hang off the function being deleted.

Holding the lock across the prompt is still worth doing — it stops a second
`apply` interleaving — but it does *not* close the finding on its own, because
`plan::write` acquires no lock at all. Executing from the held copy is what
closes it. The research note has been corrected on this point.

---

## 5. The startup menu

`comic-renamer` with no arguments. `Cli::command` is currently required, so this
needs `Option<Command>`; the menu refuses without a TTY exactly as `map` does.

```
  comic-renamer

  A run was interrupted on 2026-10-24 (312 of 400 rows placed).
    r  reverse it      l  leave it

  comic-renamer apply placed 400 files on 2026-10-23.
    u  undo it         k  keep it

  1  map a new bundle        — pick a folder, approve each row
  2  scan a mapped folder    — plan without saving anything
  3  apply a plan
  4  reverse this session's run
  q  quit
```

- Two kinds of record can be waiting, and the menu must not describe them in the
  same words. An **interrupted** run died mid-way and left a library
  half-migrated; it comes first, because it is the only item about a library that
  is currently inconsistent. A **completed** run from an earlier subcommand
  invocation is intact and merely still reversible. The first draft had one
  offer worded "a run was interrupted", which would have described a clean
  `apply --yes` from yesterday as a crash.
- Either offer appears only for a non-empty record no live process holds.
- Declining a completed run's offer (`k`) is the decline D55 keys on, so that
  record is deleted. Declining the interrupted one (`l`) is not: a
  half-migrated library stays reversible until someone fixes it.
- Item 4 appears only when this session has a run to reverse, and quitting
  deletes it with one line saying so rather than silently.
- The record's path is never printed. Today's `apply` prints it; availability is
  what the user can act on, and a printed path invites being passed back in.

**Per-action options, not a settings screen.** Source, destination, mode and
preset are collected by the action that needs them. Persisting them is a separate
decision and deliberately not proposed: a config file that remembers a
destination is a config file that silently files a bundle somewhere the user
forgot about.

---

## 6. What this breaks

- **D38** becomes unnecessary *in-session* and must be *replaced* out of session:
  its premise is void once the record is tool-written, but its lock consequence
  is live and is answered in §3, not dropped. "Unnecessary rather than wrong",
  as the first draft put it, was half right.
- **`comic-renamer undo <plan>` changes shape rather than going away** (D56):
  the subcommand survives, keyed on a run instead of a plan path. Pre-release, so
  no migration is owed.
- **The roots cross-check** (`undo.rs:216-221`) becomes `x == x` once the record
  supplies both sides, so it and its test are deleted rather than inverted.
- **CLAUDE.md** needs invariant 11, the `undo` entry under State, and a revisit
  of the Known Gaps entry about `ApplyResult.failed`.
- **The forged-manifest tests** change shape. The first draft said to keep and
  invert them to prove a planted file is unreachable; that test is unwritable,
  since `undo::reverse` is `pub`, takes a path, and the sweep reaches such a file
  anyway. Under D54 the valuable test is the real one: **a planted record naming
  a library file does not delete it unless a matching file is present at the
  recorded source.** That is exactly the executed reproduction from the research
  note, turned into a regression test.
- **`reverse_one`'s identity logic, `plain_relative`, `single_component` and the
  orphan handling are unaffected.** What changes is where the record comes from
  and what authorises the delete.

---

## 7. Left to the builder

- The hash to content-verify with, and whether to stream or read whole. The
  streamed copy path already hashes, so the machinery exists.
- Whether the menu is `ratatui` like `map`'s table or plain prompts. `map`
  already owns the dependency, so either is cheap; five lines does not obviously
  need a TUI.
- How a partially-placed run renders its detail behind the `r` offer.
- Sweeping records left by an install that predates the sweep.
