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
    /// Do the `/skill-name` slash-commands a body names still resolve?
    ///
    /// Distinct from [`Self::PathResolution`], which resolves filesystem paths.
    /// A bare `` `/vocabularify` `` is neither a markdown link nor a
    /// `<repo>/<path>`, so the path matcher rejects it outright and the class
    /// was **structurally invisible** rather than merely under-gated: measured
    /// 2026-07-31, four dead pointers were present while the run passed green
    /// both with and without `--skip-path-resolution`.
    ///
    /// Resolves against the skill listing, which travels with the corpus, so
    /// unlike path resolution it is never knowably-unavailable and has no
    /// skip flag. A token that is legitimately not a skill (an HTTP route
    /// shares the form) is declared with a scoped `pending-skill-pointer:`.
    SkillPointer,
    /// Is every declared tier ledger honest about its ceilings?
    ///
    /// `selo::SealTier::OnlyMitigated(Ceiling)` makes an unnamed-ceiling
    /// mitigation unconstructible in Rust. The ledgers that get written are
    /// markdown tables, which typecheck against nothing — so the same
    /// constraint is enforced here, over `selo`'s vocabulary.
    TierLedger,
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
    /// Inline shell in a GitHub Actions `run:` block.
    ///
    /// The fleet PRIME DIRECTIVE allows ~3 lines of inline glue and no more. The
    /// shape that evades it is not a `.sh` file — it is a block-form `run:`,
    /// because that does not look like a shell script, it looks like YAML with a
    /// long string in it. Measured 2026-08-06: an agent wrote a ~15-line `run:`
    /// (embedded `nix eval`, `grep -qx`, if/else) into substrate's
    /// `rust-auto-release.yml` DURING the session it spent enforcing that rule.
    /// The rule was stated in two places and still lost — unenforced, not weak.
    WorkflowRun,
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
            Self::SkillPointer => write!(f, "skill-pointer"),
            Self::TierLedger => write!(f, "tier-ledger"),
            Self::ClaudeMdEntry => write!(f, "claudemd-entry"),
            Self::ClaudeMdFile => write!(f, "claudemd-file"),
            Self::WorkflowRun => write!(f, "workflow-run"),
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
            "skill-pointer" => Ok(Self::SkillPointer),
            "tier-ledger" => Ok(Self::TierLedger),
            "claudemd-entry" => Ok(Self::ClaudeMdEntry),
            "claudemd-file" => Ok(Self::ClaudeMdFile),
            "workflow-run" => Ok(Self::WorkflowRun),
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

    /// The workflow sibling of [`Self::NoDocsScanned`]. Same rule, and it earns
    /// its own variant rather than a shared one because the remedy differs: the
    /// operator needs to be told the flag AND that a directory is not a file.
    #[error("[{kind}] no workflow files were scanned (asked for: {searched}) — a run that lints zero workflows is a vacuous pass, not a success. Pass --file <PATH> once per workflow (e.g. --file .github/workflows/ci.yml); a directory is not a file.")]
    NoWorkflowsScanned { kind: CheckKind, searched: String },

    #[error("[{kind}] {file}:{line} step '{step}' runs {lines} lines of inline shell, over the {cap}-line glue allowance by {over}. Inline shell in a `run:` block does not look like a shell script — it looks like YAML with a long string in it, which is exactly why this class keeps landing. Move the logic into a typed tool (a `.tlisp` under tools/ run by pleme-io/actions/tatara-script, or a `nix run .#app`) and leave a one-line invocation here. Comments are not counted, so the WHY can stay. If this is known debt, record it with --write-baseline.")]
    InlineShellTooLong {
        kind: CheckKind,
        file: String,
        step: String,
        line: usize,
        lines: usize,
        cap: usize,
        over: usize,
    },

    #[error("[{kind}] {file}:{line} step '{step}' grew to {lines} lines of inline shell from the {recorded} recorded in the baseline (+{grew}). Baselined shell may shrink or hold, never grow — that is the whole point of the baseline. Put the new logic in a typed tool instead of on the end of this block, or re-record deliberately with --write-baseline.")]
    InlineShellGrew {
        kind: CheckKind,
        file: String,
        step: String,
        line: usize,
        lines: usize,
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

    /// The frontmatter block exists but is not valid YAML.
    ///
    /// Distinct from `MissingFrontmatter` on purpose: reporting a parse failure
    /// as a missing field named "frontmatter (parse error)" sends the reader
    /// looking for an absent key instead of at the syntax, and drops the
    /// parser's line/column entirely. The cause is carried verbatim.
    #[error("[{kind}] skill '{skill}': frontmatter is not valid YAML — {cause}{hint}")]
    UnparseableFrontmatter {
        kind: CheckKind,
        skill: String,
        cause: String,
        hint: String,
    },

    #[error("[{kind}] skill '{skill}': name '{found}' does not match directory '{expected}'")]
    NameMismatch {
        kind: CheckKind,
        skill: String,
        found: String,
        expected: String,
    },

    /// A skill names a skill that does not exist.
    ///
    /// Raised by TWO checks against the same oracle, distinguished by `kind`:
    /// `map-integrity` for a `references:` edge in the YAML, `skill-pointer`
    /// for a `/name` written in a body's prose. One defect, one message; the
    /// tag says which file to open.
    #[error("[{kind}] skill '{skill}' references unknown skill '{target}'")]
    BrokenReference {
        kind: CheckKind,
        skill: String,
        target: String,
    },

    #[error("[{kind}] skill '{skill}': tier-ledger row '{subject}' claims only-mitigated without naming a ceiling. A mitigation whose ceiling is unnamed is a tier rounded up — say WHY it cannot be higher, as `only-mitigated (C1..C6)`. selo makes this unconstructible in Rust (`SealTier::OnlyMitigated(Ceiling)`); a markdown table has to be told.")]
    LedgerMitigationUnbounded {
        kind: CheckKind,
        skill: String,
        subject: String,
    },

    #[error("[{kind}] skill '{skill}': tier-ledger row '{subject}' grades '{tier}', which is not a seal tier. The vocabulary is selo's and closed: truly-unrep | parse-time-rejected | only-mitigated (C1..C6). If this table grades milestones or recon results rather than seals, it is not a tier ledger — drop the <!-- tier-ledger --> marker.")]
    LedgerTierUnknown {
        kind: CheckKind,
        skill: String,
        subject: String,
        tier: String,
    },

    #[error("[{kind}] skill '{skill}' is required to declare a tier-honest ledger and declares none. Add a `<!-- tier-ledger -->` marker above a table whose last column is the tier: truly-unrep | parse-time-rejected | only-mitigated (C1..C6).")]
    LedgerMissing { kind: CheckKind, skill: String },

    #[error("[{kind}] skill '{skill}': a `<!-- tier-ledger -->` marker is followed by no table. A marker pointing at nothing reads as a graded ledger while grading nothing — put the table under it, or remove the marker.")]
    LedgerMalformed { kind: CheckKind, skill: String },

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
            | Self::NoWorkflowsScanned { kind, .. }
            | Self::InlineShellTooLong { kind, .. }
            | Self::InlineShellGrew { kind, .. }
            | Self::MissingMapEntry { kind, .. }
            | Self::OrphanMapEntry { kind, .. }
            | Self::MissingFrontmatter { kind, .. }
            | Self::UnparseableFrontmatter { kind, .. }
            | Self::NameMismatch { kind, .. }
            | Self::BrokenReference { kind, .. }
            | Self::LedgerMitigationUnbounded { kind, .. }
            | Self::LedgerTierUnknown { kind, .. }
            | Self::LedgerMissing { kind, .. }
            | Self::LedgerMalformed { kind, .. }
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
        assert_eq!(CheckKind::SkillPointer.to_string(), "skill-pointer");
        assert_eq!(CheckKind::TierLedger.to_string(), "tier-ledger");
        assert_eq!(CheckKind::ClaudeMdEntry.to_string(), "claudemd-entry");
        assert_eq!(CheckKind::ClaudeMdFile.to_string(), "claudemd-file");
        assert_eq!(CheckKind::WorkflowRun.to_string(), "workflow-run");
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

    /// The message has to name the SHAPE that hid the defect and the move that
    /// fixes it. "Too much shell" without "this looks like YAML, not a script"
    /// leaves the reader with the same blind spot that let it land.
    #[test]
    fn inline_shell_message_names_the_shape_and_the_move() {
        let err = LintError::InlineShellTooLong {
            kind: CheckKind::WorkflowRun,
            file: "rust-auto-release.yml".into(),
            step: "Resolve test environment".into(),
            line: 321,
            lines: 15,
            cap: 3,
            over: 12,
        };
        let msg = err.to_string();
        assert!(msg.contains("[workflow-run]"), "kind tag missing: {msg}");
        assert!(msg.contains("rust-auto-release.yml:321"), "location missing: {msg}");
        assert!(msg.contains("Resolve test environment"), "step missing: {msg}");
        assert!(msg.contains("15 lines") && msg.contains("3-line"), "sizes missing: {msg}");
        assert!(msg.contains("YAML with a long string"), "the shape is not named: {msg}");
        assert!(msg.contains("tatara-script"), "the remedy is not named: {msg}");
        assert!(msg.contains("--write-baseline"), "escape hatch missing: {msg}");
    }

    #[test]
    fn inline_shell_growth_names_the_baseline_it_exceeded() {
        let err = LintError::InlineShellGrew {
            kind: CheckKind::WorkflowRun,
            file: "image-push.yml".into(),
            step: "Build and push".into(),
            line: 60,
            lines: 64,
            recorded: 60,
            grew: 4,
        };
        assert!(err.to_string().contains("grew to 64 lines"), "{err}");
        assert!(err.to_string().contains("from the 60 recorded"), "{err}");
    }

    /// A vacuous workflow run must say what it looked for, and must not silently
    /// accept a directory where a file was wanted.
    #[test]
    fn no_workflows_scanned_names_what_was_searched() {
        let err = LintError::NoWorkflowsScanned {
            kind: CheckKind::Discovery,
            searched: "<no files given>".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("vacuous pass"), "{msg}");
        assert!(msg.contains("<no files given>"), "{msg}");
        assert!(msg.contains("a directory is not a file"), "{msg}");
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
            (LintError::BrokenReference { kind: CheckKind::SkillPointer, skill: "x".into(), target: "y".into() }, CheckKind::SkillPointer),
            (LintError::LedgerMitigationUnbounded { kind: CheckKind::TierLedger, skill: "x".into(), subject: "s".into() }, CheckKind::TierLedger),
            (LintError::LedgerTierUnknown { kind: CheckKind::TierLedger, skill: "x".into(), subject: "s".into(), tier: "t".into() }, CheckKind::TierLedger),
            (LintError::LedgerMissing { kind: CheckKind::TierLedger, skill: "x".into() }, CheckKind::TierLedger),
            (LintError::LedgerMalformed { kind: CheckKind::TierLedger, skill: "x".into() }, CheckKind::TierLedger),
            (LintError::NoDocsScanned { kind: CheckKind::Discovery, searched: "x".into() }, CheckKind::Discovery),
            (LintError::IndexEntryTooLong { kind: CheckKind::ClaudeMdEntry, file: "f".into(), entry: "e".into(), line: 1, bytes: 2, cap: 1, over: 1 }, CheckKind::ClaudeMdEntry),
            (LintError::IndexEntryGrew { kind: CheckKind::ClaudeMdEntry, file: "f".into(), entry: "e".into(), line: 1, bytes: 2, recorded: 1, grew: 1 }, CheckKind::ClaudeMdEntry),
            (LintError::DocTooLarge { kind: CheckKind::ClaudeMdFile, file: "f".into(), bytes: 2, cap: 1, over: 1 }, CheckKind::ClaudeMdFile),
            (LintError::DocGrew { kind: CheckKind::ClaudeMdFile, file: "f".into(), bytes: 2, recorded: 1, grew: 1 }, CheckKind::ClaudeMdFile),
            (LintError::NoWorkflowsScanned { kind: CheckKind::Discovery, searched: "x".into() }, CheckKind::Discovery),
            (LintError::InlineShellTooLong { kind: CheckKind::WorkflowRun, file: "f".into(), step: "s".into(), line: 1, lines: 4, cap: 3, over: 1 }, CheckKind::WorkflowRun),
            (LintError::InlineShellGrew { kind: CheckKind::WorkflowRun, file: "f".into(), step: "s".into(), line: 1, lines: 5, recorded: 4, grew: 1 }, CheckKind::WorkflowRun),
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
            CheckKind::SkillPointer,
            CheckKind::TierLedger,
            CheckKind::ClaudeMdEntry,
            CheckKind::ClaudeMdFile,
            CheckKind::WorkflowRun,
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
