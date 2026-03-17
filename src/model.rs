use std::collections::BTreeMap;

use serde::Deserialize;

/// The full skill-map.yaml structure.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillMap {
    pub version: Option<String>,
    pub last_modified: Option<String>,
    #[serde(default)]
    pub domains: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub skills: BTreeMap<String, SkillEntry>,
}

/// A single skill entry in the map.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub description: String,
    pub domain: String,
    pub repo: String,
    #[serde(default)]
    pub concerns: Vec<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub anti_overlap: Vec<String>,
}

/// YAML frontmatter parsed from a SKILL.md file.
/// Keys use kebab-case (e.g. `allowed-tools`) matching the SKILL.md convention.
#[derive(Debug, Deserialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default, alias = "allowed-tools")]
    pub allowed_tools: Option<String>,
    #[serde(default)]
    pub metadata: Option<SkillMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct SkillMetadata {
    pub version: Option<String>,
    pub last_verified: Option<String>,
}

/// Parse YAML frontmatter from a SKILL.md file.
///
/// Expects `---\n...\n---\n` delimiters.
///
/// # Errors
///
/// Returns an error if the frontmatter delimiters are missing or the YAML is invalid.
pub fn parse_frontmatter(content: &str) -> anyhow::Result<SkillFrontmatter> {
    let content = content.trim_start();
    anyhow::ensure!(content.starts_with("---"), "missing opening --- delimiter");
    let rest = &content[3..];
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow::anyhow!("missing closing --- delimiter"))?;
    let yaml = &rest[..end];
    Ok(serde_yaml::from_str(yaml)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_frontmatter() {
        let content = "---\nname: test-skill\ndescription: A test skill\nallowed-tools: Read, Bash\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"\n---\n\n# Body";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("test-skill"));
        assert_eq!(fm.description.as_deref(), Some("A test skill"));
        assert_eq!(fm.metadata.as_ref().unwrap().version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn parse_missing_delimiter() {
        let content = "name: broken\n";
        assert!(parse_frontmatter(content).is_err());
    }

    #[test]
    fn deserialize_skill_map() {
        let yaml = r#"
version: "1.0.0"
lastModified: "2026-03-17"
domains:
  rust: [rust-binary, rust-tool]
  meta: [claude-skills]
skills:
  rust-binary:
    description: Scaffold a Rust binary
    domain: rust
    repo: blackmatter-pleme
    concerns: [Cargo.toml, crate2nix]
    references: [rust-tool]
    antiOverlap: [docker]
  rust-tool:
    description: Scaffold a Rust CLI tool
    domain: rust
    repo: blackmatter-pleme
    concerns: [releases, cross-platform]
    references: [rust-binary]
    antiOverlap: []
  claude-skills:
    description: Maintain skills
    domain: meta
    repo: blackmatter-pleme
    concerns: [SKILL.md, skill-map]
    references: []
    antiOverlap: []
"#;
        let map: SkillMap = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(map.skills.len(), 3);
        assert_eq!(map.domains.len(), 2);
        assert_eq!(map.version.as_deref(), Some("1.0.0"));
    }
}
