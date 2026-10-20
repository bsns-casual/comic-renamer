#!/usr/bin/env python3
"""Compare corpus/manifest.json against upstream.

--write updates the manifest in place when upstream is newer.
Emits `changed=`, `old_date=`, `new_date=` for GITHUB_OUTPUT on stdout.
"""
import json, sys, urllib.request

def main(argv):
    write = "--write" in argv
    path = "corpus/manifest.json"
    ours = json.load(open(path))
    with urllib.request.urlopen(ours["upstreamManifest"], timeout=60) as r:
        up = json.load(r)

    changed = ours["sha256"] != up.get("sha256")
    print("changed=%s" % ("true" if changed else "false"))
    print("old_date=%s" % ours["dumpDate"])
    print("new_date=%s" % up.get("dumpDate", ""))

    if changed and write:
        for k in ("dumpDate", "url", "sha256", "sizeBytes"):
            if k in up:
                ours[k] = up[k]
        with open(path, "w") as f:
            json.dump(ours, f, indent=2)
            f.write("\n")
        print("manifest updated to %s" % up.get("dumpDate"), file=sys.stderr)
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
