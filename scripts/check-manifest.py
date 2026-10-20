#!/usr/bin/env python3
"""Validate corpus/manifest.json. Exits non-zero on any problem."""
import json, re, sys

REQUIRED = {"schemaVersion", "dumpDate", "url", "sha256", "sizeBytes",
            "seriesCount", "attribution", "licence", "upstreamManifest"}

def main(path="corpus/manifest.json"):
    m = json.load(open(path))
    problems = []
    missing = REQUIRED - set(m)
    if missing:
        problems.append("missing fields: %s" % sorted(missing))
    if not re.fullmatch(r"[0-9a-f]{64}", str(m.get("sha256", ""))):
        problems.append("sha256 is not 64 lowercase hex characters")
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", str(m.get("dumpDate", ""))):
        problems.append("dumpDate is not YYYY-MM-DD")
    elif m["dumpDate"] not in str(m.get("url", "")):
        problems.append("url does not contain dumpDate -- pin may be inconsistent")
    if m.get("licence") != "CC BY-SA 4.0":
        problems.append("corpus licence must remain CC BY-SA 4.0 (its source requires it)")
    if "comics.org" not in str(m.get("attribution", "")):
        problems.append("Grand Comics Database attribution is required")
    if not isinstance(m.get("sizeBytes"), int) or m["sizeBytes"] <= 0:
        problems.append("sizeBytes must be a positive integer")

    for p in problems:
        print("error: %s" % p, file=sys.stderr)
    if problems:
        return 1
    print("manifest OK: dump %s, %s series, %d bytes"
          % (m["dumpDate"], m["seriesCount"], m["sizeBytes"]))
    return 0

if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:]))
