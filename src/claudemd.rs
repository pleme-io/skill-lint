//! `CLAUDE.md` linting — the anti-regrowth seal.
//!
//! A `CLAUDE.md` is loaded into every session, whole, before the first token of
//! work. Its size is therefore a standing tax on every task, and it grows the
//! way every unenforced contract grows: monotonically, one reasonable-looking
//! addition at a time.
//!
//! The pleme-io org `CLAUDE.md` reached 295,821 bytes. Its own
//! `## ★★ Substrate primitive index` section reached 137,663 B — 46.5% of the
//! file — while the section's OWN header declares the contract every entry is
//! supposed to obey:
//!
//! > Each line: **rule** + skill (if any) + long-form doc.
//!
//! 62 of 68 entries violated it (median 1,811 B, max 8,434 B). The contract was
//! stated and never checked, which is the whole story: a sprint cut the file
//! back, and without a gate it regrows exactly as it grew the first time.
//!
//! # What is measured, and in what unit
//!
//! **Per index entry: folded bytes.** Folding — [`crate::budget::fold`], the
//! same normalization the skill-listing accounting uses — collapses runs of
//! whitespace to one space, so an entry's measurement is invariant under hard
//! wrapping. Two entries with identical content must measure the same whether
//! one was wrapped at 80 columns and the other written on one line; without
//! folding the ceiling would be a rule about line-breaking rather than about
//! content. Markdown does not need a *different* normalization from YAML here —
//! it needs the same one, for the same reason.
//!
//! The UNIT is bytes rather than chars because the ceiling is stated in bytes
//! and this corpus is dense with multi-byte characters (★, —, §, CJK): a char
//! count would systematically understate what the entry actually costs.
//!
//! **Per file: raw bytes.** No folding. The file's cost is its bytes as they
//! sit on disk, newlines included — that is exactly what gets loaded.
//!
//! # Baseline debt
//!
//! The gate ships adoptable on day one or it does not ship: the live file still
//! carries ~63 over-ceiling entries, and a check that goes red on every run is a
//! check that gets skipped. Known violations are recorded in a baseline file
//! with the size they had when recorded, and only two things fail:
//!
//! - an entry (or file) over ceiling that the baseline does not name;
//! - a baselined entry (or file) that has GROWN past its recorded size.
//!
//! That second rule is the seal. A pure allowlist would let a baselined 1,400 B
//! entry drift to 8,000 B in silence, which is precisely the failure mode being
//! sealed. The baseline is a ratchet, not an amnesty.
//!
//! # Census
//!
//! `skip-<name>:` waivers, `pending-<name>:` markers and hard-imperative lines
//! are COUNTED and REPORTED, never gated. There is no defensible threshold for
//! "how many waivers is too many" — but the totals are a load signal a human
//! can read a trend from, and a number nobody prints is a number nobody watches.
//! Census counts the whole file, fenced code blocks included, because the model
//! reads the whole file including fences.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::budget::fold;
use crate::error::{CheckKind, LintError};
use crate::ratchet::{Ratchet, Verdict, parse_lines};

/// Per-entry ceiling. An index entry is a pointer — rule, skill, doc — and 400
/// bytes is room for all three plus a sentence of orientation.
pub const DEFAULT_MAX_ENTRY_BYTES: usize = 400;

/// Whole-file ceiling: 256 KiB. A round number, deliberately not tuned to any
/// one file — a default reverse-engineered from the corpus it is meant to
/// constrain would ratify whatever that corpus happens to weigh today.
pub const DEFAULT_MAX_FILE_BYTES: usize = 262_144;

/// Heading substring identifying the index section whose entries are measured.
pub const DEFAULT_INDEX_HEADING: &str = "Substrate primitive index";

/// Longest key fragment kept from an entry title.
const MAX_TITLE_BYTES: usize = 60;

// ═══════════════════════════════════════════════════════════════════
// DocSource — the I/O seam
// ═══════════════════════════════════════════════════════════════════

/// One document to lint: a stable key plus its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// Invocation-path-independent identifier, used in reports and baselines.
    pub key: String,
    /// Full file content.
    pub content: String,
}

/// Where documents come from. Mirrors [`crate::check::SkillSource`]: production
/// reads the filesystem, tests hand over strings, and neither one knows about
/// the other.
pub trait DocSource {
    /// Load every document to lint.
    ///
    /// # Errors
    ///
    /// Returns an error if any named document cannot be read. A file that is
    /// not there is a hard error, never a skip — silently dropping an
    /// unreadable file is how a linter comes to cover fewer files than its
    /// operator believes it covers.
    fn docs(&self) -> anyhow::Result<Vec<Doc>>;

    /// Human-readable description of what was asked for, reported when nothing
    /// was found.
    fn origin(&self) -> String;
}

/// Filesystem-backed [`DocSource`].
pub struct FsDocSource<'a> {
    /// Paths to lint, in the order given.
    pub paths: &'a [PathBuf],
}

impl DocSource for FsDocSource<'_> {
    fn docs(&self) -> anyhow::Result<Vec<Doc>> {
        self.paths
            .iter()
            .map(|path| {
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?;
                Ok(Doc { key: doc_key(path), content })
            })
            .collect()
    }

    fn origin(&self) -> String {
        if self.paths.is_empty() {
            return "<no files given>".to_owned();
        }
        self.paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Derive a document's report/baseline key from its path.
///
/// The last two components (`docs/pleme-io-CLAUDE.md`), so the key survives
/// being invoked by absolute path, by relative path, or from a different
/// working directory — a baseline keyed on the invocation string would go stale
/// the first time CI ran the tool from somewhere else.
#[must_use]
pub fn doc_key(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    match path.parent().and_then(Path::file_name) {
        Some(parent) if !parent.is_empty() => format!("{}/{name}", parent.to_string_lossy()),
        _ => name,
    }
}

// ═══════════════════════════════════════════════════════════════════
// Parsing
// ═══════════════════════════════════════════════════════════════════

/// One entry of the index section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// Human-facing name lifted from the bullet's opening.
    pub title: String,
    /// `<doc-key>::<title>` — what a baseline names.
    pub key: String,
    /// Folded size in bytes.
    pub bytes: usize,
    /// 1-based line number of the bullet, so a finding is navigable.
    pub line: usize,
}

/// The index section of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSection {
    /// The heading line as written.
    pub heading: String,
    /// Entries found, in document order.
    pub entries: Vec<IndexEntry>,
}

/// Directive load counted across one document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Census {
    /// `skip-<name>:` waiver tokens, by name.
    pub skips: BTreeMap<String, usize>,
    /// `pending-<name>:` markers, by name.
    pendings: BTreeMap<String, usize>,
    /// Occurrences of each hard-imperative token/phrase.
    pub imperatives: BTreeMap<String, usize>,
    /// Lines carrying at least one hard imperative.
    pub imperative_lines: usize,
    /// Occurrences of lowercase ` never ` — the soft-prose sibling of `NEVER`,
    /// counted separately because it is the far more common form and reads as
    /// prose rather than as a rule.
    pub never_lowercase: usize,
}

impl Census {
    /// Total `skip-*` waiver tokens.
    #[must_use]
    pub fn skip_total(&self) -> usize { self.skips.values().sum() }

    /// Total `pending-*` markers.
    #[must_use]
    pub fn pending_total(&self) -> usize { self.pendings.values().sum() }

    /// `pending-<name>:` markers, by name.
    #[must_use]
    pub fn pendings(&self) -> &BTreeMap<String, usize> { &self.pendings }
}

/// Everything one pass over a document yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocScan {
    /// Report/baseline key.
    pub key: String,
    /// Raw file size in bytes — the actual load cost, unfolded.
    pub bytes: usize,
    /// The index section, when the document has one.
    pub section: Option<IndexSection>,
    /// Directive load.
    pub census: Census,
}

impl DocScan {
    /// Entries found, or an empty slice when the document has no index section.
    #[must_use]
    pub fn entries(&self) -> &[IndexEntry] {
        self.section.as_ref().map_or(&[], |s| s.entries.as_slice())
    }
}

/// Is this line the start of an index entry?
///
/// # The bullet-matching trap
///
/// Entries are written `- **…`, EXCEPT the ones written `- ★…`. Scanning for
/// `- **` alone does not merely miss a star-prefixed entry — it silently WELDS
/// that entry's bytes onto the preceding one, because an entry runs until the
/// next bullet the scanner recognizes. The original audit of the live file
/// reported an 8,434 B entry that did not exist: one real entry plus the 4,139 B
/// star-prefixed entry beneath it, fused by the matcher.
///
/// A miss here is not a miss, it is a wrong number attributed to a named entry —
/// which is worse than silence, because it looks like a measurement.
#[must_use]
pub fn is_entry_bullet(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- ") else { return false };
    rest.starts_with("**") || rest.starts_with('★') || rest.starts_with('☆')
}

/// Scan one document.
///
/// Fenced code blocks are tracked so a `## ` heading or a `- **` bullet shown
/// INSIDE a fence — the live file has several, including a whole example
/// workflow — cannot terminate a section or invent an entry.
#[must_use]
pub fn scan_doc(key: &str, content: &str, index_heading: &str) -> DocScan {
    let lines: Vec<&str> = content.lines().collect();
    let mut fenced = false;
    let mut section_start: Option<(usize, usize, String)> = None; // (index, level, heading)
    let mut section_end = lines.len();
    let mut census = Census::default();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        // Census counts everything the model reads, fences included.
        count_census(line, &mut census);
        if fenced {
            continue;
        }

        let Some(level) = heading_level(line) else { continue };
        match &section_start {
            None => {
                if line.contains(index_heading) {
                    section_start = Some((index, level, (*line).to_owned()));
                }
            }
            Some((_, start_level, _)) => {
                if level <= *start_level && section_end == lines.len() {
                    section_end = index;
                }
            }
        }
    }

    let section = section_start.map(|(start, _, heading)| IndexSection {
        heading,
        entries: collect_entries(key, &lines[start + 1..section_end], start + 1),
    });

    DocScan { key: key.to_owned(), bytes: content.len(), section, census }
}

/// Heading level for a line, or `None` when it is not an ATX heading.
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (hashes > 0 && line[hashes..].starts_with(' ')).then_some(hashes)
}

/// Split a section body into entries: each runs from its bullet to the line
/// before the next one.
fn collect_entries(doc_key: &str, body: &[&str], line_offset: usize) -> Vec<IndexEntry> {
    let mut fenced = false;
    let mut starts: Vec<usize> = Vec::new();
    for (index, line) in body.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if !fenced && is_entry_bullet(line) {
            starts.push(index);
        }
    }

    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut entries = Vec::with_capacity(starts.len());
    for (nth, &start) in starts.iter().enumerate() {
        let end = starts.get(nth + 1).copied().unwrap_or(body.len());
        let text = body[start..end].join("\n");
        let title = title_of(body[start]);
        // Two entries may legitimately share a title; the key must still be
        // unique or a baseline entry would silently cover both.
        let count = seen.entry(title.clone()).or_default();
        *count += 1;
        let title = if *count > 1 { format!("{title} #{count}") } else { title };
        entries.push(IndexEntry {
            key: format!("{doc_key}::{title}"),
            title,
            bytes: fold(&text).len(),
            line: line_offset + start + 1,
        });
    }
    entries
}

/// Lift a stable, human-meaningful title out of an entry's opening line.
///
/// Cuts at the first em/en dash or bold-close, which is where every entry in
/// the live corpus separates its NAME from its gloss — so the key stays stable
/// while the gloss is rewritten, which is what a baseline needs.
#[must_use]
pub fn title_of(first_line: &str) -> String {
    let stripped = first_line.strip_prefix("- ").unwrap_or(first_line);
    let stripped = stripped
        .trim_start_matches(|c: char| matches!(c, '*' | '★' | '☆' | '✦') || c.is_whitespace());

    let cut = ["—", "–", "**"]
        .iter()
        .filter_map(|sep| stripped.find(sep))
        .min()
        .unwrap_or(stripped.len());

    let title = stripped[..cut]
        .trim()
        .trim_end_matches(|c: char| matches!(c, '.' | ',' | ':' | ';' | '*' | '-') || c.is_whitespace());
    let title = fold(title);

    let title = if title.len() > MAX_TITLE_BYTES {
        let mut end = MAX_TITLE_BYTES;
        while end > 0 && !title.is_char_boundary(end) {
            end -= 1;
        }
        title[..end].trim_end().to_owned()
    } else {
        title
    };

    if title.is_empty() { "<untitled>".to_owned() } else { title }
}

// ═══════════════════════════════════════════════════════════════════
// Census
// ═══════════════════════════════════════════════════════════════════

/// Uppercase standalone tokens that read as a hard rule.
const IMPERATIVE_TOKENS: &[&str] = &["NEVER", "MUST", "ALWAYS"];

/// Phrase forms of the same thing.
const IMPERATIVE_PHRASES: &[&str] = &["is forbidden", "are forbidden", "is banned", "are banned"];

fn count_census(line: &str, census: &mut Census) {
    count_marker(line, "skip-", &mut census.skips);
    count_marker(line, "pending-", &mut census.pendings);

    let mut hit = false;
    for token in IMPERATIVE_TOKENS {
        let n = count_standalone(line, token);
        if n > 0 {
            hit = true;
            *census.imperatives.entry((*token).to_owned()).or_default() += n;
        }
    }
    for phrase in IMPERATIVE_PHRASES {
        let n = line.matches(phrase).count();
        if n > 0 {
            hit = true;
            *census.imperatives.entry((*phrase).to_owned()).or_default() += n;
        }
    }
    if hit {
        census.imperative_lines += 1;
    }
    census.never_lowercase += line.matches(" never ").count();
}

/// Count `<prefix><name>:` tokens, e.g. `skip-urdume:`.
///
/// The trailing colon is required: `skip-` alone appears in ordinary prose
/// ("skip-level"), and a waiver census that counts prose is a number nobody can
/// act on.
fn count_marker(line: &str, prefix: &str, out: &mut BTreeMap<String, usize>) {
    let mut cursor = 0;
    while let Some(found) = line[cursor..].find(prefix) {
        let start = cursor + found;
        let after = &line[start + prefix.len()..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect();
        cursor = start + prefix.len() + name.len().max(1);

        // Not a continuation of a longer word: `no-skip-x:` is not a waiver.
        let prev_ok = line[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-'));
        if prev_ok && !name.is_empty() && after[name.len()..].starts_with(':') {
            *out.entry(name).or_default() += 1;
        }
    }
}

/// Count occurrences of `token` that are not part of a longer word.
fn count_standalone(line: &str, token: &str) -> usize {
    let mut count = 0;
    let mut cursor = 0;
    while let Some(found) = line[cursor..].find(token) {
        let start = cursor + found;
        let end = start + token.len();
        let before_ok = line[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let after_ok = line[end..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if before_ok && after_ok {
            count += 1;
        }
        cursor = end;
    }
    count
}

// ═══════════════════════════════════════════════════════════════════
// Baseline
// ═══════════════════════════════════════════════════════════════════

/// Known debt: what was already over ceiling, and how big it was then.
///
/// A ratchet rather than an allowlist — the recorded size is a ceiling of its
/// own, so a baselined entry may shrink or hold but never grow. The mechanism
/// itself lives in [`crate::ratchet`], shared with the `workflows` gate: a
/// ratchet re-implemented per gate is a ratchet that drifts per gate, and the
/// looser copy is the one nobody notices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    /// Entry key → size when recorded.
    pub entries: Ratchet,
    /// Document key → size when recorded.
    pub files: Ratchet,
}

impl Baseline {
    /// Parse a baseline file.
    ///
    /// Lines are `entry: <key> <bytes>` or `file: <key> <bytes>`; the grammar is
    /// [`crate::ratchet::parse_lines`]'s.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut baseline = Self::default();
        for (kind, key, size) in parse_lines(text) {
            match kind.as_str() {
                "entry" => baseline.entries.insert(key, size),
                "file" => baseline.files.insert(key, size),
                _ => {}
            }
        }
        baseline
    }

    /// Load a baseline from disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read. A missing baseline is an
    /// error, not an empty baseline: silently treating "cannot read the
    /// baseline" as "there is no debt" turns a typo in a CI path into a gate
    /// that reports every pre-existing violation as new, which gets the gate
    /// disabled.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading baseline {}", path.display()))?;
        Ok(Self::parse(&text))
    }

    /// Render the current violations as a baseline file.
    #[must_use]
    pub fn render(scans: &[DocScan], config: &ClaudeMdConfig) -> String {
        let mut out = String::from(
            "# skill-lint claudemd baseline — known debt, recorded at its measured size.\n\
             # A baselined item may SHRINK or hold, never grow: exceeding the recorded\n\
             # size fails the gate. Regenerate with `skill-lint claudemd --write-baseline`.\n",
        );
        for scan in scans {
            if scan.bytes > config.max_file_bytes {
                out.push_str(&format!("file: {} {}\n", scan.key, scan.bytes));
            }
        }
        for scan in scans {
            for entry in scan.entries() {
                if entry.bytes > config.max_entry_bytes {
                    out.push_str(&format!("entry: {} {}\n", entry.key, entry.bytes));
                }
            }
        }
        out
    }
}

// ═══════════════════════════════════════════════════════════════════
// Config, context, checkers
// ═══════════════════════════════════════════════════════════════════

/// Configuration for a `claudemd` run.
#[derive(Debug, Clone)]
pub struct ClaudeMdConfig {
    /// Folded-byte ceiling for one index entry.
    pub max_entry_bytes: usize,
    /// Raw-byte ceiling for a whole document.
    pub max_file_bytes: usize,
    /// Heading substring identifying the index section.
    pub index_heading: String,
}

impl Default for ClaudeMdConfig {
    fn default() -> Self {
        Self {
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            index_heading: DEFAULT_INDEX_HEADING.to_owned(),
        }
    }
}

/// Shared context built once, passed to every checker.
#[must_use]
pub struct DocContext {
    /// One scan per document, in the order given.
    pub scans: Vec<DocScan>,
    /// What was asked for — reported when nothing was found.
    pub origin: String,
    /// Known debt.
    pub baseline: Baseline,
}

/// A single composable check over documents. Mirrors [`crate::check::Checker`],
/// on its own context because a `CLAUDE.md` is not a skill directory and
/// pretending otherwise would make the shared context's field names lie.
pub trait DocChecker {
    /// The check category this checker belongs to.
    fn kind(&self) -> CheckKind;
    /// Run validation, appending any errors found.
    fn check(&self, ctx: &DocContext, errors: &mut Vec<LintError>);
}

/// Refuses a run that linted nothing.
///
/// The coverage half of the gate. A linter wired into one repository of seven
/// is green because it never looked at the other six — the same vacuous pass
/// [`LintError::NoSkillsFound`] already forbids for skills. Which files were
/// scanned is an OUTPUT of every run for the same reason.
pub struct DocDiscoveryChecker;

impl DocChecker for DocDiscoveryChecker {
    fn kind(&self) -> CheckKind { CheckKind::Discovery }

    fn check(&self, ctx: &DocContext, errors: &mut Vec<LintError>) {
        if ctx.scans.is_empty() {
            errors.push(LintError::NoDocsScanned {
                kind: CheckKind::Discovery,
                searched: ctx.origin.clone(),
            });
        }
    }
}

/// Enforces the per-entry ceiling, against the baseline ratchet.
pub struct EntryBudgetChecker {
    /// Folded-byte ceiling.
    pub cap: usize,
}

impl DocChecker for EntryBudgetChecker {
    fn kind(&self) -> CheckKind { CheckKind::ClaudeMdEntry }

    fn check(&self, ctx: &DocContext, errors: &mut Vec<LintError>) {
        for scan in &ctx.scans {
            for entry in scan.entries() {
                if entry.bytes <= self.cap {
                    continue;
                }
                match ctx.baseline.entries.judge(&entry.key, entry.bytes) {
                    Verdict::Unrecorded => errors.push(LintError::IndexEntryTooLong {
                        kind: CheckKind::ClaudeMdEntry,
                        file: scan.key.clone(),
                        entry: entry.title.clone(),
                        line: entry.line,
                        bytes: entry.bytes,
                        cap: self.cap,
                        over: entry.bytes - self.cap,
                    }),
                    Verdict::Grew { recorded, grew } => {
                        errors.push(LintError::IndexEntryGrew {
                            kind: CheckKind::ClaudeMdEntry,
                            file: scan.key.clone(),
                            entry: entry.title.clone(),
                            line: entry.line,
                            bytes: entry.bytes,
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

/// Enforces the whole-file ceiling, against the same ratchet.
pub struct FileBudgetChecker {
    /// Raw-byte ceiling.
    pub cap: usize,
}

impl DocChecker for FileBudgetChecker {
    fn kind(&self) -> CheckKind { CheckKind::ClaudeMdFile }

    fn check(&self, ctx: &DocContext, errors: &mut Vec<LintError>) {
        for scan in &ctx.scans {
            if scan.bytes <= self.cap {
                continue;
            }
            match ctx.baseline.files.judge(&scan.key, scan.bytes) {
                Verdict::Unrecorded => errors.push(LintError::DocTooLarge {
                    kind: CheckKind::ClaudeMdFile,
                    file: scan.key.clone(),
                    bytes: scan.bytes,
                    cap: self.cap,
                    over: scan.bytes - self.cap,
                }),
                Verdict::Grew { recorded, grew } => errors.push(LintError::DocGrew {
                    kind: CheckKind::ClaudeMdFile,
                    file: scan.key.clone(),
                    bytes: scan.bytes,
                    recorded,
                    grew,
                }),
                Verdict::Held => {}
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Report + orchestrator
// ═══════════════════════════════════════════════════════════════════

/// Aggregated results of a `claudemd` run.
#[must_use]
pub struct ClaudeMdReport {
    /// One scan per document — the coverage output.
    pub scans: Vec<DocScan>,
    /// Everything that failed.
    pub errors: Vec<LintError>,
    /// The ceiling entries were measured against.
    pub max_entry_bytes: usize,
}

impl ClaudeMdReport {
    /// Returns `true` when nothing failed.
    #[must_use]
    pub fn is_ok(&self) -> bool { self.errors.is_empty() }

    /// Filter errors by [`CheckKind`].
    #[must_use]
    pub fn errors_of(&self, kind: CheckKind) -> Vec<&LintError> {
        self.errors.iter().filter(|e| e.kind() == kind).collect()
    }

    /// Every entry across every document, largest first.
    #[must_use]
    pub fn entries_by_size(&self) -> Vec<&IndexEntry> {
        let mut all: Vec<&IndexEntry> = self.scans.iter().flat_map(DocScan::entries).collect();
        all.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.key.cmp(&b.key)));
        all
    }

    /// Total entries scanned.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.scans.iter().map(|s| s.entries().len()).sum()
    }

    /// Entries over the ceiling, regardless of baseline.
    #[must_use]
    pub fn over_ceiling(&self) -> usize {
        self.scans
            .iter()
            .flat_map(DocScan::entries)
            .filter(|e| e.bytes > self.max_entry_bytes)
            .count()
    }

    /// Median entry size in bytes, or `None` when there are no entries.
    #[must_use]
    pub fn median_entry_bytes(&self) -> Option<usize> {
        let mut sizes: Vec<usize> =
            self.scans.iter().flat_map(DocScan::entries).map(|e| e.bytes).collect();
        if sizes.is_empty() {
            return None;
        }
        sizes.sort_unstable();
        Some(sizes[sizes.len() / 2])
    }

    /// Combined census across every document scanned.
    #[must_use]
    pub fn census(&self) -> Census {
        let mut total = Census::default();
        for scan in &self.scans {
            for (name, count) in &scan.census.skips {
                *total.skips.entry(name.clone()).or_default() += count;
            }
            for (name, count) in &scan.census.pendings {
                *total.pendings.entry(name.clone()).or_default() += count;
            }
            for (name, count) in &scan.census.imperatives {
                *total.imperatives.entry(name.clone()).or_default() += count;
            }
            total.imperative_lines += scan.census.imperative_lines;
            total.never_lowercase += scan.census.never_lowercase;
        }
        total
    }
}

/// Lint every document a source yields.
///
/// # Errors
///
/// Returns an error if the source cannot be read.
pub fn lint_all(
    source: &dyn DocSource,
    config: &ClaudeMdConfig,
    baseline: Baseline,
) -> anyhow::Result<ClaudeMdReport> {
    let scans: Vec<DocScan> = source
        .docs()?
        .iter()
        .map(|doc| scan_doc(&doc.key, &doc.content, &config.index_heading))
        .collect();

    let ctx = DocContext { scans, origin: source.origin(), baseline };

    let checkers: Vec<Box<dyn DocChecker>> = vec![
        Box::new(DocDiscoveryChecker),
        Box::new(EntryBudgetChecker { cap: config.max_entry_bytes }),
        Box::new(FileBudgetChecker { cap: config.max_file_bytes }),
    ];

    let mut errors = Vec::new();
    for checker in &checkers {
        checker.check(&ctx, &mut errors);
    }

    Ok(ClaudeMdReport { scans: ctx.scans, errors, max_entry_bytes: config.max_entry_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory [`DocSource`], so parsing and gating are testable without a
    /// filesystem.
    struct MockDocSource(Vec<Doc>);

    impl MockDocSource {
        fn new() -> Self { Self(Vec::new()) }
        fn with(mut self, key: &str, content: &str) -> Self {
            self.0.push(Doc { key: key.to_owned(), content: content.to_owned() });
            self
        }
    }

    impl DocSource for MockDocSource {
        fn docs(&self) -> anyhow::Result<Vec<Doc>> { Ok(self.0.clone()) }
        fn origin(&self) -> String { "<mock>".to_owned() }
    }

    fn index(entries: &str) -> String {
        format!("# Doc\n\n## ★★ Substrate primitive index\n\nEach line: rule + skill + doc.\n\n{entries}\n")
    }

    // ───────────────────────── the bullet-matching trap ─────────────────────

    /// A star-prefixed bullet is an entry.
    ///
    /// This is the ONLY coverage for the trap: the one live star-prefixed entry
    /// was normalized to `- **`, so the real corpus can no longer exercise it.
    /// A matcher that only knows `- **` does not merely miss this bullet — it
    /// welds its bytes onto the entry above, and reports a phantom entry whose
    /// size is the sum of two. That is exactly how the original audit produced
    /// an 8,434 B entry that never existed.
    #[test]
    fn a_star_prefixed_bullet_is_its_own_entry() {
        assert!(is_entry_bullet("- ★★ The Tendril Method — canonical doctrine."));
        assert!(is_entry_bullet("- ★ Cache-leverage — the precomputed path."));
        assert!(is_entry_bullet("- **★★ Bold entry — a doctrine.**"));
        assert!(!is_entry_bullet("- plain bullet"));
        assert!(!is_entry_bullet("  - **nested**"));
    }

    #[test]
    fn a_star_prefixed_entry_is_not_welded_onto_the_one_above_it() {
        let doc = index(
            "- **★★ Alpha — first.** aaaa\n\n\
             - ★★ Beta — second.** bbbbbbbbbb\n\n\
             - **★★ Gamma — third.** cc\n",
        );
        let scan = scan_doc("t/CLAUDE.md", &doc, DEFAULT_INDEX_HEADING);
        let entries = scan.entries();

        assert_eq!(entries.len(), 3, "star bullet not recognized: {entries:#?}");
        assert_eq!(entries.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), [
            "Alpha", "Beta", "Gamma"
        ]);
        // The welding signature: Alpha absorbing Beta would be far larger than
        // Alpha's own line, and Beta would be absent entirely.
        assert!(
            entries[0].bytes < entries[1].bytes,
            "Alpha ({}) looks welded to Beta ({})",
            entries[0].bytes,
            entries[1].bytes
        );
    }

    // ───────────────────────── measurement ──────────────────────────────────

    /// Folding is what makes the ceiling a rule about content rather than about
    /// line-breaking: the same entry, hard-wrapped, must measure the same.
    #[test]
    fn entry_size_is_invariant_under_hard_wrapping() {
        let one_line = index("- **★★ A — x.** one two three four five six seven\n");
        let wrapped = index("- **★★ A — x.** one two three\n  four five six\n  seven\n");
        let a = scan_doc("k", &one_line, DEFAULT_INDEX_HEADING);
        let b = scan_doc("k", &wrapped, DEFAULT_INDEX_HEADING);
        assert_eq!(a.entries()[0].bytes, b.entries()[0].bytes);
    }

    /// Bytes, not chars: this corpus is dense with ★, — and §, and a char count
    /// would understate what an entry actually costs.
    #[test]
    fn measurement_is_in_bytes_not_chars() {
        let doc = index("- **★★ A — x.** ★★★\n");
        let scan = scan_doc("k", &doc, DEFAULT_INDEX_HEADING);
        let text = "- **★★ A — x.** ★★★";
        assert_eq!(scan.entries()[0].bytes, text.len());
        assert!(text.len() > text.chars().count(), "fixture must be multi-byte");
    }

    #[test]
    fn file_size_is_raw_bytes() {
        let doc = index("- **★★ A — x.** y\n");
        let scan = scan_doc("k", &doc, DEFAULT_INDEX_HEADING);
        assert_eq!(scan.bytes, doc.len());
    }

    // ───────────────────────── section scoping ──────────────────────────────

    #[test]
    fn only_the_index_section_yields_entries() {
        let doc = "# Doc\n\n## Prose\n\n- **A very long prose bullet** that is not an index entry.\n\n\
                   ## ★★ Substrate primitive index\n\n- **★ Real — entry.** x\n\n\
                   ## After\n\n- **Another prose bullet** here.\n";
        let scan = scan_doc("k", doc, DEFAULT_INDEX_HEADING);
        let entries = scan.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Real");
    }

    #[test]
    fn a_document_without_an_index_section_yields_no_entries_and_no_error() {
        let scan = scan_doc("k", "# Doc\n\n- **A bullet** x\n", DEFAULT_INDEX_HEADING);
        assert!(scan.section.is_none());
        assert_eq!(scan.entries().len(), 0);
    }

    /// A `##` heading or a `- **` bullet inside a fence is shown, not written —
    /// the live file fences a whole example workflow whose comments start with
    /// `#`.
    #[test]
    fn fenced_headings_and_bullets_are_not_structure() {
        let doc = "# Doc\n\n## ★★ Substrate primitive index\n\n\
                   - **★ A — one.** x\n\n\
                   ```yaml\n## Not a heading\n- **Not an entry**\n```\n\n\
                   - **★ B — two.** y\n\n## After\n\n- **prose**\n";
        let scan = scan_doc("k", doc, DEFAULT_INDEX_HEADING);
        let titles: Vec<&str> = scan.entries().iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["A", "B"], "fence leaked into structure");
    }

    // ───────────────────────── titles / keys ────────────────────────────────

    #[test]
    fn titles_cut_at_the_name_gloss_boundary() {
        assert_eq!(title_of("- **★★ OPERATING-THEORY — the six-axis…**"), "OPERATING-THEORY");
        assert_eq!(title_of("- ★★ The Tendril Method — canonical doctrine"), "The Tendril Method");
        assert_eq!(title_of("- **★★ Shigoto for every work graph.** Any tool…"), "Shigoto for every work graph");
        assert_eq!(title_of("- **★ Cache-leverage — the precomputed path.**"), "Cache-leverage");
    }

    #[test]
    fn duplicate_titles_get_distinct_keys() {
        let doc = index("- **★ Same — one.** a\n\n- **★ Same — two.** b\n");
        let scan = scan_doc("k", &doc, DEFAULT_INDEX_HEADING);
        let keys: Vec<&str> = scan.entries().iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["k::Same", "k::Same #2"]);
    }

    #[test]
    fn entries_carry_their_line_number() {
        let doc = index("- **★ A — one.** a\n");
        let scan = scan_doc("k", &doc, DEFAULT_INDEX_HEADING);
        let line = scan.entries()[0].line;
        assert_eq!(doc.lines().nth(line - 1).unwrap(), "- **★ A — one.** a");
    }

    // ───────────────────────── the gate, both directions ────────────────────

    fn long_entry(title: &str, bytes: usize) -> String {
        format!("- **★ {title} — gloss.** {}\n", "x".repeat(bytes))
    }

    /// RED. A gate never observed to fail may be checking nothing.
    #[test]
    fn an_over_ceiling_entry_is_caught() {
        let source = MockDocSource::new().with("t/CLAUDE.md", &index(&long_entry("Fat", 900)));
        let report = lint_all(&source, &ClaudeMdConfig::default(), Baseline::default()).unwrap();
        let errs = report.errors_of(CheckKind::ClaudeMdEntry);
        assert_eq!(errs.len(), 1, "expected one entry error, got {errs:?}");
        assert!(
            matches!(errs[0], LintError::IndexEntryTooLong { cap: 400, .. }),
            "wrong shape: {:?}",
            errs[0]
        );
        assert!(errs[0].to_string().contains("Fat"));
    }

    /// GREEN. A compliant file must pass, or the gate is just noise.
    #[test]
    fn a_compliant_file_passes() {
        let doc = index("- **★ Small — a rule.** See `theory/X.md`.\n\n- ★ Tiny — another.\n");
        let source = MockDocSource::new().with("t/CLAUDE.md", &doc);
        let report = lint_all(&source, &ClaudeMdConfig::default(), Baseline::default()).unwrap();
        assert!(report.is_ok(), "compliant file failed: {:?}", report.errors);
        assert_eq!(report.entry_count(), 2);
        assert_eq!(report.over_ceiling(), 0);
    }

    #[test]
    fn the_entry_ceiling_is_configurable() {
        let source = MockDocSource::new().with("t/CLAUDE.md", &index(&long_entry("Mid", 500)));
        let tight = ClaudeMdConfig { max_entry_bytes: 100, ..ClaudeMdConfig::default() };
        let loose = ClaudeMdConfig { max_entry_bytes: 5000, ..ClaudeMdConfig::default() };
        assert_eq!(lint_all(&source, &tight, Baseline::default()).unwrap().errors.len(), 1);
        assert_eq!(lint_all(&source, &loose, Baseline::default()).unwrap().errors.len(), 0);
    }

    // ───────────────────────── coverage is part of the gate ─────────────────

    /// A run over zero files must exit non-zero. A linter wired into one repo of
    /// seven is green because it never looked at the other six.
    #[test]
    fn a_run_over_zero_files_is_an_error_never_a_pass() {
        let source = MockDocSource::new();
        let report = lint_all(&source, &ClaudeMdConfig::default(), Baseline::default()).unwrap();
        assert!(!report.is_ok(), "a zero-file run reported success");
        assert!(matches!(report.errors[0], LintError::NoDocsScanned { .. }));
    }

    #[test]
    fn every_scanned_file_appears_in_the_report() {
        let source = MockDocSource::new()
            .with("a/CLAUDE.md", &index("- **★ A — x.** a\n"))
            .with("b/CLAUDE.md", "# no index here\n");
        let report = lint_all(&source, &ClaudeMdConfig::default(), Baseline::default()).unwrap();
        let keys: Vec<&str> = report.scans.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["a/CLAUDE.md", "b/CLAUDE.md"]);
    }

    // ───────────────────────── baseline debt ────────────────────────────────

    #[test]
    fn a_baselined_entry_does_not_fail() {
        let doc = index(&long_entry("Fat", 900));
        let source = MockDocSource::new().with("t/CLAUDE.md", &doc);
        let config = ClaudeMdConfig::default();
        let seed = lint_all(&source, &config, Baseline::default()).unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let report = lint_all(&source, &config, baseline).unwrap();
        assert!(report.is_ok(), "baselined debt still failed: {:?}", report.errors);
    }

    /// The ratchet. A pure allowlist would let a baselined entry drift from
    /// 1,400 B to 8,000 B in silence — which is the failure mode being sealed.
    #[test]
    fn a_baselined_entry_that_grows_fails() {
        let config = ClaudeMdConfig::default();
        let before = MockDocSource::new().with("t/CLAUDE.md", &index(&long_entry("Fat", 900)));
        let seed = lint_all(&before, &config, Baseline::default()).unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let after = MockDocSource::new().with("t/CLAUDE.md", &index(&long_entry("Fat", 1500)));
        let report = lint_all(&after, &config, baseline).unwrap();
        let errs = report.errors_of(CheckKind::ClaudeMdEntry);
        assert_eq!(errs.len(), 1, "growth past the baseline went unreported");
        assert!(matches!(errs[0], LintError::IndexEntryGrew { grew: 600, .. }), "{:?}", errs[0]);
    }

    #[test]
    fn a_baseline_does_not_cover_an_entry_it_does_not_name() {
        let config = ClaudeMdConfig::default();
        let before = MockDocSource::new().with("t/CLAUDE.md", &index(&long_entry("Old", 900)));
        let seed = lint_all(&before, &config, Baseline::default()).unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let after = MockDocSource::new().with(
            "t/CLAUDE.md",
            &index(&format!("{}\n{}", long_entry("Old", 900), long_entry("New", 900))),
        );
        let report = lint_all(&after, &config, baseline).unwrap();
        let errs = report.errors_of(CheckKind::ClaudeMdEntry);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].to_string().contains("New"), "{}", errs[0]);
    }

    #[test]
    fn baseline_parses_keys_containing_spaces() {
        let b = Baseline::parse(
            "# comment\n\nentry: docs/CLAUDE.md::The Tendril Method 4139\nfile: docs/CLAUDE.md 282648\nnonsense\n",
        );
        assert_eq!(b.entries.recorded("docs/CLAUDE.md::The Tendril Method"), Some(4139));
        assert_eq!(b.files.recorded("docs/CLAUDE.md"), Some(282_648));
        assert_eq!(b.entries.len(), 1);
    }

    // ───────────────────────── file ceiling ─────────────────────────────────

    #[test]
    fn an_over_ceiling_file_is_caught_and_can_be_baselined() {
        let doc = format!("# Doc\n{}", "x".repeat(5000));
        let source = MockDocSource::new().with("t/CLAUDE.md", &doc);
        let config = ClaudeMdConfig { max_file_bytes: 1000, ..ClaudeMdConfig::default() };

        let report = lint_all(&source, &config, Baseline::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::ClaudeMdFile).len(), 1);

        let baseline = Baseline::parse(&Baseline::render(&report.scans, &config));
        assert!(lint_all(&source, &config, baseline).unwrap().is_ok());
    }

    #[test]
    fn a_baselined_file_that_grows_fails() {
        let config = ClaudeMdConfig { max_file_bytes: 1000, ..ClaudeMdConfig::default() };
        let before = MockDocSource::new().with("t/CLAUDE.md", &"x".repeat(5000));
        let seed = lint_all(&before, &config, Baseline::default()).unwrap();
        let baseline = Baseline::parse(&Baseline::render(&seed.scans, &config));

        let after = MockDocSource::new().with("t/CLAUDE.md", &"x".repeat(6000));
        let report = lint_all(&after, &config, baseline).unwrap();
        assert!(matches!(
            report.errors_of(CheckKind::ClaudeMdFile)[0],
            LintError::DocGrew { grew: 1000, .. }
        ));
    }

    // ───────────────────────── census ───────────────────────────────────────

    #[test]
    fn census_counts_waivers_markers_and_imperatives() {
        let doc = "Use `skip-urdume: not-a-service` or `skip-tela: not-web`.\n\
                   A `pending-quadro: 2` note; another `skip-urdume: x`.\n\
                   This MUST hold and is forbidden otherwise; NEVER round up.\n\
                   We never round up in prose either.\n";
        let scan = scan_doc("k", doc, DEFAULT_INDEX_HEADING);
        let c = &scan.census;

        assert_eq!(c.skip_total(), 3);
        assert_eq!(c.skips.get("urdume"), Some(&2));
        assert_eq!(c.pending_total(), 1);
        assert_eq!(c.pendings().get("quadro"), Some(&1));
        assert_eq!(c.imperatives.get("MUST"), Some(&1));
        assert_eq!(c.imperatives.get("NEVER"), Some(&1));
        assert_eq!(c.imperatives.get("is forbidden"), Some(&1));
        assert_eq!(c.imperative_lines, 1);
        assert_eq!(c.never_lowercase, 1);
    }

    /// The census is a load signal, not a verdict — no count of waivers is a
    /// defensible pass/fail threshold, so it never produces an error.
    #[test]
    fn census_never_fails_the_run() {
        let doc = index("- **★ A — x.** skip-a: r skip-b: r pending-c: n MUST NEVER ALWAYS\n");
        let source = MockDocSource::new().with("t/CLAUDE.md", &doc);
        let report = lint_all(&source, &ClaudeMdConfig::default(), Baseline::default()).unwrap();
        assert!(report.is_ok(), "census produced an error: {:?}", report.errors);
        assert_eq!(report.census().skip_total(), 2);
    }

    #[test]
    fn a_marker_without_a_colon_is_prose_not_a_waiver() {
        let scan = scan_doc("k", "a skip-level review; no-skip-x: y\n", DEFAULT_INDEX_HEADING);
        assert_eq!(scan.census.skip_total(), 0);
    }

    #[test]
    fn an_imperative_inside_a_longer_word_does_not_count() {
        let scan = scan_doc("k", "MUSTARD and NEVERMORE and ALWAYSON\n", DEFAULT_INDEX_HEADING);
        assert_eq!(scan.census.imperative_lines, 0);
    }

    #[test]
    fn census_sums_across_documents() {
        let source = MockDocSource::new()
            .with("a/CLAUDE.md", "skip-one: r\n")
            .with("b/CLAUDE.md", "skip-two: r\nskip-one: r\n");
        let report = lint_all(&source, &ClaudeMdConfig::default(), Baseline::default()).unwrap();
        let census = report.census();
        assert_eq!(census.skip_total(), 3);
        assert_eq!(census.skips.get("one"), Some(&2));
    }

    // ───────────────────────── keys / report shape ──────────────────────────

    #[test]
    fn doc_key_is_the_last_two_path_components() {
        assert_eq!(doc_key(Path::new("/a/b/docs/pleme-io-CLAUDE.md")), "docs/pleme-io-CLAUDE.md");
        assert_eq!(doc_key(Path::new("CLAUDE.md")), "CLAUDE.md");
    }

    #[test]
    fn report_statistics_describe_the_corpus() {
        let doc = index(&format!(
            "{}\n{}\n{}",
            long_entry("A", 10),
            long_entry("B", 500),
            long_entry("C", 900)
        ));
        let source = MockDocSource::new().with("t/CLAUDE.md", &doc);
        let report = lint_all(&source, &ClaudeMdConfig::default(), Baseline::default()).unwrap();
        assert_eq!(report.entry_count(), 3);
        assert_eq!(report.over_ceiling(), 2);
        assert_eq!(report.entries_by_size()[0].title, "C");
        assert!(report.median_entry_bytes().is_some());
    }
}
