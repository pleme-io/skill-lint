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
