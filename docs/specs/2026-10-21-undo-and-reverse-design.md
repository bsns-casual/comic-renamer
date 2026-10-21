# Undo and `reverse` — design

**Date:** 2026-10-21
**Status:** proposed, not implemented. Written because two implementations
failed review.
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
7. **One lock serves the plan**, taken by `apply` and by `reverse` alike.

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

---

## Record format

Append-only. Header line, then a root record, then one record per status
transition. RFC 4180 quoting, doubled embedded quotes, every path field
prefixed with `'` unconditionally.

```
seq,status,mode,mechanism,source_name,dest_relative,size,inode
0,roots,,,'<source_root>,'<dest_root>,,
1,pending,move,stream,'mazebook.pdf,'Mazebook/Mazebook.pdf,4096,8814
1,placed,,,,,4096,9201
1,done,,,,,,
```

### Statuses

The second implementation had `pending` and `done`, which made a bare
`pending` ambiguous between *crashed mid-operation* and *refused at the
syscall*. `reverse` guessed between them by matching on size, and that guess
is what reopened the two worst findings.

| status | meaning | what `reverse` does |
|---|---|---|
| `roots` | the two roots for this run | validated, then every path is resolved beneath them |
| `pending` | about to attempt; source identity recorded | identity unknown — refuse unless the destination is byte-identical to a surviving source |
| `failed` | attempted, nothing created | skip |
| `placed` | destination exists and is verified; its identity recorded | the destination is ours: remove it, and for a move the source may still be present |
| `done` | complete; for a move the source is gone | reverse normally |

`placed` is the record finding 6 needed and neither implementation had. A
streamed move that verifies the copy and then fails to unlink the source sits
at `placed`, unambiguously — and removing the destination is then provably
safe rather than inferred. The reviewer's point stands that `failed` is the
wrong shape for "succeeded, then half-failed"; `placed` is that shape.

A rename goes `pending` → `done` with no `placed`: it is atomic and the inode
does not move.

---

## What `reverse` does, per row

1. Resolve both roots. Refuse the whole manifest only if the `roots` record is
   missing or either root is not a directory.
2. Reject a `source_name` that is not a single normal component, or a
   `dest_relative` containing anything but normal components.
3. Stat the destination with `symlink_metadata`. `NotFound` is *nothing to
   do*; any other error is a refusal naming the path, because
   `Path::exists()` answers `false` on `EACCES`.
4. Require exact identity. `placed` and `done` carry the destination's own
   size and inode. A bare `pending` carries only the source's, so unless the
   mechanism is `rename` — where the inode is preserved and the match is still
   exact — the row is **refused**, not matched on size.
5. For `copy`: stat the source. **If the source is absent, refuse** — deleting
   the destination would destroy the last copy. This is the one path an
   ordinary user reaches without a crash or an edit: copy a bundle in, delete
   the bundle folder, undo.
6. For `move`: if the source is present and identical to what was recorded,
   this is `placed`'s duplicate — remove the destination. If it is present and
   different, refuse as occupied. If it is absent, restore it, creating the
   parent first (`ensure_parent`), via `rename_excl`, falling back to
   `stream_verified` + unlink on `CrossesDevices`.
7. Report. `UndoReport { restored, skipped, refused }`, where every refusal
   names the path and the reason.

---

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
- **A duplicate left by a failed cross-device reverse.** Streaming back
  produces a new inode, so a failed unlink leaves a row matching no recorded
  identity. It is reported, and reported accurately — the previous version
  called it "occupied", which pointed the user at deleting the copy they had
  just recovered.
- **Non-UTF-8 filenames.** Paths go through `to_string_lossy`, so a name that
  is not valid UTF-8 is recorded with replacement characters. APFS rejects
  such names; this is reachable only on exFAT, SMB or NFS destinations, and is
  out of Phase 1's scope.
- **A stale lock.** It is reported with its path so a user can remove it. It
  is not auto-cleared: a lock whose owner is still running is the thing it
  exists to prevent, and a pid check cannot distinguish a dead owner from a
  recycled pid.

---

## Changes outside `undo.rs`

- `apply` writes `failed` on its error path and `placed` between the
  destination landing and the source being unlinked.
- `apply` runs `ensure_parent` *after* `record_pending`, so a created series
  directory is recorded rather than left behind.
- `reverse` takes the plan path, not the manifest path, and derives the
  manifest from it — so it takes the same `<plan>.lock` `apply` does. Today
  they take two different files and do not serialise at all.
- An unknown status, mode or mechanism becomes a per-row refusal. Adding
  `placed` and `failed` to the vocabulary while unknown values still abort the
  file would add two more ways to strand a manifest.
