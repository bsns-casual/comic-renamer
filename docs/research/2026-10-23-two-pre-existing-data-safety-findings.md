# Two pre-existing data-safety findings

Found by an adversarial review seat on 2026-10-23, during the final whole-branch
review of the mapping-first rebuild. **Neither is caused by that branch.**
`crates/core/src/undo.rs` is untouched by its 44 commits, and both paths are
reachable on `main` today.

They are recorded here rather than fixed in that branch because the first one's
fix changes D38's manifest contract, which is the owner's call, and because
folding a data-loss fix into a 46-file rebuild diff is how a careful fix becomes
an unreviewed one.

---

## 1. `undo` deletes a library file no run placed, and reports success

**Status:** reproduced, executed. The artifacts were inspected afterwards to
confirm the deleted file could not have come from a real run.

`undo` takes the plan path (D38) and derives the manifest beside it as
`<plan>.undo.csv`. `reverse_one` defends itself two ways:

- Invariant 2 applied in reverse: a destination is only recognised if the preset
  still produces it, so a hand-written manifest cannot name an arbitrary path
  (`undo.rs:251`).
- An exact identity check: the file at the destination must match the `size`,
  `inode` and `mtime_ns` the manifest recorded (`undo.rs:290`).

Both checks read from attacker-supplied data. The destination is recomputed from
the **plan's** metadata, and the plan is untrusted by design — invariant 2 exists
because plans get handed to AI assistants and attached to public PRs. The
identity is read from the **manifest**, which arrives in the same directory as
the plan, by naming convention. So the identity check compares the file against
a number whoever wrote the manifest chose.

The reproduction was a plan in `mode: copy`, `mechanism: clone`, with one row
naming `Hellboy/Hellboy Vol. 01.cbz`, and a hand-written manifest whose `placed`
row carried that file's real identity. Reversing a copy means deleting the copy,
so `undo` deleted it and printed `restored: 1, refused: []`.

The deleted file was 33 bytes. The plan's source was 5 bytes. No clone or stream
of that source could have produced it, which is the whole claim: the file was
never placed by any run.

Compounding it, `undo` discards the edited-plan signal its own read returns
(`apply.rs:354`, `let (header, rows, _edited)`), so the plan that anchors the
manifest is not even reported as having been touched since the scan. `apply`
does report it (`apply.rs:81`, and `Facts::gather` takes it); `undo` drops it.

**What a fix has to decide:** where the trusted record of a run lives. Today it
is a CSV beside a plan, and both are as trustworthy as the directory they sit
in. Options worth weighing: a manifest the tool can authenticate as its own; a
manifest kept somewhere a handed-over plan does not travel to; or `undo`
requiring the destination's identity to match a record it can tie to a run it
performed. Each changes D38.

## 2. The typed confirmation gates a plan that `apply` re-reads

The plan is read three times, and the prompt sits between the second read and the
third:

1. `main.rs:363` reads it for the disclosure's arithmetic. Its own comment says
   "this copy is never what decides anything", which is the defect stated as a
   reassurance: this is the copy the user is shown.
2. `apply::check` re-reads it (`apply.rs:81`), takes the lock at `apply.rs:76`,
   and **releases the lock when it returns**.
3. The user types `MOVE` or `COPY`. Then `apply::run` re-reads the plan and takes
   the lock again.

Nothing holds the lock across the prompt, and `plan::write` takes no lock at all
— the only two `Lock::acquire` calls in the tree are `apply.rs:76` and
`apply.rs:353`. So a second terminal running `scan` is enough: `scan` and `apply`
both default to `comic-renamer-plan.jsonl`, and an ordinary concurrent use of the
tool, with no adversary at all, substitutes a different plan under a confirmation
the user gave for the first one.

Invariant 2 limits the damage: `apply` recomputes every destination through the
preset, so the substituted plan cannot write outside the library layout. What the
user consented to, though, is not what runs.

**What a fix has to decide:** whether consent binds to the plan's bytes. Holding
the lock across the prompt is the smallest version and closes the concurrent-
`scan` case on its own. Executing from the copy the disclosure was computed from
goes further. Re-reading after the prompt and comparing the checksum is the
version that *reports* the substitution rather than silently preferring one read
— and `plan::read_allowing_edits` already returns the signal that would carry it.

---

## Not a finding, but the maintainer's call

Whether `DIE: Loaded` and `Glarien: Origins` are their own series or arcs within
`DIE` and `Glarien`. The mapping table can record either; nothing in the code
prefers one.
