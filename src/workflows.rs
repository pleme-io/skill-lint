//! GitHub Actions workflow linting — the no-shell seal at the shape that hides.
//!
//! The fleet PRIME DIRECTIVE forbids shell beyond ~3 lines of inline glue. The
//! form most often missed is not a `.sh` file — those get noticed — it is shell
//! inline in an Actions `run:` block, because **it does not look like a shell
//! script. It looks like YAML with a long string in it.**
//!
//! On 2026-08-06 an agent wrote a ~15-line `run:` — an embedded `nix eval`, a
//! `grep -qx`, an if/else — into substrate's `rust-auto-release.yml` DURING the
//! session it spent enforcing that very rule. The rule was written down in two
//! places and still lost. It was unenforced, not weak: nothing read the YAML and
//! asked how many lines of shell were in it.
//!
//! # What counts, and what deliberately does not
//!
//! A violation is a **block-form** `run:` — `|`, `>`, `|-`, `>-`, and the
//! indentation-indicator spellings — whose body exceeds N non-blank,
//! non-comment lines. Default N = 3, the org rule's glue allowance.
//!
//! **Flow form is never a violation.** `run: cargo build` is the target shape:
//! one invocation of a typed tool. A gate that flagged it would be arguing
//! against its own destination.
//!
//! **Comments do not count.** Explaining WHY at the call site is fleet house
//! style — the workflows in substrate carry long, dated rationale blocks above
//! and inside their steps — and a line-count gate that taxed comments would
//! teach exactly the wrong lesson: delete the explanation, keep the shell. The
//! measured quantity is the number of lines of SHELL, not the size of the step.
//!
//! # Keying, and its trade-off
//!
//! A baseline entry has to survive an edit ABOVE a block — insert a comment at
//! the top of the file and every line number below it moves — while a block that
//! GROWS must still re-fail. Those two requirements rule out the two obvious
//! keys:
//!
//! - **the line number** breaks on the exact edit the requirement names;
//! - **a digest of the body** survives motion perfectly but changes on ANY body
//!   edit, including a SHRINK — which would report a step that got better as a
//!   brand-new violation, defeating the ratchet.
//!
//! So the key is `<file>::<job>/<step label>`, plus `#N` in document order when
//! two steps in one job share a label, and the RECORDED VALUE is the line count.
//! Identity comes from position in the workflow's structure; growth comes from
//! comparing counts. The label is the step's `name:` when it has one, and its
//! first line of shell when it does not.
//!
//! The trade-off, stated: this key is stable under edits above the block, under
//! edits to the body, and under moving a whole step within its job. It is NOT
//! stable under **renaming the step** — a rename reads as a new violation. That
//! is a deliberate choice rather than a defect: renaming a step is an explicit
//! act, re-recording costs one flag, and the alternative (keying on the body) is
//! unstable under the far more common act of editing the shell. For an
//! **unnamed** step the label is its first command, so editing that first line
//! also breaks the key — which is one more reason to name steps, and the report
//! prints the label so the drift is visible rather than mysterious.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::budget::fold;
use crate::error::{CheckKind, LintError};
use crate::ratchet::{Ratchet, Verdict, parse_lines};

/// Non-blank, non-comment lines a block-form `run:` body may hold.
///
/// Three, because that is the org rule's own allowance for "inline glue" — not a
/// number reverse-engineered from the corpus. A default derived from what the
/// fleet currently weighs would ratify the fleet's current debt.
pub const DEFAULT_MAX_RUN_LINES: usize = 3;

/// Longest label fragment kept in a key.
const MAX_LABEL_BYTES: usize = 60;

/// Baseline line kind for a run step.
const RATCHET_KIND_RUN: &str = "run";

// ═══════════════════════════════════════════════════════════════════
// WorkflowSource — the I/O seam
// ═══════════════════════════════════════════════════════════════════

/// One workflow to lint: a stable key plus its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    /// Invocation-path-independent identifier, used in reports and baselines.
    pub key: String,
    /// Full file content.
    pub content: String,
}

/// Where workflows come from. Mirrors [`crate::claudemd::DocSource`]: production
/// reads the filesystem, tests hand over strings.
pub trait WorkflowSource {
    /// Load every workflow to lint.
    ///
    /// # Errors
    ///
    /// Returns an error if any named file cannot be read. A file that is not
    /// there is a hard error, never a skip — silently dropping an unreadable
    /// file is how a linter comes to cover fewer files than its operator
    /// believes it covers.
    fn workflows(&self) -> anyhow::Result<Vec<Workflow>>;

    /// Human-readable description of what was asked for.
    fn origin(&self) -> String;
}

/// Filesystem-backed [`WorkflowSource`].
pub struct FsWorkflowSource<'a> {
    /// Paths to lint, in the order given.
    pub paths: &'a [PathBuf],
}

impl WorkflowSource for FsWorkflowSource<'_> {
    fn workflows(&self) -> anyhow::Result<Vec<Workflow>> {
        self.paths
            .iter()
            .map(|path| {
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?;
                Ok(Workflow { key: workflow_key(path), content })
            })
            .collect()
    }

    fn origin(&self) -> String {
        if self.paths.is_empty() {
            return "<no files given>".to_owned();
        }
        self.paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    }
}

/// Derive a workflow's report/baseline key from its path.
///
/// The bare file name. Unlike a `CLAUDE.md` — where the name alone is ambiguous
/// and the parent directory carries the meaning — every workflow lives in
/// `.github/workflows/`, so the parent component is the same constant string for
/// every file in the corpus and would only pad the key.
#[must_use]
pub fn workflow_key(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

// ═══════════════════════════════════════════════════════════════════
// Parsing
// ═══════════════════════════════════════════════════════════════════

/// One block-form `run:` step found in a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStep {
    /// `<file>::<job>/<label>` — what a baseline names.
    pub key: String,
    /// Enclosing job id, or `<no job>` for a composite action's step list.
    pub job: String,
    /// The step's `name:`, or its first line of shell when unnamed.
    pub label: String,
    /// 1-based line of the `run:` header, so a finding is navigable.
    pub line: usize,
    /// Non-blank, non-comment body lines — the measured quantity.
    pub shell_lines: usize,
    /// Every body line, comments and blanks included. Reported for orientation:
    /// the difference between the two numbers is how much of the step is
    /// explanation rather than shell.
    pub body_lines: usize,
    /// Block style as written: `|` (literal) or `>` (folded).
    pub style: char,
}

/// Everything one pass over a workflow yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowScan {
    /// Report/baseline key.
    pub key: String,
    /// Block-form `run:` steps, in document order.
    pub runs: Vec<RunStep>,
    /// Flow-form `run:` steps — `run: cargo build`. Counted and reported
    /// because they are the TARGET shape, never a finding.
    pub flow_runs: usize,
}

/// A parsed mapping-key line.
struct KeyLine<'a> {
    /// Column the key itself starts at, past any `- ` sequence marker.
    indent: usize,
    /// Column the `-` starts at, when this line opens a sequence item.
    item_indent: Option<usize>,
    /// The key, unquoted.
    key: &'a str,
    /// Everything after the colon, trimmed.
    value: &'a str,
}

/// Parse a line as `key: value`, or `- key: value`.
///
/// Returns `None` for anything that is not a mapping key at the start of a line
/// — which is every line of a block scalar's body, and every continuation line
/// of a multi-line plain scalar.
fn key_line(line: &str) -> Option<KeyLine<'_>> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();
    let (indent, item_indent, rest) = match rest.strip_prefix("- ") {
        Some(after) => {
            let extra = after.len();
            let after_trimmed = after.trim_start();
            (indent + 2 + (extra - after_trimmed.len()), Some(indent), after_trimmed)
        }
        None => (indent, None, rest),
    };

    let key_len = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .map(char::len_utf8)
        .sum::<usize>();
    if key_len == 0 || !rest[key_len..].starts_with(':') {
        return None;
    }
    Some(KeyLine { indent, item_indent, key: &rest[..key_len], value: rest[key_len + 1..].trim() })
}

/// Is this value a YAML block-scalar header, and in which style?
///
/// Accepts `|`, `>` and every legal decoration: a chomping indicator (`-`/`+`),
/// an indentation indicator (a digit), either order, plus a trailing comment.
/// Rejecting `|-` or `>2-` here would leave the exact spelling a careful author
/// reaches for as the one the gate cannot see.
#[must_use]
pub fn block_scalar_style(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let style = match chars.next()? {
        '|' => '|',
        '>' => '>',
        _ => return None,
    };
    let rest: String = chars.collect();
    let after_indicators = rest.trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+'));
    let tail = after_indicators.trim();
    (tail.is_empty() || tail.starts_with('#')).then_some(style)
}

/// Non-blank, non-comment lines of a block body — the measured quantity.
fn shell_lines(body: &[&str]) -> usize {
    body.iter()
        .filter(|line| {
            let t = line.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .count()
}

/// First line of actual shell in a body, folded — the fallback label for a step
/// that has no `name:`.
fn first_shell_line(body: &[&str]) -> String {
    body.iter()
        .map(|l| l.trim())
        .find(|t| !t.is_empty() && !t.starts_with('#'))
        .map_or_else(|| "<empty>".to_owned(), fold)
}

/// Truncate a label at a char boundary, so a key stays printable.
fn clip(label: &str) -> String {
    let label = fold(label.trim().trim_matches(|c| matches!(c, '"' | '\'')));
    if label.len() <= MAX_LABEL_BYTES {
        return if label.is_empty() { "<unlabelled>".to_owned() } else { label };
    }
    let mut end = MAX_LABEL_BYTES;
    while end > 0 && !label.is_char_boundary(end) {
        end -= 1;
    }
    label[..end].trim_end().to_owned()
}

/// Scan one workflow for `run:` steps.
///
/// # Why this is a line scanner and not a YAML deserialization
///
/// The finding has to carry a LINE NUMBER, and a serde round-trip through
/// `serde_yaml` discards the position of everything it parses. It would also
/// force a choice about malformed input: a workflow that fails to deserialize
/// would either abort the whole run or be silently skipped, and "silently
/// skipped" is how a linter comes to cover less than its operator believes.
/// A scanner reports what it can read and never has that failure mode.
///
/// Every block scalar is consumed whole — not only `run:` ones — because a
/// `script: |` or a heredoc that WRITES a workflow both contain lines that read
/// exactly like structure. Consuming the block is what keeps a `run: |` shown
/// inside another block from being counted as a second step.
#[must_use]
pub fn scan_workflow(file_key: &str, content: &str) -> WorkflowScan {
    let lines: Vec<&str> = content.lines().collect();
    let mut runs: Vec<RunStep> = Vec::new();
    let mut flow_runs = 0usize;
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();

    let mut in_jobs = false;
    let mut job: Option<String> = None;
    // The column the `steps:` KEY sits at, versus the column a step's own FIELDS
    // sit at. Two different columns doing two different jobs — the first bounds
    // the block we are inside, the second is what a `name:` must match to be
    // THIS step's name rather than the job's or the workflow's.
    let mut steps_col: Option<usize> = None;
    let mut field_col: Option<usize> = None;
    let mut step_name: Option<String> = None;

    let mut index = 0;
    while index < lines.len() {
        let Some(kl) = key_line(lines[index]) else {
            index += 1;
            continue;
        };

        // ── structure: which job, which step ──────────────────────────────
        if kl.indent == 0 && kl.item_indent.is_none() {
            in_jobs = kl.key == "jobs";
            job = None;
            steps_col = None;
            field_col = None;
            step_name = None;
        } else if in_jobs && kl.indent == 2 && kl.item_indent.is_none() {
            job = Some(kl.key.to_owned());
            steps_col = None;
            field_col = None;
            step_name = None;
        }
        if steps_col.is_some_and(|col| kl.indent <= col) && kl.key != "steps" {
            steps_col = None;
            field_col = None;
            step_name = None;
        }
        if kl.key == "steps" {
            steps_col = Some(kl.indent);
            field_col = None;
            step_name = None;
        }
        // A sequence item indented past `steps:` opens a new step.
        if let (Some(item), Some(col)) = (kl.item_indent, steps_col)
            && item > col
        {
            field_col = Some(kl.indent);
            step_name = None;
        }
        if kl.key == "name" && field_col == Some(kl.indent) && !kl.value.is_empty() {
            step_name = Some(kl.value.to_owned());
        }

        // ── the value: block scalar, flow scalar, or a nested mapping ─────
        let Some(style) = block_scalar_style(kl.value) else {
            if kl.key == "run" && !kl.value.is_empty() {
                flow_runs += 1;
            }
            index += 1;
            continue;
        };

        // A blank line never ends a block scalar; only a non-blank line whose
        // indentation has returned to the key's own column or further left.
        let mut end = index + 1;
        while end < lines.len() {
            let raw = lines[end];
            if raw.trim().is_empty() {
                end += 1;
                continue;
            }
            if raw.len() - raw.trim_start().len() <= kl.indent {
                break;
            }
            end += 1;
        }
        let body = &lines[index + 1..end];

        if kl.key == "run" {
            let label = clip(
                &step_name.clone().unwrap_or_else(|| first_shell_line(body)),
            );
            let job_name = job.clone().unwrap_or_else(|| "<no job>".to_owned());
            // Two steps in one job may legitimately share a label; the key must
            // still be unique or one baseline line would cover both.
            let base = format!("{job_name}/{label}");
            let count = seen.entry(base.clone()).or_default();
            *count += 1;
            let label_key = if *count > 1 { format!("{base} #{count}") } else { base };
            runs.push(RunStep {
                key: format!("{file_key}::{label_key}"),
                job: job_name,
                label,
                line: index + 1,
                shell_lines: shell_lines(body),
                body_lines: body.len(),
                style,
            });
        }

        index = end;
    }

    WorkflowScan { key: file_key.to_owned(), runs, flow_runs }
}

// ═══════════════════════════════════════════════════════════════════
// Baseline
// ═══════════════════════════════════════════════════════════════════

/// Known inline-shell debt: step key → line count when recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    /// The ratchet itself.
    pub runs: Ratchet,
}

impl Baseline {
    /// Parse a baseline file. Grammar is [`crate::ratchet::parse_lines`]'s.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut baseline = Self::default();
        for (kind, key, size) in parse_lines(text) {
            if kind == RATCHET_KIND_RUN {
                baseline.runs.insert(key, size);
            }
        }
        baseline
    }

    /// Load a baseline from disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read. A missing baseline is an
    /// error, not an empty baseline: treating "cannot read the baseline" as
    /// "there is no debt" turns a typo in a CI path into a gate that reports
    /// every pre-existing violation as new, which gets the gate disabled.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading baseline {}", path.display()))?;
        Ok(Self::parse(&text))
    }

    /// Render the current violations as a baseline file.
    #[must_use]
    pub fn render(scans: &[WorkflowScan], config: &WorkflowConfig) -> String {
        let mut out = String::from(
            "# skill-lint workflows baseline — known inline shell, recorded at its\n\
             # measured line count. A baselined step may SHRINK or hold, never grow:\n\
             # exceeding the recorded count fails the gate. Regenerate deliberately\n\
             # with `skill-lint workflows --write-baseline`.\n",
        );
        for scan in scans {
            for run in &scan.runs {
                if run.shell_lines > config.max_run_lines {
                    // `write!` into a String is infallible; the Result is only
                    // there because Write is one trait for files and buffers.
                    let _ = writeln!(out, "{RATCHET_KIND_RUN}: {} {}", run.key, run.shell_lines);
                }
            }
        }
        out
    }
}

// ═══════════════════════════════════════════════════════════════════
// Config, context, checkers
// ═══════════════════════════════════════════════════════════════════

/// Configuration for a `workflows` run.
#[derive(Debug, Clone)]
pub struct WorkflowConfig {
    /// Non-blank, non-comment body lines a block-form `run:` may hold.
    pub max_run_lines: usize,
}

impl Default for WorkflowConfig {
    fn default() -> Self { Self { max_run_lines: DEFAULT_MAX_RUN_LINES } }
}

/// Shared context built once, passed to every checker.
#[must_use]
pub struct WorkflowContext {
    /// One scan per workflow, in the order given.
    pub scans: Vec<WorkflowScan>,
    /// What was asked for — reported when nothing was found.
    pub origin: String,
    /// Known debt.
    pub baseline: Baseline,
}

/// A single composable check over workflows.
pub trait WorkflowChecker {
    /// The check category this checker belongs to.
    fn kind(&self) -> CheckKind;
    /// Run validation, appending any errors found.
    fn check(&self, ctx: &WorkflowContext, errors: &mut Vec<LintError>);
}

/// Refuses a run that linted nothing.
///
/// The coverage half of the gate, and the same rule
/// [`LintError::NoSkillsFound`] and [`LintError::NoDocsScanned`] already state:
/// a linter wired into one repository of seven is green because it never looked
/// at the other six.
pub struct WorkflowDiscoveryChecker;

impl WorkflowChecker for WorkflowDiscoveryChecker {
    fn kind(&self) -> CheckKind { CheckKind::Discovery }

    fn check(&self, ctx: &WorkflowContext, errors: &mut Vec<LintError>) {
        if ctx.scans.is_empty() {
            errors.push(LintError::NoWorkflowsScanned {
                kind: CheckKind::Discovery,
                searched: ctx.origin.clone(),
            });
        }
    }
}

/// Enforces the inline-shell line limit, against the baseline ratchet.
pub struct RunLengthChecker {
    /// Line limit.
    pub cap: usize,
}

impl WorkflowChecker for RunLengthChecker {
    fn kind(&self) -> CheckKind { CheckKind::WorkflowRun }

    fn check(&self, ctx: &WorkflowContext, errors: &mut Vec<LintError>) {
        for scan in &ctx.scans {
            for run in &scan.runs {
                if run.shell_lines <= self.cap {
                    continue;
                }
                match ctx.baseline.runs.judge(&run.key, run.shell_lines) {
                    Verdict::Unrecorded => errors.push(LintError::InlineShellTooLong {
                        kind: CheckKind::WorkflowRun,
                        file: scan.key.clone(),
                        step: run.label.clone(),
                        line: run.line,
                        lines: run.shell_lines,
                        cap: self.cap,
                        over: run.shell_lines - self.cap,
                    }),
                    Verdict::Grew { recorded, grew } => {
                        errors.push(LintError::InlineShellGrew {
                            kind: CheckKind::WorkflowRun,
                            file: scan.key.clone(),
                            step: run.label.clone(),
                            line: run.line,
                            lines: run.shell_lines,
                            recorded,
                            grew,
                        });
                    }
                    Verdict::Held => {}
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Report + orchestrator
// ═══════════════════════════════════════════════════════════════════

/// Aggregated results of a `workflows` run.
#[must_use]
pub struct WorkflowReport {
    /// One scan per workflow — the coverage output.
    pub scans: Vec<WorkflowScan>,
    /// Everything that failed.
    pub errors: Vec<LintError>,
    /// The limit steps were measured against.
    pub max_run_lines: usize,
}

impl WorkflowReport {
    /// Returns `true` when nothing failed.
    #[must_use]
    pub fn is_ok(&self) -> bool { self.errors.is_empty() }

    /// Filter errors by [`CheckKind`].
    #[must_use]
    pub fn errors_of(&self, kind: CheckKind) -> Vec<&LintError> {
        self.errors.iter().filter(|e| e.kind() == kind).collect()
    }

    /// Total block-form `run:` steps scanned.
    #[must_use]
    pub fn block_runs(&self) -> usize { self.scans.iter().map(|s| s.runs.len()).sum() }

    /// Total flow-form `run:` steps — the target shape.
    #[must_use]
    pub fn flow_runs(&self) -> usize { self.scans.iter().map(|s| s.flow_runs).sum() }

    /// Block-form steps over the limit, regardless of baseline.
    #[must_use]
    pub fn over_limit(&self) -> usize {
        self.runs_by_size().iter().filter(|r| r.shell_lines > self.max_run_lines).count()
    }

    /// Every block-form step across every workflow, longest first.
    #[must_use]
    pub fn runs_by_size(&self) -> Vec<&RunStep> {
        let mut all: Vec<&RunStep> = self.scans.iter().flat_map(|s| s.runs.iter()).collect();
        all.sort_by(|a, b| b.shell_lines.cmp(&a.shell_lines).then(a.key.cmp(&b.key)));
        all
    }

    /// Total lines of inline shell across the corpus — the quantity that has to
    /// go to zero for the directive to be real.
    #[must_use]
    pub fn total_shell_lines(&self) -> usize {
        self.scans.iter().flat_map(|s| s.runs.iter()).map(|r| r.shell_lines).sum()
    }
}

/// Lint every workflow a source yields.
///
/// # Errors
///
/// Returns an error if the source cannot be read.
pub fn lint_all(
    source: &dyn WorkflowSource,
    config: &WorkflowConfig,
    baseline: Baseline,
) -> anyhow::Result<WorkflowReport> {
    let scans: Vec<WorkflowScan> = source
        .workflows()?
        .iter()
        .map(|wf| scan_workflow(&wf.key, &wf.content))
        .collect();

    let ctx = WorkflowContext { scans, origin: source.origin(), baseline };

    let checkers: Vec<Box<dyn WorkflowChecker>> = vec![
        Box::new(WorkflowDiscoveryChecker),
        Box::new(RunLengthChecker { cap: config.max_run_lines }),
    ];

    let mut errors = Vec::new();
    for checker in &checkers {
        checker.check(&ctx, &mut errors);
    }

    Ok(WorkflowReport { scans: ctx.scans, errors, max_run_lines: config.max_run_lines })
}


#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory [`WorkflowSource`], so parsing and gating are testable without
    /// a filesystem.
    struct MockSource(Vec<Workflow>);

    impl MockSource {
        fn new() -> Self { Self(Vec::new()) }
        fn with(mut self, key: &str, content: &str) -> Self {
            self.0.push(Workflow { key: key.to_owned(), content: content.to_owned() });
            self
        }
    }

    impl WorkflowSource for MockSource {
        fn workflows(&self) -> anyhow::Result<Vec<Workflow>> { Ok(self.0.clone()) }
        fn origin(&self) -> String { "<mock>".to_owned() }
    }

    /// Join fixture lines. One source line per YAML line, so every fixture's
    /// INDENTATION — the thing the scanner actually reads — is visible at a
    /// glance instead of buried in escapes.
    fn yaml(lines: &[&str]) -> String {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }

    /// Wrap step lines in the minimum real workflow structure.
    fn wf(steps: &[&str]) -> String {
        let mut lines: Vec<&str> =
            vec!["name: CI", "on:", "  push:", "", "jobs:", "  build:", "    runs-on: ubuntu-latest", "    steps:"];
        lines.extend_from_slice(steps);
        yaml(&lines)
    }

    fn scan(steps: &[&str]) -> WorkflowScan { scan_workflow("ci.yml", &wf(steps)) }

    // ───────────────────────── block-header recognition ─────────────────────

    /// Every legal spelling, because rejecting one leaves it as the spelling the
    /// gate cannot see — and `|-` is exactly what a careful author writes.
    #[test]
    fn every_block_scalar_spelling_is_recognized() {
        for value in ["|", ">", "|-", ">-", "|+", ">+", "|2", "|2-", "|-2", "| # why"] {
            assert!(block_scalar_style(value).is_some(), "not recognized: {value:?}");
        }
        assert_eq!(block_scalar_style("|-"), Some('|'));
        assert_eq!(block_scalar_style(">-"), Some('>'));
    }

    /// Two different rejections, and the second one is the easy one to leave
    /// vacuous: a value that does not START with `|`/`>` is obviously flow, but a
    /// value that starts with one and then carries CONTENT is not a block header
    /// either — `|| true` and `>&2 …` are shell, not YAML indicators. Without the
    /// tail check those read as an empty block, and the step's real body — the
    /// lines below it — gets attributed to a header that YAML itself would
    /// reject. Silently mis-scanning malformed input is worse than reporting
    /// nothing, because it looks like a measurement.
    #[test]
    fn a_flow_scalar_is_not_a_block_header() {
        for value in ["cargo build", "", "echo '|'", "${{ inputs.cmd }}", "nix run .#x -- --arg 2"] {
            assert_eq!(block_scalar_style(value), None, "wrongly a block: {value:?}");
        }
        for value in ["|| true", ">&2 echo oops", "| grep x", ">/dev/null", "|2 tail"] {
            assert_eq!(
                block_scalar_style(value),
                None,
                "indicator-shaped shell read as a block header: {value:?}"
            );
        }
    }

    // ───────────────────────── flow form is the target shape ────────────────

    /// A one-line invocation is what the directive is FOR. A gate that flagged
    /// it would argue against its own destination.
    #[test]
    fn flow_form_is_never_a_violation_however_long_the_line() {
        let long = format!("        run: nix run .#{}", "a".repeat(400));
        let scanned = scan(&[
            "      - name: Build",
            "        run: cargo build --workspace --all-targets",
            "      - name: Long",
            &long,
            "      - run: pip install ansible",
        ]);
        assert_eq!(scanned.runs.len(), 0, "flow form produced findings: {:#?}", scanned.runs);
        assert_eq!(scanned.flow_runs, 3);
    }

    // ───────────────────────── the measurement ──────────────────────────────

    #[test]
    fn a_three_line_block_is_glue_and_a_four_line_block_is_not() {
        let three = scan(&[
            "      - name: A",
            "        run: |",
            "          one",
            "          two",
            "          three",
        ]);
        assert_eq!(three.runs[0].shell_lines, 3);

        let four = scan(&[
            "      - name: A",
            "        run: |",
            "          one",
            "          two",
            "          three",
            "          four",
        ]);
        assert_eq!(four.runs[0].shell_lines, 4);
    }

    /// Comments explaining WHY at the call site are fleet house style. Taxing
    /// them would teach exactly the wrong lesson: delete the explanation, keep
    /// the shell.
    #[test]
    fn a_comment_only_body_is_not_a_violation() {
        let scanned = scan(&[
            "      - name: Documented",
            "        run: |",
            "          # Measured 2026-08-06: this class keeps landing because",
            "          # inline shell in a run: block reads as YAML, not a script.",
            "          # Four lines of rationale.",
            "          # Still four.",
            "          nix run .#gate",
        ]);
        assert_eq!(scanned.runs[0].shell_lines, 1, "comments were counted as shell");
        assert_eq!(scanned.runs[0].body_lines, 5, "body total should include comments");

        let source = MockSource::new().with("ci.yml", &wf(&[
            "      - name: Documented",
            "        run: |",
            "          # one",
            "          # two",
            "          # three",
            "          # four",
            "          # five",
            "          nix run .#gate",
        ]));
        let report = lint_all(&source, &WorkflowConfig::default(), Baseline::default()).unwrap();
        assert!(report.is_ok(), "a documented one-liner failed: {:?}", report.errors);
    }

    #[test]
    fn a_blank_line_inside_a_block_does_not_end_it() {
        let scanned = scan(&[
            "      - name: A",
            "        run: |",
            "          one",
            "",
            "          two",
            "",
            "",
            "          three",
            "          four",
            "      - name: B",
            "        run: echo done",
        ]);
        assert_eq!(scanned.runs.len(), 1);
        assert_eq!(scanned.runs[0].shell_lines, 4, "block ended early at a blank line");
        assert_eq!(scanned.flow_runs, 1);
    }

    /// The block ends when indentation returns to the `run:` key's own column —
    /// a sibling key like `env:`, or the next step's `- `.
    #[test]
    fn a_block_ends_when_indentation_returns_to_the_run_level() {
        let scanned = scan(&[
            "      - name: A",
            "        run: |",
            "          one",
            "          two",
            "        env:",
            "          FOO: bar",
            "      - name: B",
            "        run: |",
            "          three",
        ]);
        assert_eq!(scanned.runs.len(), 2);
        assert_eq!(scanned.runs[0].shell_lines, 2, "block swallowed the sibling env: mapping");
        assert_eq!(scanned.runs[1].shell_lines, 1);
    }

    /// A `run: |` SHOWN inside another block scalar is a string, not a step. A
    /// scanner that re-read block bodies as structure would invent steps out of
    /// any workflow that writes a workflow.
    #[test]
    fn a_run_shown_inside_another_block_scalar_is_not_a_step() {
        let scanned = scan(&[
            "      - name: Generate",
            "        run: |",
            "          cat > out.yml <<'EOF'",
            "          jobs:",
            "            x:",
            "              steps:",
            "                - run: |",
            "                    echo nested",
            "                    echo more",
            "          EOF",
        ]);
        assert_eq!(scanned.runs.len(), 1, "nested run: became a step: {:#?}", scanned.runs);
        assert_eq!(scanned.runs[0].label, "Generate");
        assert_eq!(scanned.runs[0].shell_lines, 8);
    }

    /// Consuming EVERY block scalar, not only `run:` ones, is what keeps a
    /// `run: |` that a non-run block merely QUOTES from being counted as a step
    /// of this workflow. `actions/github-script`'s `script: |`, a docs example, a
    /// generator template — all carry workflow-shaped text inside a string, and a
    /// scanner that only skipped `run:` blocks would attribute that text's shell
    /// to a step that does not exist.
    #[test]
    fn a_non_run_block_scalar_is_not_measured() {
        let scanned = scan(&[
            "      - uses: actions/checkout@v4",
            "        with:",
            "          sparse-checkout: |",
            "            one",
            "            two",
            "            three",
            "            four",
            "      - uses: actions/github-script@v7",
            "        with:",
            "          script: |",
            "            // the template this action writes for consumers",
            "            const tmpl = `steps:",
            "              - name: Quoted",
            "                run: |",
            "                  echo one",
            "                  echo two",
            "                  echo three",
            "                  echo four`;",
            "            core.setOutput('t', tmpl)",
            "      - name: Real",
            "        run: |",
            "          go",
        ]);
        let labels: Vec<&str> = scanned.runs.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Real"], "a quoted run: was counted as a step");
    }

    /// `defaults: run: shell:` is a nested mapping, not a script.
    #[test]
    fn a_run_mapping_is_not_a_script() {
        let scanned = scan_workflow("ci.yml", &yaml(&[
            "jobs:",
            "  build:",
            "    defaults:",
            "      run:",
            "        shell: bash",
            "    steps:",
            "      - run: echo hi",
        ]));
        assert_eq!(scanned.runs.len(), 0);
        assert_eq!(scanned.flow_runs, 1);
    }

    // ───────────────────────── keys and labels ──────────────────────────────

    #[test]
    fn a_key_names_the_file_the_job_and_the_step() {
        let scanned = scan(&[
            "      - name: Resolve test environment",
            "        run: |",
            "          a",
        ]);
        assert_eq!(scanned.runs[0].key, "ci.yml::build/Resolve test environment");
        assert_eq!(scanned.runs[0].job, "build");
    }

    /// A step with no `name:` still needs a key, and its first command is the
    /// only stable thing it has.
    #[test]
    fn an_unnamed_step_is_keyed_on_its_first_command() {
        let scanned = scan(&[
            "      - run: |",
            "          nix build .#thing",
            "          echo done",
        ]);
        assert_eq!(scanned.runs[0].key, "ci.yml::build/nix build .#thing");
    }

    #[test]
    fn two_steps_sharing_a_label_get_distinct_keys() {
        let scanned = scan(&[
            "      - name: Same",
            "        run: |",
            "          a",
            "      - name: Same",
            "        run: |",
            "          b",
        ]);
        let keys: Vec<&str> = scanned.runs.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["ci.yml::build/Same", "ci.yml::build/Same #2"]);
    }

    /// A step name at STEP level only. The workflow's own `name:` and the job's
    /// `name:` must not leak in as a step label.
    #[test]
    fn a_workflow_or_job_name_is_not_a_step_name() {
        let scanned = scan_workflow("ci.yml", &yaml(&[
            "name: Whole workflow",
            "on:",
            "  push:",
            "jobs:",
            "  build:",
            "    name: The job",
            "    steps:",
            "      - run: |",
            "          the-command",
        ]));
        assert_eq!(scanned.runs[0].label, "the-command", "a non-step name leaked in");
    }

    #[test]
    fn the_job_is_tracked_across_several_jobs() {
        let scanned = scan_workflow("ci.yml", &yaml(&[
            "jobs:",
            "  first:",
            "    steps:",
            "      - name: A",
            "        run: |",
            "          a",
            "  second:",
            "    steps:",
            "      - name: B",
            "        run: |",
            "          b",
        ]));
        let jobs: Vec<&str> = scanned.runs.iter().map(|r| r.job.as_str()).collect();
        assert_eq!(jobs, ["first", "second"]);
    }

    #[test]
    fn steps_carry_their_line_number() {
        let text = wf(&["      - name: A", "        run: |", "          the-command"]);
        let scanned = scan_workflow("ci.yml", &text);
        let line = scanned.runs[0].line;
        assert_eq!(text.lines().nth(line - 1).unwrap().trim(), "run: |");
    }

    #[test]
    fn workflow_key_is_the_bare_file_name() {
        assert_eq!(workflow_key(Path::new("/a/.github/workflows/rust.yml")), "rust.yml");
    }

    // ───────────────────────── the gate, both directions ────────────────────

    /// NEGATIVE CONTROL, and the defect that motivated the whole subcommand: the
    /// ~15-line `run:` written into substrate's `rust-auto-release.yml` on
    /// 2026-08-06 — embedded `nix eval`, `grep -qx`, if/else — during the very
    /// session spent enforcing the no-shell rule. A gate never observed to
    /// reject anything reports a tier it does not have.
    #[test]
    fn the_2026_08_06_defect_is_rejected() {
        let source = MockSource::new().with("rust-auto-release.yml", &wf(&[
            "      - name: Resolve test environment",
            "        id: env",
            "        run: |",
            "          # pick the consumer's own devShell when it has one",
            "          shells=$(nix eval --impure --json --expr \\",
            "            \"builtins.attrNames (builtins.getFlake (toString ./.)).devShells\")",
            "          if echo \"$shells\" | grep -qx '\"default\"'; then",
            "            echo \"installable=.#default\" >> \"$GITHUB_OUTPUT\"",
            "            echo \"tier=consumer\" >> \"$GITHUB_OUTPUT\"",
            "          else",
            "            echo \"installable=github:pleme-io/substrate#release-gate\" >> \"$GITHUB_OUTPUT\"",
            "            echo \"tier=fallback\" >> \"$GITHUB_OUTPUT\"",
            "          fi",
            "          echo \"resolved: $(cat \"$GITHUB_OUTPUT\")\"",
            "          exit 0",
        ]));
        let report = lint_all(&source, &WorkflowConfig::default(), Baseline::default()).unwrap();

        let errs = report.errors_of(CheckKind::WorkflowRun);
        assert_eq!(errs.len(), 1, "the motivating defect went unreported: {errs:?}");
        let msg = errs[0].to_string();
        assert!(msg.contains("[workflow-run]"), "kind tag missing: {msg}");
        assert!(msg.contains("rust-auto-release.yml:"), "location missing: {msg}");
        assert!(msg.contains("Resolve test environment"), "step missing: {msg}");
        assert!(msg.contains("--write-baseline"), "escape hatch missing: {msg}");
        assert!(
            matches!(errs[0], LintError::InlineShellTooLong { cap: 3, lines: 11, .. }),
            "wrong shape: {:?}",
            errs[0]
        );
    }

    /// GREEN. A workflow that is all `uses:` plus one-line glue must pass, or
    /// the gate is noise nobody can satisfy.
    #[test]
    fn a_compliant_workflow_passes() {
        let source = MockSource::new().with("ci.yml", &wf(&[
            "      - uses: actions/checkout@v4",
            "      - uses: pleme-io/actions/tatara-script@main",
            "        with:",
            "          script: tools/release.tlisp",
            "      - name: Glue",
            "        run: nix run .#gate",
        ]));
        let report = lint_all(&source, &WorkflowConfig::default(), Baseline::default()).unwrap();
        assert!(report.is_ok(), "compliant workflow failed: {:?}", report.errors);
        assert_eq!(report.block_runs(), 0);
        assert_eq!(report.flow_runs(), 1);
    }

    #[test]
    fn the_line_limit_is_configurable() {
        let source = MockSource::new().with("ci.yml", &wf(&[
            "      - name: A",
            "        run: |",
            "          a",
            "          b",
            "          c",
            "          d",
            "          e",
        ]));
        let tight = WorkflowConfig { max_run_lines: 1 };
        let loose = WorkflowConfig { max_run_lines: 10 };
        assert_eq!(lint_all(&source, &tight, Baseline::default()).unwrap().errors.len(), 1);
        assert_eq!(lint_all(&source, &loose, Baseline::default()).unwrap().errors.len(), 0);
    }

    // ───────────────────────── coverage is part of the gate ─────────────────

    /// A run over zero files must exit non-zero. Matching `claudemd` exactly:
    /// "linted zero files" is a vacuous pass, never a success.
    #[test]
    fn a_run_over_zero_files_is_an_error_never_a_pass() {
        let report =
            lint_all(&MockSource::new(), &WorkflowConfig::default(), Baseline::default()).unwrap();
        assert!(!report.is_ok(), "a zero-file run reported success");
        assert!(matches!(report.errors[0], LintError::NoWorkflowsScanned { .. }));
    }

    #[test]
    fn every_scanned_file_appears_in_the_report() {
        let source = MockSource::new()
            .with("a.yml", &wf(&["      - run: echo a"]))
            .with("b.yml", "name: nothing here\n");
        let report = lint_all(&source, &WorkflowConfig::default(), Baseline::default()).unwrap();
        let keys: Vec<&str> = report.scans.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["a.yml", "b.yml"]);
    }

    // ───────────────────────── the ratchet ──────────────────────────────────

    /// One over-limit step of `lines` lines of shell, named `name`.
    fn long_step(name: &str, lines: usize) -> Vec<String> {
        let mut out = vec![format!("      - name: {name}"), "        run: |".to_owned()];
        out.extend((0..lines).map(|n| format!("          echo line-{n}")));
        out
    }

    fn steps(owned: &[String]) -> Vec<&str> { owned.iter().map(String::as_str).collect() }

    #[test]
    fn baselined_debt_does_not_fail() {
        let owned = long_step("Fat", 12);
        let source = MockSource::new().with("ci.yml", &wf(&steps(&owned)));
        let config = WorkflowConfig::default();
        let seed = lint_all(&source, &config, Baseline::default()).unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let report = lint_all(&source, &config, baseline).unwrap();
        assert!(report.is_ok(), "baselined debt still failed: {:?}", report.errors);
    }

    /// The seal: a baselined block that GROWS re-fails. Without this the
    /// baseline is an amnesty and the 12-line step drifts to 60.
    #[test]
    fn baselined_debt_that_grows_fails() {
        let config = WorkflowConfig::default();
        let small = long_step("Fat", 12);
        let seed = lint_all(
            &MockSource::new().with("ci.yml", &wf(&steps(&small))),
            &config,
            Baseline::default(),
        )
        .unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let grown = long_step("Fat", 15);
        let after = MockSource::new().with("ci.yml", &wf(&steps(&grown)));
        let report = lint_all(&after, &config, baseline).unwrap();
        let errs = report.errors_of(CheckKind::WorkflowRun);
        assert_eq!(errs.len(), 1, "growth past the baseline went unreported");
        assert!(
            matches!(errs[0], LintError::InlineShellGrew { recorded: 12, grew: 3, .. }),
            "{:?}",
            errs[0]
        );
    }

    /// The requirement the keying choice exists to satisfy: edit ABOVE a block —
    /// here 40 lines of new rationale at the top of the file, moving every line
    /// number below it — and the baseline must still cover the same step.
    #[test]
    fn a_baseline_survives_an_edit_above_the_block() {
        let config = WorkflowConfig::default();
        let owned = long_step("Fat", 12);
        let before = MockSource::new().with("ci.yml", &wf(&steps(&owned)));
        let seed = lint_all(&before, &config, Baseline::default()).unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));
        let recorded_line = seed.scans[0].runs[0].line;

        let preamble = (0..40).fold(String::new(), |mut acc, n| {
            let _ = writeln!(acc, "# rationale line {n}");
            acc
        });
        let moved = format!("{preamble}{}", wf(&steps(&owned)));
        let report = lint_all(
            &MockSource::new().with("ci.yml", &moved),
            &config,
            baseline,
        )
        .unwrap();

        assert_eq!(
            report.scans[0].runs[0].line,
            recorded_line + 40,
            "fixture did not actually move the block"
        );
        assert!(report.is_ok(), "line-number drift broke the baseline: {:?}", report.errors);
    }

    #[test]
    fn a_baseline_does_not_cover_a_step_it_does_not_name() {
        let config = WorkflowConfig::default();
        let old = long_step("Old", 12);
        let seed = lint_all(
            &MockSource::new().with("ci.yml", &wf(&steps(&old))),
            &config,
            Baseline::default(),
        )
        .unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let mut both = old.clone();
        both.extend(long_step("New", 12));
        let after = MockSource::new().with("ci.yml", &wf(&steps(&both)));
        let report = lint_all(&after, &config, baseline).unwrap();
        let errs = report.errors_of(CheckKind::WorkflowRun);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].to_string().contains("New"), "{}", errs[0]);
    }

    /// The stated trade-off, pinned as behaviour rather than left as prose: a
    /// RENAME breaks the key. Recorded so the cost is visible in the test suite
    /// and cannot be discovered by surprise in CI.
    #[test]
    fn renaming_a_step_re_reports_it_the_trade_off_of_keying_on_the_label() {
        let config = WorkflowConfig::default();
        let old = long_step("Old name", 12);
        let seed = lint_all(
            &MockSource::new().with("ci.yml", &wf(&steps(&old))),
            &config,
            Baseline::default(),
        )
        .unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let renamed = long_step("New name", 12);
        let after = MockSource::new().with("ci.yml", &wf(&steps(&renamed)));
        let report = lint_all(&after, &config, baseline).unwrap();
        assert_eq!(report.errors_of(CheckKind::WorkflowRun).len(), 1);
    }

    #[test]
    fn report_statistics_describe_the_corpus() {
        let mut owned = long_step("A", 2);
        owned.extend(long_step("B", 9));
        owned.push("      - run: echo glue".to_owned());
        let source = MockSource::new().with("ci.yml", &wf(&steps(&owned)));
        let report = lint_all(&source, &WorkflowConfig::default(), Baseline::default()).unwrap();
        assert_eq!(report.block_runs(), 2);
        assert_eq!(report.flow_runs(), 1);
        assert_eq!(report.over_limit(), 1);
        assert_eq!(report.total_shell_lines(), 11);
        assert_eq!(report.runs_by_size()[0].label, "B");
    }
}
