# Skill-Lint

> **★★★ CSE / Knowable Construction.** This repo operates under **Constructive Substrate Engineering** — canonical specification at [`pleme-io/theory/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md`](https://github.com/pleme-io/theory/blob/main/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md). The Compounding Directive (operational rules: solve once, load-bearing fixes only, idiom-first, models stay current, direction beats velocity) is in the org-level pleme-io/CLAUDE.md ★★★ section. Read both before non-trivial changes.

skill-lint — validate Claude Code skill maps

## The structural / freshness split

Checks fall in two families, and the split decides what may gate CI.

**Structural — always-on, gateable.** `discovery`, `version`, `sync`,
`frontmatter`, `map-integrity`, `listing-budget`, `path-resolution`. Each has an
objective right answer a machine checks and a human fixes without asserting
anything new about the world.

**Freshness — opt-in (`--max-age-days`), advisory.** `staleness`, `references`.
Clearing one of these means a human actually re-read the skill. A bot bumping a
date to go green manufactures a false claim, which is worse than the stale one
it replaced — so a CI gate that forces that bump is a false-claim factory.

## `pending-path:` — the path-resolution waiver

`path-resolution` reads each `SKILL.md` body and resolves what it points at:
relative markdown links (`[text](./path)`, `[text](../path)`) against the
skill's own directory, and backticked `` `<repo>/<path>` `` against the root
holding sibling repositories — the latter only when that repository is present
locally, so an uncloned repo is silence, never a finding.

A target that is legitimately absent is *declared*, not silenced, with a line in
the skill body:

```
pending-path: sui/sui-store/src/postgres.rs — ships on the tiered-backend branch, unmerged
```

The waiver is scoped to the exact path it names; the trailing reason is for the
human reader. A waiver written inside a fenced code block is an example of a
waiver, not a waiver — fenced blocks are out of scope in both directions, so the
check never fires on the documentation OF a path.

`--skip-path-resolution` exists for the case where the answer is knowably
unavailable (linting a corpus away from the repositories it points into), not
for living with dead pointers.
