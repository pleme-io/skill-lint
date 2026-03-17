use thiserror::Error;

#[derive(Debug, Error)]
pub enum LintError {
    #[error("skill directory '{0}' has no entry in skill-map.yaml")]
    MissingMapEntry(String),

    #[error("map entry '{0}' has no skill directory")]
    OrphanMapEntry(String),

    #[error("skill '{skill}': frontmatter field '{field}' is missing")]
    MissingFrontmatter { skill: String, field: String },

    #[error("skill '{skill}': name '{found}' does not match directory '{expected}'")]
    NameMismatch {
        skill: String,
        found: String,
        expected: String,
    },

    #[error("skill '{skill}' references unknown skill '{target}'")]
    BrokenReference { skill: String, target: String },

    #[error("skill '{0}' not listed in any domain")]
    OrphanDomain(String),

    #[error("domain '{domain}' lists unknown skill '{skill}'")]
    GhostDomainEntry { domain: String, skill: String },

    #[error("skill-map.yaml missing 'version' field")]
    MissingVersion,

    #[error("skill-map.yaml missing 'lastModified' field")]
    MissingLastModified,
}
