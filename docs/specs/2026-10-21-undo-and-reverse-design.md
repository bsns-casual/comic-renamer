# Undo and `reverse` — design

**Date:** 2026-10-21
**Status:** reviewed twice, revised, ready to implement. Written because two
implementations failed review; revised because two design reviews found the
first draft assumed primitives that do not exist.
**Supersedes:** the manifest description in Task 12 of
`docs/plans/2026-10-20-phase-1-implementation.md`, which is kept for its
`O_EXCL`, append-only and RFC 4180 requirements — all three survive.

---

## Why this document exists

`undo.rs` has been written twice and reviewed twice. The first version drew
six confirmed data-loss findings. The rewrite fixed five of eleven, left four
working, and turned one from a visible failure into a silent one.

The pattern in both failures is the same and worth naming, because it predicts
the next one: **each fix addressed the trigger a reviewer handed over, not the
class it belonged to.** Two examples.

- Task 9 found `..` defeating a lexical `strip_prefix` containment check in
  `fsops`. That was fixed properly, with a component-wise walk through
  directory descriptors. Three days later the rewrite introduced
  `destination.starts_with(root)` in `undo.rs` — the same bug, in a different
  file, with the correct machinery already available two modules away.
- The newline-in-a-filename finding was fixed by writing a record-level CSV
  reader. The reader is correct. But every *other* parse failure still aborts
  the whole manifest, so the class — "one bad row must not strand the rest" —
  was never addressed, despite the rewrite's own docstring promising it.

So this design states invariants first and a record format second. The code
follows the invariants; it does not accumulate guards.

---

## Invariants

1. **The manifest is untrusted input**, held to the same standard as a plan
   file (§6.11). It is a sibling of the plan, it is plain text by design, and
   `reverse` acts on it destructively.
2. **The manifest records roots and relative paths, never absolute paths.**
   This mirrors invariant 1 of the project: a bad record can then produce a
   wrong *name*, never a wrong *location*.
3. **Containment is structural, not checked.** Every filesystem operation goes
   through `fsops`' `(root, relative)` primitives, which walk components with
   `O_NOFOLLOW`. `Path::starts_with` is not a containment test and is not used
   as one.
4. **Identity is exact, or the row is refused.** No fallback that widens the
   match. A size alone is never sufficient to delete or displace a file.
5. **Every row is independent.** A row that cannot be reversed is reported;
   the rest continue. This includes rows that fail to *parse*.
6. **`reverse` never destroys the last copy.** Before removing anything it
   establishes that another copy exists.
7. **One lock serves the plan**, taken by `apply` and by `reverse` alike, and
   **held with `flock(2)`, not `O_EXCL`**. The kernel releases an `flock` when
   its owner dies; an `O_EXCL` file does not, and a crash is exactly when
   `reverse` is needed. `O_EXCL` stays on the *manifest*, where its job is to
   refuse to truncate a previous run's record.
8. **The anchor is the plan, not the manifest.** Both roots are taken from the
   plan header, canonicalised and overlap-checked the way `apply` does. The
   manifest's own `roots` record is a cross-check: if it disagrees, the
   manifest is refused. Otherwise exact identity buys nothing, because
   whoever edits the manifest supplies the identity too.
9. **A destination is only recognised if the preset still produces it.** A row
   is accepted only when its `dest_relative` equals
   `preset.destination(...)` recomputed from a plan row. This is invariant 2
   applied to `reverse`, and it is what stops a hand-written manifest naming
   an arbitrary relative path.

---

## Why relative paths settle containment

Two facts already established make this free:

- `scan` is non-recursive and builds every candidate as
  `read_dir(source_root)` + `entry.path()`, so a row's source is always
  exactly one component inside the canonical source root. `apply` now asserts
  this (`verify_inside_source_root`).
- A destination is always `preset.destination(...)`, relative to `dest_root`
  by construction, and `apply` recomputes it at the point of use.

So the manifest can record `source_root`, `dest_root`, a source *filename*,
and a destination *relative path* — and `reverse` can restore with
`rename_excl(destination, source_root, filename)`, the same fd-walked
primitive `apply` uses. Containment stops being a check that can be written
wrongly and becomes a property of which function is called.

A `..` in a recorded relative path is then rejected by
`resolve_destination`'s existing `Component::Normal` match, which is already
tested.

**This requires primitives `fsops` does not yet export.** The first draft of
this design assumed it could stat and unlink by `(root, relative)`; it cannot.
`resolve_destination` and `Destination` are private, and `rename_excl`
hardcodes `AT_FDCWD` for its *from* side. Without new primitives every step
below would be written as `dest_root.join(rel)` plus `symlink_metadata` and
`remove_file` — which follows a symlinked series directory under `dest_root`,
the same class of bug as the `starts_with` check this design exists to remove.

So `fsops` gains, over one `resolve_destination` walk each:

- `resolve_at(root, relative) -> Entry` — one walk, holding the parent
  descriptor open
- `Entry::metadata()`, `Entry::remove()`, `Entry::rename_into(to_root,
  to_rel)` — all through that one descriptor
- `stat_at(root, relative)` — a one-shot `metadata` where nothing destructive
  follows
- `ensure_root(root)` — `mkdir` the root itself when it is absent
- `prune_empty_dirs(root, relative)` — `rmdir` upward, ignoring `ENOTEMPTY`

**A handle, not a pair of functions.** `stat_at` followed by `remove_at`
resolves twice, and a component swapped between the two walks means the check
and the act saw different directories. macOS has no `funlinkat`, so the leaf
name is still re-looked-up inside the parent at unlink time; holding the
parent is what closes the part that can be closed. `apply`'s
forward path moves to `rename_excl_at` too, since `verify_inside_source_root`
already establishes its source as `source_root` + one component.

---

## Record format

Append-only. Header line, then a root record, then one record per status
transition. RFC 4180 quoting, doubled embedded quotes, every path field
prefixed with `'` unconditionally.

```
seq,status,mode,mechanism,source_name,dest_relative,size,inode,mtime_ns
0,roots,,,'<source_root>,'<dest_root>,,,
1,pending,move,stream,'mazebook.pdf,'Mazebook/Mazebook.pdf,4096,8814,17293...
1,placed,,,,,4096,9201,17293...
1,done,,,,,,,
```

Exactly one `roots` record, at seq 0, as the first data record. A second
refuses the manifest — last-wins is a hand-edit vector.

**Identity is `(size, inode, mtime_ns)`, and all three must parse and match.**
`(size, inode)` alone is preserved by an in-place edit, so invariant 6 would
delete the only *correct* copy of a file the user had fixed. An empty or
unparseable identity field refuses the row; a status record is never written
with an empty identity, because the same bytes would mean both "this status
carries none" and "the stat failed".

### Statuses

The second implementation had `pending` and `done`, which made a bare
`pending` ambiguous between *crashed mid-operation* and *refused at the
syscall*. `reverse` guessed between them by matching on size, and that guess
is what reopened the two worst findings.

| status | meaning | what `reverse` does |
|---|---|---|
| `roots` | the two roots for this run | validated, then every path is resolved beneath them |
| `pending` | about to attempt; source identity recorded | identity unknown — **refuse**, unless the mechanism is `rename`, where the inode is preserved and the source's identity is still exact |
| `failed` | attempted, created nothing | skip |
| `placed` | a new inode exists at the destination and is verified; its identity recorded | the destination is ours |
| `done` | complete; for a move the source is gone | reverse normally |

`placed` is written by **every mechanism that creates a new inode — clone and
stream — in both modes**, before any unlink. The first draft specified it for
moves only, which would have left every copy row at a bare `pending` and
refused forever. A rename creates no new inode and goes `pending` → `done`.

**A failed `placed` write aborts the row before the unlink.** Otherwise the
row is a bare `pending` with the source gone and an unrecorded inode at the
destination: the user's only copy sits in the library under a name `reverse`
will not touch. The unlink is the only step between `placed` and `done`.

Status rows are resolved per `seq`, last one wins. A status row whose `seq`
has no `pending`, a short row, or an unparseable `seq` is a **per-row
refusal** naming the line — never a whole-manifest abort. Adding two status
values while unknown vocabulary still aborted the file would add two more ways
to strand a manifest.

`placed` is the record finding 6 needed and neither implementation had. A
streamed move that verifies the copy and then fails to unlink the source sits
at `placed`, unambiguously — and removing the destination is then provably
safe rather than inferred. The reviewer's point stands that `failed` is the
wrong shape for "succeeded, then half-failed"; `placed` is that shape.

A rename goes `pending` → `done` with no `placed`: it is atomic and the inode
does not move.

---

## What `reverse` does, per row

`reverse` takes the **plan** path and derives the manifest from it, so it
takes the same lock `apply` does. §6.13 says undo operates on the manifest and
that they are different artifacts; this amends that, recorded as D38.

1. Read the plan. Canonicalise both roots, require both to be directories,
   refuse if they overlap — the same refusal `apply` makes. Resolve the
   preset.
2. Read the manifest. Refuse the whole file only for: a missing or duplicated
   `roots` record, or a `roots` record that disagrees with the plan's
   canonicalised roots. Everything else is a per-row refusal.
3. Reject a `source_name` that is not one normal component, and any
   `dest_relative` with a component that is not `Normal`. Accept the row only
   if its `dest_relative` equals `preset.destination(...)` recomputed from a
   plan row — invariant 9.
4. `stat_at(dest_root, dest_relative)`. `NotFound` is *nothing to do*; any
   other error is a refusal naming the path, because `Path::exists()` answers
   `false` on `EACCES`.
5. Require exact identity, all three fields. `placed` and `done` rows resolve
   to the destination's own; a bare `pending` resolves to the source's, which
   is exact only for `rename`. Anything else: refuse.
6. For `copy`: `stat_at(source_root, source_name)`. **If the source is absent,
   refuse** — removing the destination would destroy the last copy. This is
   the path an ordinary user reaches with no crash and no edit: copy a bundle
   in, delete the bundle folder, undo. If the source is present but is not
   what was recorded, refuse: the user has their own file there and the
   library copy may be the one they want. Otherwise `remove_at`.
7. For `move`:
   - Source absent: restore. `ensure_root(source_root)` first, since the
     common case is a user who deleted the emptied bundle folder. Then
     `rename_excl_at(dest_root, dest_relative, source_root, source_name)`,
     falling back to `stream_verified` + `remove_at` on `CrossesDevices`.
   - Source present **and the row is `placed`**: this is the duplicate left by
     a failed unlink. `remove_at` the destination. Gated on the status, not on
     identity alone.
   - Source present and the row is `done`: refuse as occupied, naming the
     path.
8. After a successful restore, `rmdir` each ancestor of `dest_relative`
   beneath `dest_root`, newest-first, ignoring `ENOTEMPTY`. `rmdir` refuses a
   non-empty directory, so this cannot remove one that still holds books.
9. Report `UndoReport { restored, skipped, refused }`, every refusal naming
   the path and the reason.
10. If the run completed with zero refusals and nothing skipped, rename the
    manifest to `<plan>.undo.csv.reversed`. The audit trail survives and the
    plan becomes runnable again — otherwise a successful undo left the
    manifest in place and `O_EXCL` refused the plan forever.

Idempotence falls out of step 4: a second pass finds every destination absent
and reports it as skipped. It is a Task 12 requirement and is stated here so
it is tested rather than assumed.

## Test obligations

Named because this document exists to stop a repeated failure, and an
unpinned invariant is how both previous attempts regressed. One test per
invariant, minimum:

| invariant | pin |
|---|---|
| 1, 8, 9 — manifest untrusted | a hand-written manifest naming a path outside both roots, and one whose `roots` disagree with the plan, are both refused |
| 2, 3 — containment structural | a `dest_relative` containing `..`, and a symlinked series directory beneath `dest_root`, are both refused; asserted through `stat_at`/`remove_at`, not `starts_with` |
| 4 — identity exact | a same-size, same-inode replacement with a different mtime is refused; a bare `pending` clone row is refused rather than matched on size |
| 5 — rows independent | a malformed row mid-file, and a reoccupied original, each leave every other row reversible |
| 6 — never the last copy | a `copy` whose source has been deleted is refused |
| 7 — one lock | `apply`'s lock blocks `reverse` and vice versa; a lock whose owner was killed does not |
| `placed` | a streamed move whose unlink failed leaves a duplicate that `reverse` removes — the test the plan lists as `an_interrupted_apply_reverses_the_completed_rows` and that has never been written |

`reverse_refuses_a_destination_outside_the_manifests_root` currently pins the
`starts_with` behaviour this design removes, and must be rewritten against the
new primitives rather than deleted.

Manifests written before this design are unreadable by it. Nothing has been
released, so no migration is owed.

## Deliberately not solved

Stating these so the docstring does not overclaim, which the second
implementation's did.

- **The crash window.** A kill between the operation completing and its status
  record landing leaves a bare `pending`. That is why step 4 refuses rather
  than guesses: the safe answer to "I cannot tell" is to leave the file alone
  and say so.
- **The status write can fail for the reason the operation did.** `ENOSPC` on
  the plan's volume affects both. `reverse` must stay correct when a status
  record is missing, which step 4 is.
- **Non-UTF-8 filenames.** Paths go through `to_string_lossy`, so a name that
  is not valid UTF-8 is recorded with replacement characters. APFS rejects
  such names; this is reachable only on exFAT, SMB or NFS destinations, and is
  out of Phase 1's scope.
- **A duplicate left by a failed cross-device reverse** is reported with the
  signal that identifies it — a surviving destination whose size and content
  hash match the restored source — and named as "a duplicate remains at
  `<dest>`; the original was restored". The previous version called it
  "occupied", which pointed the user at deleting the copy they had just
  recovered.

---

## Changes outside `undo.rs`

- `fsops` exports `stat_at`, `remove_at`, `rename_excl_at` and `ensure_root`,
  each over one `resolve_destination` walk. `rename_excl` and `clone_excl`
  become wrappers that take a descriptor on both sides.
- `stream_verified` hashes the source through the descriptor it read rather
  than re-opening the path, so the cross-device reverse verifies bytes rather
  than a name and reads the payload once.
- `Lock` uses `flock(2)`.

- `apply` writes `placed` after the destination is verified, for every
  mechanism that creates a new inode, in both modes, before any unlink.
- `apply` writes `failed` on its error path **only when nothing was
  created**. A failure after `placed` — a verified copy whose unlink then
  failed — leaves the row at `placed`, because that is the record `reverse`
  needs to clear the duplicate. Writing `failed` over it discards the one
  piece of information the state depends on, and leaves the duplicate on disk
  with `reverse` reporting the row as skipped.
- `apply` runs `ensure_parent` *after* `record_pending`, so a created series
  directory is recorded rather than left behind.
- `reverse` takes the plan path, not the manifest path, and derives the
  manifest from it — so it takes the same `<plan>.lock` `apply` does. Today
  they take two different files and do not serialise at all.
- An unknown status, mode or mechanism becomes a per-row refusal. Adding
  `placed` and `failed` to the vocabulary while unknown values still abort the
  file would add two more ways to strand a manifest.
