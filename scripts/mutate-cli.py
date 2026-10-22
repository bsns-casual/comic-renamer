import pathlib, subprocess, sys
F = pathlib.Path("crates/cli/src/main.rs")
ORIG = F.read_text()

MUTS = [
 ("confirm always succeeds",
  '    if read == 0 || typed.trim() != word {', '    if false {'),
 ("confirm accepts any non-empty input",
  'if read == 0 || typed.trim() != word {', 'if read == 0 || typed.trim().is_empty() {'),
 ("EOF consents instead of aborting",
  'if read == 0 || typed.trim() != word {', 'if typed.trim() != word && read != 0 {'),
 ("disclosure is never printed",
  '    print!("{}", disclose(&facts));', '    let _ = disclose(&facts);'),
 ("disclosure is empty",
  'fn disclose(f: &Facts) -> String {\n    let mut out = String::from("\\n");',
  'fn disclose(f: &Facts) -> String {\n    if true { let _ = f; return String::new(); }\n    let mut out = String::from("\\n");'),
 ("--copy line quotes the reserve again",
  '            "\\n  --copy       keeps originals — clones where it can, {} free needed\\n",\n            size(f.bytes)',
  '            "\\n  --copy       keeps originals — clones where it can, {} free needed\\n",\n            size(f.reserve)'),
 ("undo refusals exit zero",
  '    Err(format!("{} rows could not be reversed", refused.len()))',
  '    Ok(())'),
 ("placeholders always report zero",
  '            if is_dataless(&row.source)? {', '            if false && is_dataless(&row.source)? {'),
 ("pre-flight check removed",
  '    apply::check(path, allow_cloud_move, &mut render_progress).map_err(|e| e.to_string())?;', '    '),
 ("--yes is ignored, always prompts",
  '    if !yes {\n        confirm(header.mode)?;', '    if true {\n        let _ = yes;\n        confirm(header.mode)?;'),
 ("not_ready line removed",
  '    if f.not_ready > 0 {', '    if false {'),
 ("Komga pre-flight skipped",
  '    if let Some(url) = komga_url {', '    if false { let url = komga_url.unwrap_or_default(); let url = &url;'),
 ("size() always GiB, reviving 0.0 GiB",
  '    for (scale, unit) in UNITS {\n        if bytes >= scale {',
  '    for (scale, unit) in UNITS.iter().take(1).copied() {\n        if true {'),
 ("plural() never pluralises",
  '    if n == 1 {\n        format!("1 {noun}")', '    if true {\n        format!("{n} {noun}")'),
]

for name, old, new in MUTS:
    if old not in ORIG:
        print(f"SKIP  {name:48s} anchor not found"); continue
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
    F.write_text(ORIG)
F.write_text(ORIG)
