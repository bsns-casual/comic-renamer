#!/usr/bin/env node
/**
 * Checks the review page's path renderer against the Komga preset.
 *
 *     node scripts/check-review-page.cjs
 *
 * The page shows the destination each decision produces, so it mirrors
 * `crates/core/src/preset/komga.rs` in JavaScript. Invariant 1 is not at
 * stake — the page constructs no path the tool acts on, and inside the binary
 * the preset is still the sole constructor — but an unpinned mirror that
 * drifts shows the wrong path to someone making 2,204 decisions.
 *
 * Every expectation below is copied from a Rust test in
 * `crates/core/tests/preset_komga.rs`. When one changes there, this fails.
 *
 * Local, not CI: the workflow installs no Node. Run it after touching either
 * side.
 */

const fs = require("node:fs");
const path = require("node:path");

const PAGE = path.join(__dirname, "review-page.html");

// (series, volume, issue, subtitle, expected) — volume and issue are the
// strings the page's inputs hold, which is why "" means absent.
const CASES = [
  ["Bee and PuppyCat", "", "1", "", "Bee and PuppyCat/Bee and PuppyCat 0001.cbz"],
  ["Spawn", "", "0", "", "Spawn/Spawn 0000.cbz"],
  [
    "Love and Rockets",
    "1",
    "1",
    "Facsimile Edition",
    "Love and Rockets/Love and Rockets Vol. 01 0001 - Facsimile Edition.cbz",
  ],
  ["Saga", "3", "", "", "Saga/Saga Vol. 03.cbz"],
  ["Hack/Slash", "", "", "Back to School", "Hack - Slash/Hack - Slash - Back to School.cbz"],
  ["Archie", "", "635", "", "Archie/Archie 0635.cbz"],
  ["DIE", "", "1", "Loaded", "DIE/DIE 0001 - Loaded.cbz"],
  ["Nemesis: Rogues' Gallery", "", "", "", "Nemesis - Rogues' Gallery/Nemesis - Rogues' Gallery.cbz"],
  ["Hellboy.", "", "", "", "Hellboy/Hellboy.cbz"],
  ["Free​Issue", "1", "", "", "FreeIssue/FreeIssue Vol. 01.cbz"],
  ["A B", "", "", "", "A B/A B.cbz"],
  // The documented same-leaf pair, kept apart only by the series folder.
  ["Elephantmen 2260", "", "", "", "Elephantmen 2260/Elephantmen 2260.cbz"],
  ["Elephantmen", "", "2260", "", "Elephantmen/Elephantmen 2260.cbz"],
];

function load() {
  const html = fs.readFileSync(PAGE, "utf8");
  const open = html.indexOf("<script>");
  const close = html.lastIndexOf("</script>");
  if (open < 0 || close < 0) {
    throw new Error(`${PAGE} has no <script> block`);
  }
  const script = html.slice(open + "<script>".length, close);
  const from = script.indexOf("  // Built from code points");
  const to = script.indexOf("const el = (id)");
  if (from < 0 || to < 0 || to < from) {
    throw new Error("the page's path helpers are not where this check expects them");
  }
  const body = 'const EXT = "cbz";' + script.slice(from, to);
  return new Function(`${body}; return { komgaPath };`)();
}

let bad = 0;
const { komgaPath } = load();
for (const [series, volume, issue, subtitle, want] of CASES) {
  const p = komgaPath(series, volume, issue, subtitle);
  const got = `${p.folder}/${p.leaf}`;
  if (got !== want) {
    bad += 1;
    console.error(`FAIL ${JSON.stringify([series, volume, issue, subtitle])}`);
    console.error(`  got  ${got}`);
    console.error(`  want ${want}`);
  }
}

const named = komgaPath("Elephantmen 2260", "", "", "");
const issued = komgaPath("Elephantmen", "", "2260", "");
if (named.leaf !== issued.leaf) {
  bad += 1;
  console.error("FAIL the two leaves should be identical; this pair is the collision case");
}
if (named.folder === issued.folder) {
  bad += 1;
  console.error("FAIL only the series folder separates them, and it did not");
}

if (bad) {
  console.error(`${bad} mismatch(es) against crates/core/src/preset/komga.rs`);
  process.exit(1);
}
console.log(`review page path renderer agrees with the Komga preset (${CASES.length} cases)`);
