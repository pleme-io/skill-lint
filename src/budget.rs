//! Skill-listing budget accounting.
//!
//! Claude Code loads a listing of skill names and descriptions so the model
//! knows what is available. That listing has a hard character budget — 1% of
//! the model's context window by default — and on overflow Claude Code drops
//! descriptions, starting with the skills you invoke least.
//!
//! `/context` reports the listing size AFTER the budget is applied, which is
//! the authoritative number. But it is an interactive TUI command: it has no
//! print-mode rendering (`claude -p "/context"` returns "Execution error"), so
//! no agent, script, or CI job can read it. This module is the reachable
//! equivalent — deterministic, computed from the same frontmatter the platform
//! reads, and therefore diffable, gateable, and runnable by anything.
//!
//! # What this CANNOT know, and says so
//!
//! Which skills get dropped on overflow. That ordering is by invocation
//! frequency, which lives in the client's own history, not on disk. This module
//! reports how far over budget the corpus is and which entries are largest; it
//! never claims to know which descriptions the platform will discard. Reporting
//! a guess there would be exactly the round-up the corpus forbids.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model;

/// Platform default for `skillListingMaxDescChars` — the per-entry cap.
pub const DEFAULT_MAX_DESC_CHARS: usize = 1536;

/// Platform default listing budget as a fraction of the context window.
pub const DEFAULT_BUDGET_FRACTION: f64 = 0.01;

/// Rough chars-per-token for English prose plus markdown. Used ONLY to turn a
/// window size in tokens into a character budget; it is an estimate and the
/// report labels it as one.
pub const CHARS_PER_TOKEN: f64 = 4.0;

/// Fold a description the way the listing sees it.
///
/// YAML block scalars (`>-`) reach the consumer already folded, so counting raw
/// source bytes over-counts every multi-line description. Normalizing runs of
/// whitespace to single spaces is what makes this module's numbers agree with
/// [`crate::check::ListingBudgetChecker`]'s — they must, or the gate and the
/// report would disagree about the same corpus.
#[must_use]
pub fn fold(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One skill's contribution to the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub home: String,
    /// Folded description length in characters.
    pub desc_chars: usize,
    /// What this entry costs the listing: name + description + separators.
    pub listing_chars: usize,
    /// Characters past the per-entry cap that the platform silently discards.
    pub truncated_chars: usize,
    pub body_bytes: usize,
}

/// Why a `SKILL.md` under a scanned home did not contribute a described entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingKind {
    /// The file exists but could not be read.
    Unreadable { cause: String },
    /// The frontmatter is not valid YAML; `hint` names the fix when the cause
    /// is a known shape (an unquoted `': '` in `description`).
    Unparseable { cause: String, hint: String },
    /// The frontmatter parses but carries no (or a blank) `description`, so the
    /// listing offers a bare name nothing can route to.
    MissingDescription,
}

/// A `SKILL.md` the listing cannot account for as authored.
///
/// Skipping these is the failure this type exists to prevent: a skill that
/// silently drops out of the count makes every total smaller and every gate
/// greener than the corpus it claims to have read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: PathBuf,
    pub kind: FindingKind,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = self.path.display();
        match &self.kind {
            FindingKind::Unreadable { cause } => write!(f, "{path}: unreadable: {cause}"),
            FindingKind::Unparseable { cause, hint } => {
                write!(f, "{path}: frontmatter is not valid YAML: {cause}{hint}")
            }
            FindingKind::MissingDescription => write!(
                f,
                "{path}: no description — the listing carries a bare name nothing can route to"
            ),
        }
    }
}

/// The whole corpus's listing accounting.
#[derive(Debug, Clone)]
pub struct BudgetReport {
    pub entries: Vec<Entry>,
    pub total_listing_chars: usize,
    pub budget_chars: usize,
    pub max_desc_chars: usize,
    /// Homes scanned, so a report over a partial corpus is never mistaken for
    /// the whole. The live listing is the UNION of every deployed home.
    pub homes: Vec<String>,
    /// Every `SKILL.md` that is unreadable, unparseable or description-less,
    /// in scan order. Reported always; a failure under `--strict`.
    pub findings: Vec<Finding>,
}

impl BudgetReport {
    #[must_use]
    pub fn over_budget(&self) -> bool { self.total_listing_chars > self.budget_chars }

    #[must_use]
    pub fn overage_chars(&self) -> usize {
        self.total_listing_chars.saturating_sub(self.budget_chars)
    }

    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.budget_chars == 0 {
            return f64::INFINITY;
        }
        #[allow(clippy::cast_precision_loss)]
        let r = self.total_listing_chars as f64 / self.budget_chars as f64;
        r
    }

    /// Entries whose description is past the per-entry cap — text the platform
    /// discards outright, including any trigger phrase sitting in it.
    #[must_use]
    pub fn truncated(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.truncated_chars > 0).collect()
    }

    #[must_use]
    pub fn total_truncated_chars(&self) -> usize {
        self.entries.iter().map(|e| e.truncated_chars).sum()
    }

    #[must_use]
    pub fn over_body_cap(&self, max_body_bytes: usize) -> Vec<&Entry> {
        let mut over: Vec<&Entry> = self.entries.iter().filter(|e| e.body_bytes > max_body_bytes).collect();
        over.sort_by(|a, b| b.body_bytes.cmp(&a.body_bytes).then(a.name.cmp(&b.name)));
        over
    }

    #[must_use]
    pub fn largest_body(&self) -> Option<&Entry> {
        self.entries.iter().max_by(|a, b| a.body_bytes.cmp(&b.body_bytes).then(b.name.cmp(&a.name)))
    }
}

/// Compute the listing budget over one or more skill homes.
///
/// # Errors
///
/// Returns an error if a home cannot be read.
pub fn compute(
    homes: &[PathBuf],
    budget_chars: usize,
    max_desc_chars: usize,
) -> anyhow::Result<BudgetReport> {
    // Keyed by skill name: the live listing dedupes by name, so two homes
    // shipping the same skill cost the listing once, not twice.
    let mut by_name: BTreeMap<String, Entry> = BTreeMap::new();
    let mut scanned = Vec::new();
    let mut findings = Vec::new();

    for home in homes {
        scanned.push(home.display().to_string());
        let Ok(read) = std::fs::read_dir(home) else { continue };
        let mut dirs: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).collect();
        dirs.sort();
        for path in dirs {
            if !path.is_dir() {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let content = match std::fs::read_to_string(&skill_md) {
                Ok(content) => content,
                Err(e) => {
                    let kind = FindingKind::Unreadable { cause: e.to_string() };
                    findings.push(Finding { path: skill_md, kind });
                    continue;
                }
            };
            let fm = match model::parse_frontmatter(&content) {
                Ok(fm) => fm,
                Err(e) => {
                    let cause = e.to_string();
                    let hint = model::frontmatter_parse_hint(&cause);
                    findings.push(Finding { path: skill_md, kind: FindingKind::Unparseable { cause, hint } });
                    continue;
                }
            };
            let name = fm.name.clone().unwrap_or_else(|| {
                path.file_name().unwrap_or_default().to_string_lossy().into_owned()
            });
            let folded = fm.description.as_deref().map(fold).unwrap_or_default();
            if folded.is_empty() {
                findings.push(Finding { path: skill_md, kind: FindingKind::MissingDescription });
            }
            let desc_chars = folded.chars().count();
            let truncated_chars = desc_chars.saturating_sub(max_desc_chars);
            // The platform only ever ships the capped prefix, so the listing
            // cost is the capped length — not the authored length.
            let counted = desc_chars.min(max_desc_chars);
            by_name.insert(
                name.clone(),
                Entry {
                    listing_chars: counted + name.chars().count() + 4,
                    name,
                    home: home.display().to_string(),
                    desc_chars,
                    truncated_chars,
                    body_bytes: content.len(),
                },
            );
        }
    }

    let mut entries: Vec<Entry> = by_name.into_values().collect();
    entries.sort_by(|a, b| b.listing_chars.cmp(&a.listing_chars).then(a.name.cmp(&b.name)));
    let total_listing_chars = entries.iter().map(|e| e.listing_chars).sum();

    Ok(BudgetReport { entries, total_listing_chars, budget_chars, max_desc_chars, homes: scanned, findings })
}

/// Derive a character budget from a context-window size in tokens.
#[must_use]
pub fn budget_from_window(window_tokens: usize, fraction: f64) -> usize {
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let chars = (window_tokens as f64 * fraction * CHARS_PER_TOKEN) as usize;
    chars
}

/// Discover sibling skill homes under a workspace root.
///
/// The live listing is the union of every deployed home, so a report over one
/// repository is a report over a fraction of what the model actually receives —
/// the difference between "144 skills" and "179 skills" on this fleet.
#[must_use]
pub fn discover_homes(root: &Path) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    let Ok(read) = std::fs::read_dir(root) else { return homes };
    let mut dirs: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let candidate = dir.join("skills");
        if candidate.is_dir() {
            homes.push(candidate);
        }
    }
    homes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_matches_the_checker_normalization() {
        assert_eq!(fold("a\n  b   c\n"), "a b c");
        assert_eq!(fold("  leading and trailing  "), "leading and trailing");
    }

    #[test]
    fn budget_from_window_uses_the_documented_fraction() {
        // 1M-token window at 1% ≈ 40k chars.
        assert_eq!(budget_from_window(1_000_000, 0.01), 40_000);
        assert_eq!(budget_from_window(200_000, 0.01), 8_000);
    }

    #[test]
    fn an_over_cap_description_is_counted_at_the_cap_not_its_authored_length() {
        // The platform ships only the capped prefix, so a 3,000-char
        // description costs the listing 1,536 — the rest is discarded, not
        // budgeted. Counting the authored length would overstate the total and
        // understate how many entries fit.
        let e = Entry {
            name: "x".into(),
            home: "h".into(),
            desc_chars: 3000,
            listing_chars: 1536 + 1 + 4,
            truncated_chars: 3000 - 1536,
            body_bytes: 0,
        };
        assert_eq!(e.truncated_chars, 1464);
        assert!(e.listing_chars < e.desc_chars);
    }

    #[test]
    fn report_arithmetic() {
        let entries = vec![
            Entry { name: "a".into(), home: "h".into(), desc_chars: 100, listing_chars: 105, truncated_chars: 0, body_bytes: 500 },
            Entry { name: "b".into(), home: "h".into(), desc_chars: 2000, listing_chars: 1541, truncated_chars: 464, body_bytes: 9000 },
            Entry { name: "c".into(), home: "h".into(), desc_chars: 10, listing_chars: 15, truncated_chars: 0, body_bytes: 12_001 },
        ];
        let r = BudgetReport {
            total_listing_chars: entries.iter().map(|e| e.listing_chars).sum(),
            entries,
            budget_chars: 1000,
            max_desc_chars: 1536,
            homes: vec!["h".into()],
            findings: vec![],
        };
        assert!(r.over_budget());
        assert_eq!(r.overage_chars(), 661);
        assert_eq!(r.truncated().len(), 1);
        assert_eq!(r.total_truncated_chars(), 464);
        let over: Vec<&str> = r.over_body_cap(1000).iter().map(|e| e.name.as_str()).collect();
        assert_eq!(over, ["c", "b"]);
        assert!(r.over_body_cap(12_001).is_empty());
        assert_eq!(r.largest_body().map(|e| e.name.as_str()), Some("c"));
    }

    #[test]
    fn body_bytes_is_the_whole_skill_md_frontmatter_included() {
        let home = tempfile::TempDir::new().unwrap();
        let text = "---\nname: whole\ndescription: counted\n---\n\n# Body\n";
        std::fs::create_dir_all(home.path().join("whole")).unwrap();
        std::fs::write(home.path().join("whole").join("SKILL.md"), text).unwrap();

        let r = compute(&[home.path().to_path_buf()], 100_000, DEFAULT_MAX_DESC_CHARS).unwrap();

        assert_eq!(r.entries[0].body_bytes, text.len());
    }

    #[test]
    fn a_blank_description_is_a_finding_and_a_parse_failure_is_not_counted() {
        let home = tempfile::TempDir::new().unwrap();
        for (name, fm) in [
            ("blank", "name: blank\ndescription: \"  \""),
            ("colon", "name: colon\ndescription: two shapes: one and two"),
            ("fine", "name: fine\ndescription: ok"),
        ] {
            std::fs::create_dir_all(home.path().join(name)).unwrap();
            std::fs::write(home.path().join(name).join("SKILL.md"), format!("---\n{fm}\n---\n")).unwrap();
        }
        std::fs::create_dir_all(home.path().join("not-a-skill")).unwrap();

        let r = compute(&[home.path().to_path_buf()], 100_000, DEFAULT_MAX_DESC_CHARS).unwrap();

        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["fine", "blank"]);
        let kinds: Vec<&FindingKind> = r.findings.iter().map(|f| &f.kind).collect();
        assert_eq!(kinds.len(), 2, "{kinds:?}");
        assert_eq!(kinds[0], &FindingKind::MissingDescription);
        assert!(matches!(kinds[1], FindingKind::Unparseable { hint, .. } if hint.contains("'>-'")));
    }

    #[test]
    fn a_skill_present_in_two_homes_is_counted_once() {
        // The deployed listing dedupes by name; double-counting would inflate
        // the total on a fleet whose homes overlap by design (a source repo and
        // its deploy target).
        let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
        by_name.insert("dup".into(), 10);
        by_name.insert("dup".into(), 10);
        assert_eq!(by_name.len(), 1);
    }
}
