use std::fmt;

use thiserror::Error;

/// Which check phase produced the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckKind {
    Version,
    Sync,
    Frontmatter,
    MapIntegrity,
    Staleness,
}

impl fmt::Display for CheckKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version => write!(f, "version"),
            Self::Sync => write!(f, "sync"),
            Self::Frontmatter => write!(f, "frontmatter"),
            Self::MapIntegrity => write!(f, "map-integrity"),
            Self::Staleness => write!(f, "staleness"),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LintError {
    #[error("[{kind}] skill directory '{name}' has no entry in skill-map.yaml")]
    MissingMapEntry { kind: CheckKind, name: String },

    #[error("[{kind}] map entry '{name}' has no skill directory")]
    OrphanMapEntry { kind: CheckKind, name: String },

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
}

impl LintError {
    #[must_use]
    pub fn kind(&self) -> CheckKind {
        match self {
            Self::MissingMapEntry { kind, .. }
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
            | Self::Stale { kind, .. } => *kind,
        }
    }
}
