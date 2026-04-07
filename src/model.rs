use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The full skill-map.yaml structure.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
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
///
/// The `domain` field is validated against the filename when loaded
/// from `skill-map.d/{domain}.yaml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub description: String,
    pub domain: String,
    pub repo: String,
    #[serde(default)]
    pub concerns: Vec<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

/// Root config for the skill map (`skill-map.d/config.yaml`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillMapConfig {
    pub version: Option<String>,
    pub last_modified: Option<String>,
}

/// YAML frontmatter parsed from a SKILL.md file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default, alias = "allowed-tools")]
    pub allowed_tools: Option<String>,
    #[serde(default)]
    pub metadata: Option<SkillMetadata>,
}

/// Version and recency metadata embedded in frontmatter.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct SkillMetadata {
    /// Semantic version of the skill document.
    pub version: Option<String>,
    /// ISO-8601 date when the skill was last reviewed.
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
    Ok(serde_yaml_ng::from_str(yaml)?)
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
    fn parse_missing_opening_delimiter() {
        assert!(parse_frontmatter("name: broken\n").is_err());
    }

    #[test]
    fn parse_missing_closing_delimiter() {
        assert!(parse_frontmatter("---\nname: broken\n").is_err());
    }

    #[test]
    fn parse_empty_frontmatter() {
        let fm = parse_frontmatter("---\n---\n# Body").unwrap();
        assert!(fm.name.is_none());
    }

    #[test]
    fn deserialize_skill_map() {
        let yaml = "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains:\n  rust: [rust-binary]\n  meta: [claude-skills]\nskills:\n  rust-binary:\n    description: Build\n    domain: rust\n    repo: bp\n  claude-skills:\n    description: Meta\n    domain: meta\n    repo: bp\n";
        let map: SkillMap = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(map.skills.len(), 2);
        assert_eq!(map.domains.len(), 2);
        assert_eq!(map.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn deserialize_empty_map() {
        let yaml = "skills: {}\ndomains: {}\n";
        let map: SkillMap = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(map.skills.is_empty());
        assert!(map.version.is_none());
    }

    #[test]
    fn skill_map_serialize_roundtrip() {
        let mut map = SkillMap::default();
        map.version = Some("1.0.0".into());
        map.skills.insert("test".into(), SkillEntry {
            description: "A test".into(),
            domain: "meta".into(),
            repo: "test-repo".into(),
            concerns: vec!["testing".into()],
            references: vec![],
        });
        let yaml = serde_yaml_ng::to_string(&map).unwrap();
        let map2: SkillMap = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(map, map2);
    }

    #[test]
    fn frontmatter_default() {
        let fm = SkillFrontmatter::default();
        assert!(fm.name.is_none());
        assert!(fm.metadata.is_none());
    }

    #[test]
    fn parse_frontmatter_with_leading_whitespace() {
        let content = "  \n---\nname: trimmed\n---\n# Body";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm.name.as_deref(), Some("trimmed"));
    }

    #[test]
    fn parse_frontmatter_allowed_tools_alias() {
        let content = "---\nname: s\nallowed-tools: Read, Bash\n---\n";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm.allowed_tools.as_deref(), Some("Read, Bash"));
    }

    #[test]
    fn parse_frontmatter_allowed_tools_underscore() {
        let content = "---\nname: s\nallowed_tools: Write\n---\n";
        let fm = parse_frontmatter(content).unwrap();
        assert_eq!(fm.allowed_tools.as_deref(), Some("Write"));
    }

    #[test]
    fn deserialize_skill_entry_minimal() {
        let yaml = "description: Build stuff\ndomain: rust\nrepo: bp\n";
        let entry: SkillEntry = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(entry.description, "Build stuff");
        assert!(entry.concerns.is_empty());
        assert!(entry.references.is_empty());
    }

    #[test]
    fn deserialize_skill_entry_with_concerns_and_refs() {
        let yaml = "description: X\ndomain: d\nrepo: r\nconcerns: [a, b]\nreferences: [c]\n";
        let entry: SkillEntry = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(entry.concerns, vec!["a", "b"]);
        assert_eq!(entry.references, vec!["c"]);
    }

    #[test]
    fn skill_map_config_default() {
        let cfg = SkillMapConfig::default();
        assert!(cfg.version.is_none());
        assert!(cfg.last_modified.is_none());
    }

    #[test]
    fn skill_map_config_roundtrip() {
        let yaml = "version: \"2.0.0\"\nlastModified: \"2026-04-01\"\n";
        let cfg: SkillMapConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.version.as_deref(), Some("2.0.0"));
        assert_eq!(cfg.last_modified.as_deref(), Some("2026-04-01"));
    }

    #[test]
    fn skill_map_default_has_empty_collections() {
        let map = SkillMap::default();
        assert!(map.skills.is_empty());
        assert!(map.domains.is_empty());
        assert!(map.version.is_none());
        assert!(map.last_modified.is_none());
    }

    #[test]
    fn skill_metadata_default() {
        let m = SkillMetadata::default();
        assert!(m.version.is_none());
        assert!(m.last_verified.is_none());
    }
}
