use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Which check phase produced the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CheckKind {
    Discovery,
    Version,
    Sync,
    Frontmatter,
    MapIntegrity,
    Staleness,
    References,
    /// Per-entry skill-listing budget.
    ///
    /// Claude Code truncates each listing entry — the combined `description`
    /// and `when_to_use` — at a hard character cap, and text past that point is
    /// silently discarded. A skill whose trigger vocabulary sits past the cut
    /// still *looks* authored and maintained while being unroutable: a guard
    /// over zero subjects. This is structural, not freshness — it has an
    /// objective right answer a machine can check — so it is always-on and
    /// gateable alongside sync/frontmatter/map-integrity.
    ListingBudget,
    /// Do the paths a skill body points at actually exist?
    ///
    /// The structural half of the References family. Reference-FRESHNESS asks
    /// "was the target re-verified more recently than the referrer?" — a
    /// judgement only a human can honestly settle, so it is opt-in. Resolution
    /// asks "does the target exist at all?" — an objective fact a machine
    /// checks and a human fixes without manufacturing a claim, so it is
    /// always-on and safe to gate CI on.
    ///
    /// A dead pointer costs an agent a whole session: it reads the skill, goes
    /// looking for the file it names, and finds nothing to read.
    PathResolution,
    /// Per-entry budget of a `CLAUDE.md` index section.
    ///
    /// An index section states its own contract — "each line: rule + skill +
    /// long-form doc" — and then grows past it, because nothing was checking.
    /// The org file's index reached 137,663 B, 46.5% of the whole document,
    /// with 62 of 68 entries in violation of a contract printed at the top of
    /// the very section they sit in.
    ClaudeMdEntry,
    /// Whole-file budget of a `CLAUDE.md`.
    ///
    /// The file is loaded into every session before the first token of work, so
    /// its size is a standing tax on every task in the repository.
    ClaudeMdFile,
}

impl fmt::Display for CheckKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery => write!(f, "discovery"),
            Self::Version => write!(f, "version"),
            Self::Sync => write!(f, "sync"),
            Self::Frontmatter => write!(f, "frontmatter"),
            Self::MapIntegrity => write!(f, "map-integrity"),
            Self::Staleness => write!(f, "staleness"),
            Self::References => write!(f, "references"),
            Self::ListingBudget => write!(f, "listing-budget"),
            Self::PathResolution => write!(f, "path-resolution"),
            Self::ClaudeMdEntry => write!(f, "claudemd-entry"),
            Self::ClaudeMdFile => write!(f, "claudemd-file"),
        }
    }
}

/// How a path was written in a skill body.
///
/// Typed rather than a bare string because the two forms resolve against
/// different bases and are gated differently: a relative link resolves from the
/// skill's own directory and is always checked; a repo-relative path resolves
/// from the root holding sibling repositories and is checked only when that
/// repository is present locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathForm {
    /// A markdown link with an explicit `./` or `../` target.
    RelativeLink,
    /// A backticked `<repo>/<path>` naming a file in a sibling repository.
    RepoPath,
}

impl fmt::Display for PathForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RelativeLink => write!(f, "relative link"),
            Self::RepoPath => write!(f, "repo-relative path"),
        }
    }
}

impl FromStr for CheckKind {
    type Err = ParseCheckKindError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "discovery" => Ok(Self::Discovery),
            "version" => Ok(Self::Version),
            "sync" => Ok(Self::Sync),
            "frontmatter" => Ok(Self::Frontmatter),
            "map-integrity" => Ok(Self::MapIntegrity),
            "staleness" => Ok(Self::Staleness),
            "references" => Ok(Self::References),
            "listing-budget" => Ok(Self::ListingBudget),
            "path-resolution" => Ok(Self::PathResolution),
            "claudemd-entry" => Ok(Self::ClaudeMdEntry),
            "claudemd-file" => Ok(Self::ClaudeMdFile),
            _ => Err(ParseCheckKindError(s.to_owned())),
        }
    }
}

/// Error returned when parsing an invalid [`CheckKind`] string.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown check kind: '{0}'")]
pub struct ParseCheckKindError(String);

/// A single validation error produced by a checker.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum LintError {
    /// Discovery found nothing to lint. A run that validates zero skills is a
    /// vacuous pass — it manufactures confidence while checking nothing — so
    /// it is an error, never a success.
    #[error("[{kind}] no skills found under '{searched}' — expected subdirectories each containing a SKILL.md (pass --skills-dir to point at the right root)")]
    NoSkillsFound { kind: CheckKind, searched: String },

    /// The `CLAUDE.md` sibling of [`Self::NoSkillsFound`], and the same rule: a
    /// linter wired into one repository of seven is green because it never
    /// looked at the other six. Which files were scanned is an output of every
    /// run precisely so that this cannot happen quietly.
    #[error("[{kind}] no CLAUDE.md files were scanned (asked for: {searched}) — a run that lints zero files is a vacuous pass, not a success. Pass --file <PATH> for each file to lint.")]
    NoDocsScanned { kind: CheckKind, searched: String },

    #[error("[{kind}] {file}:{line} index entry '{entry}' is {bytes} B, over the {cap} B ceiling by {over} B. The section's own header says each line is rule + skill + long-form doc — move the argument, the tier ledger and the worked examples into the doc it points at, which loads on demand instead of in every session. If this is known debt, record it with --write-baseline.")]
    IndexEntryTooLong {
        kind: CheckKind,
        file: String,
        entry: String,
        line: usize,
        bytes: usize,
        cap: usize,
        over: usize,
    },

    #[error("[{kind}] {file}:{line} index entry '{entry}' grew to {bytes} B from the {recorded} B recorded in the baseline (+{grew} B). Baselined debt may shrink or hold, never grow — that is the whole point of the baseline. Move the new material into the long-form doc, or re-record deliberately with --write-baseline.")]
    IndexEntryGrew {
        kind: CheckKind,
        file: String,
        entry: String,
        line: usize,
        bytes: usize,
        recorded: usize,
        grew: usize,
    },

    #[error("[{kind}] {file} is {bytes} B, over the {cap} B ceiling by {over} B — this file is loaded whole into every session, so its size is a standing tax on every task. If this is known debt, record it with --write-baseline.")]
    DocTooLarge {
        kind: CheckKind,
        file: String,
        bytes: usize,
        cap: usize,
        over: usize,
    },

    #[error("[{kind}] {file} grew to {bytes} B from the {recorded} B recorded in the baseline (+{grew} B). This is the regrowth the baseline exists to catch: the file was cut back once and is climbing again. Move new material into a linked doc, or re-record deliberately with --write-baseline.")]
    DocGrew {
        kind: CheckKind,
        file: String,
        bytes: usize,
        recorded: usize,
        grew: usize,
    },

    #[error("[{kind}] skill directory '{name}' has no entry in skill-map.yaml")]
    MissingMapEntry { kind: CheckKind, name: String },

    #[error("[{kind}] map entry '{name}' has no skill directory")]
    OrphanMapEntry { kind: CheckKind, name: String },

    #[error("[{kind}] skill '{skill}': description is {chars} chars, over the {cap}-char listing cap — the last {over} chars are silently discarded and any trigger phrase in them can never match. Front-load the invocation triggers and move narrative into the skill body (the body loads on invoke and costs nothing until then).")]
    DescriptionTooLong {
        kind: CheckKind,
        skill: String,
        chars: usize,
        cap: usize,
        over: usize,
    },

    #[error("[{kind}] skill '{skill}': {form} '{path}' does not resolve — nothing exists at that path. Fix the pointer, drop it, or — if the target is legitimately absent — declare it in the skill body with a line reading `pending-path: {path} — <reason>`.")]
    UnresolvedPath {
        kind: CheckKind,
        skill: String,
        path: String,
        form: PathForm,
    },

    #[error("[{kind}] skill '{skill}': frontmatter field '{field}' is missing")]
    MissingFrontmatter {
        kind: CheckKind,
        skill: String,
        field: String,
    },

    #[error("[{kind}] skill '{skill}': name '{found}' does not match directory '{expected}'")]
    NameMismatch {
        kind: CheckKind,
        skill: String,
        found: String,
        expected: String,
    },

    #[error("[{kind}] skill '{skill}' references unknown skill '{target}'")]
    BrokenReference {
        kind: CheckKind,
        skill: String,
        target: String,
    },

    #[error("[{kind}] skill '{name}' not listed in any domain")]
    OrphanDomain { kind: CheckKind, name: String },

    #[error("[{kind}] domain '{domain}' lists unknown skill '{skill}'")]
    GhostDomainEntry {
        kind: CheckKind,
        domain: String,
        skill: String,
    },

    #[error("[{kind}] skill '{skill}' has domain '{found}' but is listed under '{expected}' in domains index")]
    DomainMismatch {
        kind: CheckKind,
        skill: String,
        found: String,
        expected: String,
    },

    #[error("[{kind}] concern '{concern}' claimed by both '{skill_a}' and '{skill_b}'")]
    DuplicateConcern {
        kind: CheckKind,
        concern: String,
        skill_a: String,
        skill_b: String,
    },

    #[error("[{kind}] skill-map.yaml missing 'version' field")]
    MissingVersion { kind: CheckKind },

    #[error("[{kind}] skill-map.yaml missing 'lastModified' field")]
    MissingLastModified { kind: CheckKind },

    #[error("[{kind}] skill '{skill}' last verified {last_verified}, exceeds {max_days} day threshold")]
    Stale {
        kind: CheckKind,
        skill: String,
        last_verified: String,
        max_days: u32,
    },

    #[error("[{kind}] skill '{skill}' (verified {skill_date}) references '{reference}' (verified {ref_date}) — referenced skill is newer, review needed")]
    ReferenceNewer {
        kind: CheckKind,
        skill: String,
        skill_date: String,
        reference: String,
        ref_date: String,
    },
}

impl LintError {
    /// Extract the [`CheckKind`] that produced this error.
    #[must_use]
    pub fn kind(&self) -> CheckKind {
        match self {
            Self::NoSkillsFound { kind, .. }
            | Self::NoDocsScanned { kind, .. }
            | Self::IndexEntryTooLong { kind, .. }
            | Self::IndexEntryGrew { kind, .. }
            | Self::DocTooLarge { kind, .. }
            | Self::DocGrew { kind, .. }
            | Self::MissingMapEntry { kind, .. }
            | Self::OrphanMapEntry { kind, .. }
            | Self::MissingFrontmatter { kind, .. }
            | Self::NameMismatch { kind, .. }
            | Self::BrokenReference { kind, .. }
            | Self::OrphanDomain { kind, .. }
            | Self::GhostDomainEntry { kind, .. }
            | Self::DomainMismatch { kind, .. }
            | Self::DuplicateConcern { kind, .. }
            | Self::MissingVersion { kind }
            | Self::MissingLastModified { kind }
            | Self::Stale { kind, .. }
            | Self::DescriptionTooLong { kind, .. }
            | Self::UnresolvedPath { kind, .. }
            | Self::ReferenceNewer { kind, .. } => *kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_kind_display() {
        assert_eq!(CheckKind::Discovery.to_string(), "discovery");
        assert_eq!(CheckKind::Version.to_string(), "version");
        assert_eq!(CheckKind::Sync.to_string(), "sync");
        assert_eq!(CheckKind::Frontmatter.to_string(), "frontmatter");
        assert_eq!(CheckKind::MapIntegrity.to_string(), "map-integrity");
        assert_eq!(CheckKind::Staleness.to_string(), "staleness");
        assert_eq!(CheckKind::References.to_string(), "references");
        assert_eq!(CheckKind::ListingBudget.to_string(), "listing-budget");
        assert_eq!(CheckKind::PathResolution.to_string(), "path-resolution");
        assert_eq!(CheckKind::ClaudeMdEntry.to_string(), "claudemd-entry");
        assert_eq!(CheckKind::ClaudeMdFile.to_string(), "claudemd-file");
    }

    /// The message has to carry the measurement AND the move that fixes it.
    /// "Too long" without a byte count is unarguable; a byte count without the
    /// remedy sends the reader looking for a policy that lives somewhere else.
    #[test]
    fn index_entry_message_carries_the_measurement_and_the_remedy() {
        let err = LintError::IndexEntryTooLong {
            kind: CheckKind::ClaudeMdEntry,
            file: "docs/pleme-io-CLAUDE.md".into(),
            entry: "OPERATING-THEORY".into(),
            line: 2529,
            bytes: 6365,
            cap: 400,
            over: 5965,
        };
        let msg = err.to_string();
        assert!(msg.contains("[claudemd-entry]"), "kind tag missing: {msg}");
        assert!(msg.contains("docs/pleme-io-CLAUDE.md:2529"), "location missing: {msg}");
        assert!(msg.contains("OPERATING-THEORY"), "entry missing: {msg}");
        assert!(msg.contains("6365 B") && msg.contains("400 B"), "sizes missing: {msg}");
        assert!(msg.contains("--write-baseline"), "baseline escape hatch missing: {msg}");
    }

    /// The growth message must say it is growth, not merely size — the reader's
    /// action differs (move the NEW material out, not the whole entry).
    #[test]
    fn growth_messages_name_the_baseline_they_exceeded() {
        let entry = LintError::IndexEntryGrew {
            kind: CheckKind::ClaudeMdEntry,
            file: "docs/CLAUDE.md".into(),
            entry: "BUILD".into(),
            line: 10,
            bytes: 2000,
            recorded: 1400,
            grew: 600,
        };
        assert!(entry.to_string().contains("grew to 2000 B from the 1400 B"), "{entry}");

        let doc = LintError::DocGrew {
            kind: CheckKind::ClaudeMdFile,
            file: "docs/CLAUDE.md".into(),
            bytes: 300_000,
            recorded: 282_648,
            grew: 17_352,
        };
        assert!(doc.to_string().contains("regrowth"), "{doc}");
    }

    /// A vacuous run must say what it looked for, or the operator cannot tell a
    /// mis-pointed `--file` from an empty corpus.
    #[test]
    fn no_docs_scanned_names_what_was_searched() {
        let err = LintError::NoDocsScanned {
            kind: CheckKind::Discovery,
            searched: "<no files given>".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("vacuous pass"), "{msg}");
        assert!(msg.contains("<no files given>"), "{msg}");
    }

    #[test]
    fn path_form_display() {
        assert_eq!(PathForm::RelativeLink.to_string(), "relative link");
        assert_eq!(PathForm::RepoPath.to_string(), "repo-relative path");
    }

    /// The message has to carry everything the reader needs to act: which
    /// skill, which path, and the exact waiver line to write if the target is
    /// legitimately absent. An error that only says "broken" sends the reader
    /// hunting — the very cost this check exists to remove.
    #[test]
    fn unresolved_path_message_is_actionable() {
        let err = LintError::UnresolvedPath {
            kind: CheckKind::PathResolution,
            skill: "iac-merge".into(),
            path: "eclusa/docs/CLI.md".into(),
            form: PathForm::RepoPath,
        };
        let msg = err.to_string();
        assert!(msg.contains("[path-resolution]"), "kind tag missing: {msg}");
        assert!(msg.contains("iac-merge"), "skill missing: {msg}");
        assert!(msg.contains("repo-relative path"), "form missing: {msg}");
        assert!(
            msg.contains("pending-path: eclusa/docs/CLI.md"),
            "waiver escape hatch missing: {msg}"
        );
    }

    #[test]
    fn lint_error_kind_extraction() {
        let cases: Vec<(LintError, CheckKind)> = vec![
            (LintError::NoSkillsFound { kind: CheckKind::Discovery, searched: ".".into() }, CheckKind::Discovery),
            (LintError::MissingMapEntry { kind: CheckKind::Sync, name: "x".into() }, CheckKind::Sync),
            (LintError::OrphanMapEntry { kind: CheckKind::Sync, name: "x".into() }, CheckKind::Sync),
            (LintError::MissingFrontmatter { kind: CheckKind::Frontmatter, skill: "x".into(), field: "y".into() }, CheckKind::Frontmatter),
            (LintError::NameMismatch { kind: CheckKind::Frontmatter, skill: "x".into(), found: "a".into(), expected: "b".into() }, CheckKind::Frontmatter),
            (LintError::BrokenReference { kind: CheckKind::MapIntegrity, skill: "x".into(), target: "y".into() }, CheckKind::MapIntegrity),
            (LintError::OrphanDomain { kind: CheckKind::MapIntegrity, name: "x".into() }, CheckKind::MapIntegrity),
            (LintError::GhostDomainEntry { kind: CheckKind::MapIntegrity, domain: "d".into(), skill: "x".into() }, CheckKind::MapIntegrity),
            (LintError::DomainMismatch { kind: CheckKind::MapIntegrity, skill: "x".into(), found: "a".into(), expected: "b".into() }, CheckKind::MapIntegrity),
            (LintError::DuplicateConcern { kind: CheckKind::MapIntegrity, concern: "c".into(), skill_a: "a".into(), skill_b: "b".into() }, CheckKind::MapIntegrity),
            (LintError::MissingVersion { kind: CheckKind::Version }, CheckKind::Version),
            (LintError::MissingLastModified { kind: CheckKind::Version }, CheckKind::Version),
            (LintError::Stale { kind: CheckKind::Staleness, skill: "x".into(), last_verified: "d".into(), max_days: 90 }, CheckKind::Staleness),
            (LintError::ReferenceNewer { kind: CheckKind::References, skill: "x".into(), skill_date: "d1".into(), reference: "y".into(), ref_date: "d2".into() }, CheckKind::References),
            (LintError::DescriptionTooLong { kind: CheckKind::ListingBudget, skill: "x".into(), chars: 2, cap: 1, over: 1 }, CheckKind::ListingBudget),
            (LintError::UnresolvedPath { kind: CheckKind::PathResolution, skill: "x".into(), path: "a/b.md".into(), form: PathForm::RepoPath }, CheckKind::PathResolution),
            (LintError::NoDocsScanned { kind: CheckKind::Discovery, searched: "x".into() }, CheckKind::Discovery),
            (LintError::IndexEntryTooLong { kind: CheckKind::ClaudeMdEntry, file: "f".into(), entry: "e".into(), line: 1, bytes: 2, cap: 1, over: 1 }, CheckKind::ClaudeMdEntry),
            (LintError::IndexEntryGrew { kind: CheckKind::ClaudeMdEntry, file: "f".into(), entry: "e".into(), line: 1, bytes: 2, recorded: 1, grew: 1 }, CheckKind::ClaudeMdEntry),
            (LintError::DocTooLarge { kind: CheckKind::ClaudeMdFile, file: "f".into(), bytes: 2, cap: 1, over: 1 }, CheckKind::ClaudeMdFile),
            (LintError::DocGrew { kind: CheckKind::ClaudeMdFile, file: "f".into(), bytes: 2, recorded: 1, grew: 1 }, CheckKind::ClaudeMdFile),
        ];
        for (err, expected_kind) in cases {
            assert_eq!(err.kind(), expected_kind, "wrong kind for {err}");
        }
    }

    #[test]
    fn lint_error_display_contains_name() {
        let err = LintError::MissingMapEntry {
            kind: CheckKind::Sync,
            name: "my-skill".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("my-skill"), "expected skill name in: {msg}");
        assert!(msg.contains("[sync]"), "expected kind tag in: {msg}");
    }

    #[test]
    fn lint_error_display_stale() {
        let err = LintError::Stale {
            kind: CheckKind::Staleness,
            skill: "old-skill".into(),
            last_verified: "2025-01-01".into(),
            max_days: 90,
        };
        let msg = err.to_string();
        assert!(msg.contains("old-skill"));
        assert!(msg.contains("2025-01-01"));
        assert!(msg.contains("90"));
    }

    #[test]
    fn lint_error_display_reference_newer() {
        let err = LintError::ReferenceNewer {
            kind: CheckKind::References,
            skill: "a".into(),
            skill_date: "2026-01-01".into(),
            reference: "b".into(),
            ref_date: "2026-03-15".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("a"));
        assert!(msg.contains("b"));
        assert!(msg.contains("2026-01-01"));
        assert!(msg.contains("2026-03-15"));
    }

    #[test]
    fn check_kind_equality_and_copy() {
        let k1 = CheckKind::Version;
        let k2 = k1;
        assert_eq!(k1, k2);
    }

    #[test]
    fn check_kind_display_fromstr_roundtrip() {
        let kinds = [
            CheckKind::Discovery,
            CheckKind::Version,
            CheckKind::Sync,
            CheckKind::Frontmatter,
            CheckKind::MapIntegrity,
            CheckKind::Staleness,
            CheckKind::References,
            CheckKind::ListingBudget,
            CheckKind::PathResolution,
            CheckKind::ClaudeMdEntry,
            CheckKind::ClaudeMdFile,
        ];
        for kind in kinds {
            let s = kind.to_string();
            let parsed: CheckKind = s.parse().unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn check_kind_fromstr_invalid() {
        let err = "bogus".parse::<CheckKind>().unwrap_err();
        assert_eq!(err.to_string(), "unknown check kind: 'bogus'");
    }
}
