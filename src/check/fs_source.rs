use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::model::{SkillEntry, SkillMap};

use super::SkillSource;

/// Filesystem-backed [`SkillSource`] that reads skills and maps from disk.
pub struct FsSource<'a> {
    /// Root directory containing skill subdirectories.
    pub skills_dir: &'a Path,
    /// Override for skill-map.d/ location. If None, searches:
    /// 1. {skills_dir}/skill-map.d/
    /// 2. {skills_dir}/../skill-map.d/
    /// 3. {skills_dir}/skill-map.yaml (legacy)
    pub map_dir_override: Option<&'a Path>,
}

impl FsSource<'_> {
    fn load_split_map(map_dir: &Path) -> anyhow::Result<SkillMap> {
        use crate::model::SkillMapConfig;

        let config_path = map_dir.join("config.yaml");
        let config: SkillMapConfig = if config_path.exists() {
            serde_yaml_ng::from_str(&fs::read_to_string(&config_path)?)?
        } else {
            SkillMapConfig::default()
        };

        let mut skills = BTreeMap::new();
        let mut domains = BTreeMap::new();

        let mut paths: Vec<_> = fs::read_dir(map_dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "yaml")
                    && p.file_stem().is_some_and(|s| s != "config")
            })
            .collect();
        paths.sort();

        for path in paths {
            let domain = path.file_stem()
                .and_then(|s| s.to_str())
                .map(ToOwned::to_owned)
                .unwrap_or_default();
            let content = fs::read_to_string(&path)?;
            let domain_skills: BTreeMap<String, SkillEntry> =
                serde_yaml_ng::from_str(&content)?;

            let members: Vec<String> = domain_skills.keys().cloned().collect();
            skills.extend(domain_skills);
            domains.insert(domain, members);
        }

        Ok(SkillMap {
            version: config.version,
            last_modified: config.last_modified,
            domains,
            skills,
        })
    }
}

impl SkillSource for FsSource<'_> {
    fn skill_map(&self) -> anyhow::Result<SkillMap> {
        if let Some(dir) = self.map_dir_override {
            anyhow::ensure!(dir.is_dir(), "map-dir {} does not exist", dir.display());
            return Self::load_split_map(dir);
        }

        let map_dir = self.skills_dir.join("skill-map.d");
        if map_dir.is_dir() {
            return Self::load_split_map(&map_dir);
        }

        if let Some(parent) = self.skills_dir.parent() {
            let sibling = parent.join("skill-map.d");
            if sibling.is_dir() {
                return Self::load_split_map(&sibling);
            }
        }

        let path = self.skills_dir.join("skill-map.yaml");
        anyhow::ensure!(
            path.exists(),
            "skill-map.d/ or skill-map.yaml not found for {}",
            self.skills_dir.display()
        );
        let content = fs::read_to_string(&path)?;
        Ok(serde_yaml_ng::from_str(&content)?)
    }

    fn skill_dirs(&self) -> anyhow::Result<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(self.skills_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type()?.is_dir()
                && self.skills_dir.join(&name).join("SKILL.md").exists()
            {
                names.insert(name);
            }
        }
        Ok(names)
    }

    fn skill_content(&self, name: &str) -> anyhow::Result<String> {
        Ok(fs::read_to_string(self.skills_dir.join(name).join("SKILL.md"))?)
    }
}
