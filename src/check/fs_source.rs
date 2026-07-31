use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{SkillEntry, SkillMap};

use super::{PathOracle, SkillSource, normalize_lexical};

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
    /// Does this directory directly contain at least one `<name>/SKILL.md`?
    fn holds_skills(dir: &Path) -> bool {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.path().join("SKILL.md").is_file())
    }

    /// The directory actually searched for skills.
    ///
    /// `--skills-dir` defaults to `.`, but a repo conventionally keeps its
    /// skills one level down in `skills/`. When the given root holds no
    /// `<name>/SKILL.md` and a `skills/` subdirectory does, descend into it —
    /// so the bare invocation from a repo root, which is what every human and
    /// agent reaches for first, finds the skills instead of silently finding
    /// none.
    ///
    /// This is ONE named convention, deliberately not general recursion:
    /// arbitrary descent would sweep in `.git`, `node_modules`, and the
    /// nested reference directories inside skills themselves, and would
    /// require the source to key skills by path rather than by name. An
    /// explicit `--skills-dir` that already holds skills is never redirected,
    /// so existing callers behave identically.
    fn effective_root(&self) -> PathBuf {
        if !Self::holds_skills(self.skills_dir) {
            let nested = self.skills_dir.join("skills");
            if Self::holds_skills(&nested) {
                return nested;
            }
        }
        self.skills_dir.to_path_buf()
    }

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

        let root = self.effective_root();

        let map_dir = root.join("skill-map.d");
        if map_dir.is_dir() {
            return Self::load_split_map(&map_dir);
        }

        if let Some(parent) = root.parent() {
            let sibling = parent.join("skill-map.d");
            if sibling.is_dir() {
                return Self::load_split_map(&sibling);
            }
        }

        let mut path = root.join("skill-map.yaml");
        // Sibling fallback, symmetric with the skill-map.d/ search above: when
        // discovery descended into `skills/`, a legacy map one level up is
        // still this root's map.
        if !path.exists()
            && let Some(parent) = root.parent()
            && parent.join("skill-map.yaml").exists()
        {
            path = parent.join("skill-map.yaml");
        }
        anyhow::ensure!(
            path.exists(),
            "skill-map.d/ or skill-map.yaml not found for {}",
            root.display()
        );
        let content = fs::read_to_string(&path)?;
        Ok(serde_yaml_ng::from_str(&content)?)
    }

    fn skill_dirs(&self) -> anyhow::Result<BTreeSet<String>> {
        let root = self.effective_root();
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type()?.is_dir() && root.join(&name).join("SKILL.md").exists() {
                names.insert(name);
            }
        }
        Ok(names)
    }

    fn skill_content(&self, name: &str) -> anyhow::Result<String> {
        Ok(fs::read_to_string(self.effective_root().join(name).join("SKILL.md"))?)
    }

    fn origin(&self) -> String {
        self.effective_root().display().to_string()
    }

    fn path_oracle(&self) -> Option<Box<dyn PathOracle>> {
        let root = self.effective_root();
        // Absolute, so `../..` in a link can never walk off the front of a
        // relative base and silently resolve somewhere else.
        let skills_root = fs::canonicalize(&root).unwrap_or(root);
        let repo_root = FsPathOracle::discover_repo_root(&skills_root);
        Some(Box::new(FsPathOracle { skills_root, repo_root }))
    }
}

/// On-disk [`PathOracle`].
///
/// Owns its roots rather than borrowing the source, so it can be handed to a
/// lifetime-free [`super::CheckContext`].
pub struct FsPathOracle {
    /// Directory holding the skill subdirectories.
    skills_root: PathBuf,
    /// Directory holding sibling repositories, when one could be identified.
    /// `None` disables every `<repo>/<path>` check — unknown, not absent.
    repo_root: Option<PathBuf>,
}

impl FsPathOracle {
    /// Find the root that holds sibling repositories.
    ///
    /// Walk up from the skills directory to the first ancestor carrying a
    /// `.git` (the repository the skills live in); its parent is where sibling
    /// repositories sit. Derived rather than configured, because a hand-set
    /// root is one more thing that can drift out from under the check.
    ///
    /// `.git` is tested with `exists`, not `is_dir`: in a worktree or submodule
    /// it is a file.
    fn discover_repo_root(skills_root: &Path) -> Option<PathBuf> {
        let mut current = skills_root;
        loop {
            if current.join(".git").exists() {
                return current.parent().map(Path::to_path_buf);
            }
            current = current.parent()?;
        }
    }
}

impl PathOracle for FsPathOracle {
    fn exists_near_skill(&self, skill: &str, rel: &str) -> bool {
        normalize_lexical(&self.skills_root.join(skill).join(rel)).exists()
    }

    fn has_repo(&self, repo: &str) -> bool {
        self.repo_root.as_ref().is_some_and(|root| root.join(repo).is_dir())
    }

    fn exists_under_repo_root(&self, rel: &str) -> bool {
        self.repo_root
            .as_ref()
            .is_some_and(|root| normalize_lexical(&root.join(rel)).exists())
    }

    fn org_segment(&self) -> Option<String> {
        self.repo_root
            .as_ref()?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}
