use std::collections::{BTreeMap, BTreeSet};

use crate::model::{SkillEntry, SkillMap};

use super::SkillSource;

/// In-memory skill source for deterministic testing without filesystem.
/// Use the builder methods to construct test scenarios.
pub struct MockSource {
    /// The in-memory skill map.
    pub map: SkillMap,
    /// Simulated directory names.
    pub dirs: BTreeSet<String>,
    /// Simulated `SKILL.md` contents keyed by skill name.
    pub contents: BTreeMap<String, String>,
}

impl MockSource {
    /// Create a source pre-loaded with version/`lastModified` but no skills.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: SkillMap {
                version: Some("1.0.0".into()),
                last_modified: Some("2026-03-17".into()),
                ..SkillMap::default()
            },
            dirs: BTreeSet::new(),
            contents: BTreeMap::new(),
        }
    }

    /// Add a fully-wired skill: directory, content, map entry, and domain listing.
    #[must_use]
    pub fn with_skill(mut self, name: &str, domain: &str, frontmatter: &str) -> Self {
        self.dirs.insert(name.into());
        self.contents.insert(name.into(), format!("---\n{frontmatter}\n---\n\n# Body\n"));
        self.map.skills.insert(name.into(), SkillEntry {
            description: format!("{name} skill"),
            domain: domain.into(),
            repo: "test".into(),
            concerns: vec![],
            references: vec![],
        });
        self.map.domains.entry(domain.into()).or_default().push(name.into());
        self
    }

    /// Append a concern to an existing skill's entry.
    #[must_use]
    pub fn with_concern(mut self, skill: &str, concern: &str) -> Self {
        if let Some(entry) = self.map.skills.get_mut(skill) {
            entry.concerns.push(concern.into());
        }
        self
    }

    /// Add a reference edge from one skill to another.
    #[must_use]
    pub fn with_reference(mut self, from: &str, to: &str) -> Self {
        if let Some(entry) = self.map.skills.get_mut(from) {
            entry.references.push(to.into());
        }
        self
    }

    /// Remove `version` and `lastModified` from the map.
    #[must_use]
    pub fn without_version(mut self) -> Self {
        self.map.version = None;
        self.map.last_modified = None;
        self
    }

    /// Remove a skill from all domain listings (but keep the map entry).
    #[must_use]
    pub fn without_domain_entry(mut self, skill: &str) -> Self {
        for members in self.map.domains.values_mut() {
            members.retain(|m| m != skill);
        }
        self
    }

    /// Remove the simulated directory and content for a skill (keeps map entry).
    #[must_use]
    pub fn without_dir(mut self, skill: &str) -> Self {
        self.dirs.remove(skill);
        self.contents.remove(skill);
        self
    }

    /// Override the raw `SKILL.md` content for a skill.
    #[must_use]
    pub fn with_raw_content(mut self, skill: &str, content: &str) -> Self {
        self.contents.insert(skill.into(), content.into());
        self
    }
}

impl Default for MockSource {
    fn default() -> Self { Self::new() }
}

impl SkillSource for MockSource {
    fn skill_map(&self) -> anyhow::Result<SkillMap> { Ok(self.map.clone()) }
    fn skill_dirs(&self) -> anyhow::Result<BTreeSet<String>> { Ok(self.dirs.clone()) }
    fn skill_content(&self, name: &str) -> anyhow::Result<String> {
        self.contents.get(name).cloned()
            .ok_or_else(|| anyhow::anyhow!("skill {name} not found"))
    }
}

/// Valid frontmatter string for a given skill name.
#[must_use]
pub fn valid_fm(name: &str) -> String {
    format!("name: {name}\ndescription: A {name} skill\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
}
