use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::error::{CheckKind, LintError};
use crate::model::{self, SkillMap};

// ═══════════════════════════════════════════════════════════════════
// Trait: abstract skill source for testability
// ═══════════════════════════════════════════════════════════════════

/// Abstraction over skill data sources. Enables mock testing without
/// filesystem access.
pub trait SkillSource {
    /// Return the parsed skill map.
    fn skill_map(&self) -> anyhow::Result<SkillMap>;

    /// Return the set of skill directory names (dirs containing SKILL.md).
    fn skill_dirs(&self) -> anyhow::Result<BTreeSet<String>>;

    /// Return the raw content of a skill's SKILL.md file.
    fn skill_content(&self, name: &str) -> anyhow::Result<String>;
}

// ═══════════════════════════════════════════════════════════════════
// Filesystem implementation
// ═══════════════════════════════════════════════════════════════════

/// Reads skills from a directory on disk.
pub struct FsSource<'a> {
    pub skills_dir: &'a Path,
}

impl SkillSource for FsSource<'_> {
    fn skill_map(&self) -> anyhow::Result<SkillMap> {
        let path = self.skills_dir.join("skill-map.yaml");
        anyhow::ensure!(path.exists(), "skill-map.yaml not found in {}", self.skills_dir.display());
        let content = fs::read_to_string(&path)?;
        Ok(serde_yaml::from_str(&content)?)
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
        Ok(fs::read_to_string(
            self.skills_dir.join(name).join("SKILL.md"),
        )?)
    }
}

// ═══════════════════════════════════════════════════════════════════
// Report
// ═══════════════════════════════════════════════════════════════════

/// Result of running all checks.
pub struct Report {
    pub errors: Vec<LintError>,
    pub skills_checked: usize,
}

impl Report {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Filter errors by check kind.
    #[must_use]
    pub fn errors_of(&self, kind: CheckKind) -> Vec<&LintError> {
        self.errors.iter().filter(|e| e.kind() == kind).collect()
    }
}

// ═══════════════════════════════════════════════════════════════════
// Orchestrator
// ═══════════════════════════════════════════════════════════════════

/// Run all checks against a skill source.
///
/// # Errors
///
/// Returns an I/O or parse error if the source can't be read.
pub fn check_all(source: &dyn SkillSource) -> anyhow::Result<Report> {
    let map = source.skill_map()?;
    let dir_names = source.skill_dirs()?;
    let map_names: BTreeSet<String> = map.skills.keys().cloned().collect();

    let mut errors = Vec::new();

    check_version(&map, &mut errors);
    check_sync(&dir_names, &map_names, &mut errors);
    check_frontmatter(source, &dir_names, &mut errors);
    check_map_integrity(&map, &map_names, &mut errors);
    check_duplicate_concerns(&map, &mut errors);

    Ok(Report {
        errors,
        skills_checked: dir_names.len(),
    })
}

/// Convenience wrapper for filesystem paths.
///
/// # Errors
///
/// Returns an error if the directory or map can't be read.
pub fn check_path(skills_dir: &Path) -> anyhow::Result<Report> {
    check_all(&FsSource { skills_dir })
}

// ═══════════════════════════════════════════════════════════════════
// Individual checks (all public for independent testing)
// ═══════════════════════════════════════════════════════════════════

/// Check that version and lastModified are present.
pub fn check_version(map: &SkillMap, errors: &mut Vec<LintError>) {
    if map.version.is_none() {
        errors.push(LintError::MissingVersion {
            kind: CheckKind::Version,
        });
    }
    if map.last_modified.is_none() {
        errors.push(LintError::MissingLastModified {
            kind: CheckKind::Version,
        });
    }
}

/// Check that every skill dir has a map entry and vice versa.
pub fn check_sync(
    dir_names: &BTreeSet<String>,
    map_names: &BTreeSet<String>,
    errors: &mut Vec<LintError>,
) {
    for name in dir_names {
        if !map_names.contains(name) {
            errors.push(LintError::MissingMapEntry {
                kind: CheckKind::Sync,
                name: name.clone(),
            });
        }
    }
    for name in map_names {
        if !dir_names.contains(name) {
            errors.push(LintError::OrphanMapEntry {
                kind: CheckKind::Sync,
                name: name.clone(),
            });
        }
    }
}

/// Check that every SKILL.md has valid frontmatter.
pub fn check_frontmatter(
    source: &dyn SkillSource,
    dir_names: &BTreeSet<String>,
    errors: &mut Vec<LintError>,
) {
    for name in dir_names {
        let content = match source.skill_content(name) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let fm = match model::parse_frontmatter(&content) {
            Ok(fm) => fm,
            Err(_) => {
                errors.push(LintError::MissingFrontmatter {
                    kind: CheckKind::Frontmatter,
                    skill: name.clone(),
                    field: "frontmatter (parse error)".into(),
                });
                continue;
            }
        };

        match &fm.name {
            Some(n) if n != name => {
                errors.push(LintError::NameMismatch {
                    kind: CheckKind::Frontmatter,
                    skill: name.clone(),
                    found: n.clone(),
                    expected: name.clone(),
                });
            }
            None => {
                errors.push(LintError::MissingFrontmatter {
                    kind: CheckKind::Frontmatter,
                    skill: name.clone(),
                    field: "name".into(),
                });
            }
            _ => {}
        }

        if fm.description.is_none() {
            errors.push(LintError::MissingFrontmatter {
                kind: CheckKind::Frontmatter,
                skill: name.clone(),
                field: "description".into(),
            });
        }

        match &fm.metadata {
            Some(m) => {
                if m.version.is_none() {
                    errors.push(LintError::MissingFrontmatter {
                        kind: CheckKind::Frontmatter,
                        skill: name.clone(),
                        field: "metadata.version".into(),
                    });
                }
                if m.last_verified.is_none() {
                    errors.push(LintError::MissingFrontmatter {
                        kind: CheckKind::Frontmatter,
                        skill: name.clone(),
                        field: "metadata.last_verified".into(),
                    });
                }
            }
            None => {
                errors.push(LintError::MissingFrontmatter {
                    kind: CheckKind::Frontmatter,
                    skill: name.clone(),
                    field: "metadata".into(),
                });
            }
        }
    }
}

/// Check references, domain index consistency, and domain field matching.
pub fn check_map_integrity(
    map: &SkillMap,
    map_names: &BTreeSet<String>,
    errors: &mut Vec<LintError>,
) {
    // All references must point to existing skills
    for (name, entry) in &map.skills {
        for r in &entry.references {
            if !map_names.contains(r) {
                errors.push(LintError::BrokenReference {
                    kind: CheckKind::MapIntegrity,
                    skill: name.clone(),
                    target: r.clone(),
                });
            }
        }
    }

    // Build reverse index: which domain lists each skill
    let mut domain_of_skill: BTreeMap<String, String> = BTreeMap::new();
    for (domain_name, members) in &map.domains {
        for member in members {
            if !map_names.contains(member) {
                errors.push(LintError::GhostDomainEntry {
                    kind: CheckKind::MapIntegrity,
                    domain: domain_name.clone(),
                    skill: member.clone(),
                });
            }
            domain_of_skill.insert(member.clone(), domain_name.clone());
        }
    }

    // Every skill must appear in the domains index
    for name in map_names {
        if !domain_of_skill.contains_key(name) {
            errors.push(LintError::OrphanDomain {
                kind: CheckKind::MapIntegrity,
                name: name.clone(),
            });
        }
    }

    // Skill's domain field must match the domain it's listed under
    for (name, entry) in &map.skills {
        if let Some(listed_domain) = domain_of_skill.get(name) {
            if *listed_domain != entry.domain {
                errors.push(LintError::DomainMismatch {
                    kind: CheckKind::MapIntegrity,
                    skill: name.clone(),
                    found: entry.domain.clone(),
                    expected: listed_domain.clone(),
                });
            }
        }
    }
}

/// Check that no two skills claim the same concern.
pub fn check_duplicate_concerns(map: &SkillMap, errors: &mut Vec<LintError>) {
    let mut concern_owners: BTreeMap<String, String> = BTreeMap::new();
    for (name, entry) in &map.skills {
        for concern in &entry.concerns {
            let normalized = concern.to_lowercase();
            if let Some(existing) = concern_owners.get(&normalized) {
                errors.push(LintError::DuplicateConcern {
                    kind: CheckKind::MapIntegrity,
                    concern: concern.clone(),
                    skill_a: existing.clone(),
                    skill_b: name.clone(),
                });
            } else {
                concern_owners.insert(normalized, name.clone());
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Mock source ──────────────────────────────────────────────

    /// In-memory skill source for deterministic testing without filesystem.
    struct MockSource {
        map: SkillMap,
        dirs: BTreeSet<String>,
        contents: BTreeMap<String, String>,
    }

    impl MockSource {
        fn new() -> Self {
            Self {
                map: SkillMap {
                    version: Some("1.0.0".into()),
                    last_modified: Some("2026-03-17".into()),
                    domains: BTreeMap::new(),
                    skills: BTreeMap::new(),
                },
                dirs: BTreeSet::new(),
                contents: BTreeMap::new(),
            }
        }

        fn with_skill(mut self, name: &str, domain: &str, frontmatter: &str) -> Self {
            self.dirs.insert(name.into());
            self.contents.insert(
                name.into(),
                format!("---\n{frontmatter}\n---\n\n# Body\n"),
            );
            self.map.skills.insert(
                name.into(),
                model::SkillEntry {
                    description: format!("{name} skill"),
                    domain: domain.into(),
                    repo: "test".into(),
                    concerns: vec![],
                    references: vec![],
                    anti_overlap: vec![],
                },
            );
            // Add to domain index
            self.map
                .domains
                .entry(domain.into())
                .or_default()
                .push(name.into());
            self
        }

        fn with_concern(mut self, skill: &str, concern: &str) -> Self {
            if let Some(entry) = self.map.skills.get_mut(skill) {
                entry.concerns.push(concern.into());
            }
            self
        }

        fn with_reference(mut self, from: &str, to: &str) -> Self {
            if let Some(entry) = self.map.skills.get_mut(from) {
                entry.references.push(to.into());
            }
            self
        }

        fn without_version(mut self) -> Self {
            self.map.version = None;
            self.map.last_modified = None;
            self
        }

        fn without_domain_entry(mut self, skill: &str) -> Self {
            for members in self.map.domains.values_mut() {
                members.retain(|m| m != skill);
            }
            self
        }
    }

    impl SkillSource for MockSource {
        fn skill_map(&self) -> anyhow::Result<SkillMap> {
            Ok(self.map.clone())
        }

        fn skill_dirs(&self) -> anyhow::Result<BTreeSet<String>> {
            Ok(self.dirs.clone())
        }

        fn skill_content(&self, name: &str) -> anyhow::Result<String> {
            self.contents
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("skill {name} not found"))
        }
    }

    fn valid_fm(name: &str) -> String {
        format!("name: {name}\ndescription: A {name} skill\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
    }

    // ─── Happy path ───────────────────────────────────────────────

    #[test]
    fn all_checks_pass() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_skill("beta", "tools", &valid_fm("beta"));
        let report = check_all(&source).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.skills_checked, 2);
    }

    // ─── Version checks ──────────────────────────────────────────

    #[test]
    fn missing_version() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .without_version();
        let report = check_all(&source).unwrap();
        assert_eq!(report.errors_of(CheckKind::Version).len(), 2);
    }

    // ─── Sync checks ─────────────────────────────────────────────

    #[test]
    fn missing_map_entry() {
        let mut source = MockSource::new()
            .with_skill("mapped", "meta", &valid_fm("mapped"));
        // Add dir without map entry
        source.dirs.insert("orphan".into());
        source.contents.insert("orphan".into(), format!("---\n{}\n---\n", valid_fm("orphan")));
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingMapEntry { name, .. } if name == "orphan"
        )));
    }

    #[test]
    fn orphan_map_entry() {
        let mut source = MockSource::new()
            .with_skill("ghost", "meta", &valid_fm("ghost"));
        source.dirs.remove("ghost");
        source.contents.remove("ghost");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::OrphanMapEntry { name, .. } if name == "ghost"
        )));
    }

    // ─── Frontmatter checks ──────────────────────────────────────

    #[test]
    fn name_mismatch() {
        let source = MockSource::new()
            .with_skill("my-skill", "meta", "name: wrong-name\ndescription: X\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::NameMismatch { .. })));
    }

    #[test]
    fn missing_description() {
        let source = MockSource::new()
            .with_skill("no-desc", "meta", "name: no-desc\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "description"
        )));
    }

    #[test]
    fn missing_metadata() {
        let source = MockSource::new()
            .with_skill("no-meta", "meta", "name: no-meta\ndescription: X");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "metadata"
        )));
    }

    #[test]
    fn missing_metadata_version() {
        let source = MockSource::new()
            .with_skill("no-ver", "meta", "name: no-ver\ndescription: X\nmetadata:\n  last_verified: \"2026-01-01\"");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "metadata.version"
        )));
    }

    #[test]
    fn unparseable_frontmatter() {
        let mut source = MockSource::new()
            .with_skill("broken", "meta", &valid_fm("broken"));
        source.contents.insert("broken".into(), "no delimiters here".into());
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field.contains("parse error")
        )));
    }

    // ─── Map integrity checks ────────────────────────────────────

    #[test]
    fn broken_reference() {
        let source = MockSource::new()
            .with_skill("linker", "meta", &valid_fm("linker"))
            .with_reference("linker", "nonexistent");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::BrokenReference { target, .. } if target == "nonexistent"
        )));
    }

    #[test]
    fn valid_reference() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_reference("a", "b");
        let report = check_all(&source).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
    }

    #[test]
    fn orphan_domain() {
        let source = MockSource::new()
            .with_skill("lonely", "meta", &valid_fm("lonely"))
            .without_domain_entry("lonely");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::OrphanDomain { name, .. } if name == "lonely"
        )));
    }

    #[test]
    fn domain_mismatch() {
        let mut source = MockSource::new()
            .with_skill("misplaced", "rust", &valid_fm("misplaced"));
        // Skill says domain=rust but move it to "go" in the index
        source.map.domains.clear();
        source.map.domains.insert("go".into(), vec!["misplaced".into()]);
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::DomainMismatch { skill, found, expected, .. }
            if skill == "misplaced" && found == "rust" && expected == "go"
        )));
    }

    #[test]
    fn ghost_domain_entry() {
        let mut source = MockSource::new()
            .with_skill("real", "meta", &valid_fm("real"));
        source.map.domains.get_mut("meta").unwrap().push("ghost".into());
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::GhostDomainEntry { skill, .. } if skill == "ghost"
        )));
    }

    // ─── Duplicate concern checks ────────────────────────────────

    #[test]
    fn duplicate_concern_detected() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_concern("a", "Cargo.toml")
            .with_concern("b", "Cargo.toml");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::DuplicateConcern { concern, .. } if concern == "Cargo.toml"
        )));
    }

    #[test]
    fn duplicate_concern_case_insensitive() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_concern("a", "Docker")
            .with_concern("b", "docker");
        let report = check_all(&source).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::DuplicateConcern { .. })));
    }

    #[test]
    fn unique_concerns_pass() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_concern("a", "Cargo.toml")
            .with_concern("b", "package.json");
        let report = check_all(&source).unwrap();
        assert!(report.errors_of(CheckKind::MapIntegrity).is_empty());
    }

    // ─── Report filtering ────────────────────────────────────────

    #[test]
    fn report_filters_by_kind() {
        let source = MockSource::new()
            .with_skill("a", "meta", "name: wrong\ndescription: X\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
            .without_version();
        let report = check_all(&source).unwrap();
        assert_eq!(report.errors_of(CheckKind::Version).len(), 2);
        assert_eq!(report.errors_of(CheckKind::Frontmatter).len(), 1);
        assert_eq!(report.errors_of(CheckKind::Sync).len(), 0);
    }

    // ─── Filesystem integration (tempdir) ────────────────────────

    #[test]
    fn filesystem_source_works() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\n{}\n---\n\n# Body\n", valid_fm("test-skill")),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("skill-map.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains:\n  meta: [test-skill]\nskills:\n  test-skill:\n    description: A test\n    domain: meta\n    repo: test\n",
        )
        .unwrap();

        let report = check_path(dir.path()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
    }
}
