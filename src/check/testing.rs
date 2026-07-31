use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::model::{SkillEntry, SkillMap};

use super::{PathOracle, SkillSource, normalize_lexical};

/// In-memory skill source for deterministic testing without filesystem.
/// Use the builder methods to construct test scenarios.
pub struct MockSource {
    /// The in-memory skill map.
    pub map: SkillMap,
    /// Simulated directory names.
    pub dirs: BTreeSet<String>,
    /// Simulated `SKILL.md` contents keyed by skill name.
    pub contents: BTreeMap<String, String>,
    /// Repositories simulated as present on this machine.
    pub repos: BTreeSet<String>,
    /// Files simulated under the repo root, e.g. `theory/THEORY.md`.
    pub repo_files: BTreeSet<PathBuf>,
    /// Files simulated under the skills root, e.g. `alpha/references/a.md`.
    pub skill_files: BTreeSet<PathBuf>,
    /// Name of the directory holding the repos; enables the org-qualified reading.
    pub org: Option<String>,
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
            repos: BTreeSet::new(),
            repo_files: BTreeSet::new(),
            skill_files: BTreeSet::new(),
            org: None,
        }
    }

    /// Simulate a repository being present on this machine.
    ///
    /// Separate from [`Self::with_repo_file`] on purpose: "the repo is here but
    /// the file is not" (a real dead pointer) and "the repo was never cloned"
    /// (unknowable, must stay silent) are different states, and the gate that
    /// tells them apart is only testable if a test can build each.
    #[must_use]
    pub fn with_repo(mut self, repo: &str) -> Self {
        self.repos.insert(repo.into());
        self
    }

    /// Name the directory that holds the repos, enabling the org-qualified
    /// `<org>/<repo>/<path>` reading. Absent by default so existing tests keep
    /// exercising the bare form alone.
    #[must_use]
    pub fn with_org(mut self, org: &str) -> Self {
        self.org = Some(org.into());
        self
    }

    /// Simulate a file under the repo root, and the repository holding it.
    #[must_use]
    pub fn with_repo_file(mut self, path: &str) -> Self {
        if let Some(repo) = path.split('/').next() {
            self.repos.insert(repo.into());
        }
        self.repo_files.insert(normalize_lexical(&PathBuf::from(path)));
        self
    }

    /// Simulate a file under the skills root, e.g. `alpha/references/a.md`.
    #[must_use]
    pub fn with_skill_file(mut self, path: &str) -> Self {
        self.skill_files.insert(normalize_lexical(&PathBuf::from(path)));
        self
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

    /// Replace a skill's body, keeping its frontmatter valid.
    #[must_use]
    pub fn with_body(mut self, skill: &str, body: &str) -> Self {
        self.contents
            .insert(skill.into(), format!("---\n{}\n---\n\n{body}\n", valid_fm(skill)));
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

    fn path_oracle(&self) -> Option<Box<dyn PathOracle>> {
        Some(Box::new(MockPathOracle {
            repos: self.repos.clone(),
            repo_files: self.repo_files.clone(),
            skill_files: self.skill_files.clone(),
            org: self.org.clone(),
        }))
    }
}

/// In-memory [`PathOracle`] — a virtual filesystem of exactly the paths a test
/// declared, so path resolution is exercised without touching disk.
pub struct MockPathOracle {
    repos: BTreeSet<String>,
    repo_files: BTreeSet<PathBuf>,
    skill_files: BTreeSet<PathBuf>,
    /// Name of the directory holding the repos, so the org-qualified reading is
    /// exercisable in-memory. `None` models a source with no identifiable root.
    org: Option<String>,
}

impl PathOracle for MockPathOracle {
    fn exists_near_skill(&self, skill: &str, rel: &str) -> bool {
        self.skill_files.contains(&normalize_lexical(&PathBuf::from(skill).join(rel)))
    }

    fn has_repo(&self, repo: &str) -> bool { self.repos.contains(repo) }

    fn exists_under_repo_root(&self, rel: &str) -> bool {
        self.repo_files.contains(&normalize_lexical(&PathBuf::from(rel)))
    }

    fn org_segment(&self) -> Option<String> { self.org.clone() }
}

/// Valid frontmatter string for a given skill name.
#[must_use]
pub fn valid_fm(name: &str) -> String {
    format!("name: {name}\ndescription: A {name} skill\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
}
