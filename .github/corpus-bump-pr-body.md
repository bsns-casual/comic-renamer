Upstream published a newer Grand Comics Database extract, and this pull request
bumps `corpus/manifest.json` to match.

**Do not merge on the hash check alone.** A corpus bump changes resolution
behaviour for every user, so per design §6.5 and decision D19 the coverage delta
belongs in this pull request before it merges.

- [ ] Held-out located-rate, before and after. Must not regress below 92.3%.
- [ ] Every row whose resolved series changed, listed.
- [ ] Silent-false-positive check — **only once the labelled subset from D26
      exists.** Until then, say it was not checked rather than implying it passed.
