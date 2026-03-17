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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
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
        let map: SkillMap = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(map.skills.len(), 2);
        assert_eq!(map.domains.len(), 2);
        assert_eq!(map.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn deserialize_empty_map() {
        let yaml = "skills: {}\ndomains: {}\n";
        let map: SkillMap = serde_yaml::from_str(yaml).unwrap();
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
        let yaml = serde_yaml::to_string(&map).unwrap();
        let map2: SkillMap = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(map, map2);
    }

    #[test]
    fn frontmatter_default() {
        let fm = SkillFrontmatter::default();
        assert!(fm.name.is_none());
        assert!(fm.metadata.is_none());
    }
}
