"""Mutation-test the CLI's own tests: break the code one string at a time and
check that something fails.

Run from the repository root: `python3 scripts/mutate-cli.py`. Each mutation is
applied, `cargo test -p comic-renamer` runs, and the file is restored --
including when a mutation does not compile.

A SKIP means the anchor text no longer exists, so that mutation tested nothing.
Treat it as a hole to fix, not a pass: it is how `size()` moving to
`types::human_bytes` quietly stopped being checked at all.
"""

import pathlib, subprocess, sys

CLI = "crates/cli/src/main.rs"
CORE_TYPES = "crates/core/src/types.rs"
DETAIL = "crates/cli/src/map/detail.rs"

# (name, file, old, new)
MUTS = [
 ("confirm always succeeds", CLI,
  '    if read == 0 || typed.trim() != word {', '    if false {'),
 ("confirm accepts any non-empty input", CLI,
  'if read == 0 || typed.trim() != word {', 'if read == 0 || typed.trim().is_empty() {'),
 ("EOF consents instead of aborting", CLI,
  'if read == 0 || typed.trim() != word {', 'if typed.trim() != word && read != 0 {'),
 ("disclosure is never printed", CLI,
  '    print!("{}", disclose(&facts));', '    let _ = disclose(&facts);'),
 ("disclosure is empty", CLI,
  'fn disclose(f: &Facts) -> String {\n    let mut out = String::from("\\n");',
  'fn disclose(f: &Facts) -> String {\n    if true { let _ = f; return String::new(); }\n    let mut out = String::from("\\n");'),
 ("--copy line quotes the reserve again", CLI,
  '            "\\n  --copy       keeps originals — clones where it can, {} free needed\\n",\n            size(f.bytes)',
  '            "\\n  --copy       keeps originals — clones where it can, {} free needed\\n",\n            size(f.reserve)'),
 ("undo refusals exit zero", CLI,
  '    Err(format!("{} rows could not be reversed", refused.len()))',
  '    Ok(())'),
 ("placeholders always report zero", CLI,
  '            if is_dataless(&row.source)? {', '            if false && is_dataless(&row.source)? {'),
 ("pre-flight check removed", CLI,
  '    apply::check(path, allow_cloud_move, &mut render_progress).map_err(|e| e.to_string())?;', '    '),
 ("--yes is ignored, always prompts", CLI,
  '    if !yes {\n        confirm(header.mode)?;', '    if true {\n        let _ = yes;\n        confirm(header.mode)?;'),
 ("not_ready line removed", CLI,
  '    if f.not_ready > 0 {', '    if false {'),
 ("Komga pre-flight skipped", CLI,
  '    if let Some(url) = komga_url {', '    if false { let url = komga_url.unwrap_or_default(); let url = &url;'),
 # Lives in core since T13 -- the CLI imports it as `size`. The test that
 # covers it is still in main.rs's `mod tests`, so the mutation belongs here.
 ("human_bytes always GiB, reviving 0.0 GiB", CORE_TYPES,
  '    for (scale, unit) in UNITS {\n        if bytes >= scale {',
  '    for (scale, unit) in UNITS.iter().take(1).copied() {\n        if true {'),
 ("plural() never pluralises", CLI,
  '    if n == 1 {\n        format!("1 {noun}")', '    if true {\n        format!("{n} {noun}")'),
 ("sentence box re-splits field edits", DETAIL,
  '        if self.structured {\n            return;\n        }\n        self.text.push(c);',
  '        self.text.push(c);'),
 ("a no-op pick reloads and wipes typed text", DETAIL,
  '        if self.picked == Some(at) {\n            return; // nothing moved: keep what the user typed\n        }', ''),
 ("flagged row accepts its own pre-fill", DETAIL,
  '        self.parsed != self.prefill || self.confirmed', '        true || self.confirmed'),
 ("another garbled candidate counts as verified", DETAIL,
  'c.is_unsplit() && as_parsed(c) == self.parsed', 'false && as_parsed(c) == self.parsed'),
 ("^R does not restore free text", DETAIL,
  '                self.structured = false;\n                Step::Editing', '                Step::Editing'),
]

skipped = survived = 0

for name, path, old, new in MUTS:
    F = pathlib.Path(path)
    ORIG = F.read_text()
    if old not in ORIG:
        print(f"SKIP  {name:48s} anchor not found in {path}")
        skipped += 1
        continue
    F.write_text(ORIG.replace(old, new, 1))
    r = subprocess.run(["cargo","test","-p","comic-renamer"],capture_output=True,text=True)
    out = r.stdout + r.stderr
    if "error[" in out or "error: could not compile" in out:
        print(f"BUILD {name:48s} did not compile"); F.write_text(ORIG); continue
    failed = sorted({l.split()[1] for l in out.splitlines()
                     if l.startswith("test ") and l.rstrip().endswith("FAILED")})
    if failed:
        print(f"CAUGHT {name:47s} {', '.join(failed)}")
    else:
        print(f"*** SURVIVED *** {name}")
        survived += 1
    F.write_text(ORIG)

# One known survivor: `Facts::gather`'s placeholder accumulation. SF_DATALESS is
# kernel-managed and chflags cannot set it, so no generated fixture is dataless.
print(f"\n{len(MUTS)} mutations: {len(MUTS)-survived-skipped} caught, "
      f"{survived} survived, {skipped} skipped")
if skipped:
    print("a SKIP tested nothing -- fix the anchor")
sys.exit(1 if skipped or survived > 1 else 0)
