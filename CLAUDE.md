# Skill-Lint

> **★★★ CSE / Knowable Construction.** This repo operates under **Constructive Substrate Engineering** — canonical specification at [`pleme-io/theory/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md`](https://github.com/pleme-io/theory/blob/main/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md). The Compounding Directive (operational rules: solve once, load-bearing fixes only, idiom-first, models stay current, direction beats velocity) is in the org-level pleme-io/CLAUDE.md ★★★ section. Read both before non-trivial changes.

skill-lint — validate Claude Code skill maps

## The structural / freshness split

Checks fall in two families, and the split decides what may gate CI.

**Structural — always-on, gateable.** `discovery`, `version`, `sync`,
`frontmatter`, `map-integrity`, `listing-budget`, `path-resolution`. Each has an
objective right answer a machine checks and a human fixes without asserting
anything new about the world.

`claudemd-entry`, `claudemd-file` and `claudemd-chain` are structural too, but
they belong to the `claudemd` and `chain` subcommands rather than to `check` — they lint `CLAUDE.md` files, not
skills, and pretending a document is a skill directory would make the shared
context's field names lie.

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

## `claudemd` — the anti-regrowth seal

A `CLAUDE.md` is loaded whole into every session before the first token of work,
so its size is a standing tax on every task in the repository. The org file
reached 295,821 B; its `## ★★ Substrate primitive index` section alone reached
137,663 B — 46.5% of the file — while the section's OWN header declares the
contract every entry is meant to obey:

> Each line: **rule** + skill (if any) + long-form doc.

62 of 68 entries violated it. The contract was stated and never enforced, which
is the whole story: a sprint cut the file back, and without a gate it regrows
exactly as it grew the first time.

```
skill-lint claudemd --file docs/pleme-io-CLAUDE.md --baseline .claudemd-baseline
```

Three measurements, one verdict:

| What | Unit | Gated? |
|---|---|---|
| Each index-section entry | folded bytes, ceiling `--max-entry-bytes` (400) | yes |
| The whole file | raw bytes, ceiling `--max-file-bytes` (256 KiB) | yes |
| `skip-*` / `pending-*` / hard-imperative census | counts | **no** — a load signal |

**Folded, not raw, per entry.** Same normalization as
[`budget::fold`](src/budget.rs): runs of whitespace collapse to one space, so an
entry measures the same whether it was written on one line or hard-wrapped at 80
columns. Without folding the ceiling would be a rule about line-breaking. The
whole-FILE number is raw bytes, because that is literally what gets loaded.

**Bytes, not chars.** This corpus is dense with `★`, `—`, `§` and CJK; a char
count would systematically understate what an entry costs.

**The census is never a verdict.** There is no defensible threshold for "how
many waivers is too many", so the totals are printed and nothing else. A number
nobody prints is a number nobody watches.

**Coverage is part of the gate.** Which files were scanned is an OUTPUT of every
run, and a run over zero files exits non-zero — a linter wired into one
repository of seven is green because it never looked at the other six.

### The baseline is a ratchet, not an amnesty

The live file still carries ~63 over-ceiling entries. A gate that goes red on
every run is a gate that gets skipped, so known debt is recorded — **with the
size it had when recorded**:

```
skill-lint claudemd --file docs/pleme-io-CLAUDE.md --write-baseline .claudemd-baseline
```

```
file: docs/pleme-io-CLAUDE.md 282648
entry: docs/pleme-io-CLAUDE.md::OPERATING-THEORY 2893
```

Only two things fail: an over-ceiling item the baseline does not name, and a
baselined item that has **grown** past its recorded size. A pure allowlist would
let a baselined 1,400 B entry drift to 8,000 B in silence — which is precisely
the failure mode being sealed.

### The bullet-matching trap

Index entries are written `- **…` — **except** the ones written `- ★…`. A
scanner that matches only `- **` does not merely *miss* a star-prefixed entry: it
**welds** that entry's bytes onto the one above it, because an entry runs until
the next bullet the scanner recognizes. The original audit of the live file
reported an 8,434 B entry that did not exist — one real entry plus the 4,139 B
star-prefixed entry beneath it, fused by the matcher. A wrong number attributed
to a named entry is worse than silence, because it looks like a measurement.

The single live star-prefixed entry has since been normalized, so **the corpus
can no longer exercise this**. The fixtures in
`claudemd::tests::a_star_prefixed_entry_is_not_welded_onto_the_one_above_it` and
`claudemd_counts_a_star_prefixed_bullet_as_its_own_entry` are the only coverage
there is; deleting them silently removes the only thing standing between this
tool and the bug it was written to avoid repeating.

## `chain` — what one session actually loads

`claudemd` measures one file. Claude Code never loads one file: a session
started in directory `D` reads the global `~/.claude/CLAUDE.md`, every
`CLAUDE.md` from `$HOME` down to `D`, and every `@path` those files import,
recursively. Nothing measured that sum, so it regrew in silence — ~130k chars
for a session in `pleme-io/nix`, measured 2026-10-01.

```
skill-lint chain --dir ~/code/github/pleme-io/nix --dir ~ --max-bytes 150000
```

```
session ~/code/github/pleme-io/nix — 11 file(s), 133339 B
     33313 B  global  ~/.claude/CLAUDE.md
      2046 B  chain   ~/CLAUDE.md
       957 B  import    ~/code/github/pleme-io/nix/docs/knowledge/home/INDEX.md   <- ~/CLAUDE.md:58
      ...
     21204 B  chain   ~/code/github/pleme-io/nix/CLAUDE.md
     17498 B  import    ~/code/github/pleme-io/nix/docs/knowledge/nix/INDEX.md   <- ~/code/github/pleme-io/nix/CLAUDE.md:182
  total 133339 B of a 150000 B ceiling
```

**Load order.** `--global` (default `<home>/.claude/CLAUDE.md`), then, root-most
first, each directory's `CLAUDE.md`, `.claude/CLAUDE.md` and `CLAUDE.local.md`,
each file followed depth-first by what it imports. A file reached twice loads
once (keyed on its canonical path), so `~/.claude/CLAUDE.md` is not counted
again in a session started in `$HOME`.

**Imports.** `@path` at line start or after whitespace, in prose. `~/` resolves
against `--home`, an absolute path stands, anything else resolves against the
IMPORTING file's directory; `#fragment` is dropped. Inside a fence or an inline
code span it is an example, not an import.

**What fails.** A total over `--max-bytes`; a file over `--max-file-bytes`; an
import whose target is not a file (Claude Code skips it without a word, so a
dangling import looks exactly like a working one); an import past the 5-hop
limit (dropped the same way); a session that loads zero files (a vacuous
pass). Without the two ceilings the sizes are reported, not gated. A cycle is
reported and never fails: the second visit costs nothing.

**What it does not model.** Indented code blocks and HTML comments (an `@path`
there is read as an import — loud, never silently uncounted); directories
above `$HOME` (Claude Code walks to `/`; the walk stops at `--home` so fixtures
stay hermetic); import approval and file-type filters (every import counts, so
the total is a ceiling); `.claude/rules/`.

`--json` prints the same report as one JSON document: per session, `dir`,
`total_bytes`, the ceilings, `files` (`path`, `bytes`, `depth`, `origin` —
`global` / `chain` / `import` — `imported_by`, `line`), `cycles` and `errors`.

## `usage` — which skills are actually invoked

`budget` will not guess which descriptions the platform drops on overflow: that
order is by invocation frequency, and frequency was not on disk. `usage` puts it
there. `usage record` runs as a Claude Code hook (`PreToolUse` with matcher
`Skill`, and `UserPromptSubmit`) and appends one line per skill invocation to
`$XDG_STATE_HOME/skill-lint/usage.jsonl` (else `~/.local/state/…`):

```json
{"ts":"2026-10-01T14:33:05Z","event":"PreToolUse","skill":"bidama","trigger":"tool","session_id":"…","cwd":"/…","transcript_path":"/…","tool_input":{"skill":"bidama"}}
```

`trigger` is `tool` (the model called the `Skill` tool; the name is
`tool_input.skill`, measured from recorded transcripts, with `command` / `name`
read defensively and the raw `tool_input` kept) or `slash` (the prompt starts
with `/<name>` and `<name>` is a deployed `~/.claude/skills/<name>/SKILL.md`, so
`/clear` is not a skill). `transcript_path` and `tool_input` are omitted when
absent.

**The recorder never touches the session.** A hook's exit status and stdout
reach the agent: exit 2 blocks the tool call or erases the prompt, and a
`UserPromptSubmit` hook's stdout is added to the context. So every failure —
malformed input, an unwritable log, a panic, and a flag this binary does not
know — ends in exit 0 with nothing printed and nothing written. `usage record`
is dispatched before `Cli::parse` for that last one: clap exits 2 on a bad
argument. One `write` per line on an `O_APPEND` file, so concurrent sessions
interleave whole lines.

```
skill-lint usage report [--since 30d|12h|2w|all] [--skills-dir ~/.claude/skills] [--json]
```

Per skill in the window: total, slash, tool, last used. Then deployed skills
with no use in the window (retire/merge candidates, with their last use ever),
skills used but not deployed (plugins, retired ones), and the **drop order**:
every deployed skill least used first — never-seen before seen-long-ago — with
its listing cost and the cumulative chars freed, marking where the listing
would fit (`--budget-chars` / `--window-tokens`, as `budget`). A missing log is
zero events, not an error; an unreadable line is counted as malformed, never
silently dropped.
