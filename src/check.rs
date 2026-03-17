use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::error::LintError;
use crate::model::{self, SkillMap};

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
}

/// Run all checks against a skills directory containing skill subdirectories
/// and a `skill-map.yaml` file.
///
/// # Errors
///
/// Returns an I/O or parse error if the directory or map can't be read.
pub fn check_all(skills_dir: &Path) -> anyhow::Result<Report> {
    let map_path = skills_dir.join("skill-map.yaml");
    anyhow::ensure!(
        map_path.exists(),
        "skill-map.yaml not found in {}",
        skills_dir.display()
    );

    let map_content = fs::read_to_string(&map_path)?;
    let map: SkillMap = serde_yaml::from_str(&map_content)?;

    // Collect skill directory names (subdirs that contain SKILL.md)
    let mut dir_names = BTreeSet::new();
    for entry in fs::read_dir(skills_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type()?.is_dir() && skills_dir.join(&name).join("SKILL.md").exists() {
            dir_names.insert(name);
        }
    }

    let map_names: BTreeSet<String> = map.skills.keys().cloned().collect();

    let mut errors = Vec::new();

    // Check 1: version + lastModified
    check_version(&map, &mut errors);

    // Check 2: sync — dirs vs map entries
    check_sync(&dir_names, &map_names, &mut errors);

    // Check 3: frontmatter — every SKILL.md has valid metadata
    check_frontmatter(skills_dir, &dir_names, &mut errors);

    // Check 4: map integrity — references, domains
    check_map_integrity(&map, &map_names, &mut errors);

    Ok(Report {
        errors,
        skills_checked: dir_names.len(),
    })
}

fn check_version(map: &SkillMap, errors: &mut Vec<LintError>) {
    if map.version.is_none() {
        errors.push(LintError::MissingVersion);
    }
    if map.last_modified.is_none() {
        errors.push(LintError::MissingLastModified);
    }
}

fn check_sync(
    dir_names: &BTreeSet<String>,
    map_names: &BTreeSet<String>,
    errors: &mut Vec<LintError>,
) {
    for name in dir_names {
        if !map_names.contains(name) {
            errors.push(LintError::MissingMapEntry(name.clone()));
        }
    }
    for name in map_names {
        if !dir_names.contains(name) {
            errors.push(LintError::OrphanMapEntry(name.clone()));
        }
    }
}

fn check_frontmatter(
    skills_dir: &Path,
    dir_names: &BTreeSet<String>,
    errors: &mut Vec<LintError>,
) {
    for name in dir_names {
        let skill_md = skills_dir.join(name).join("SKILL.md");
        let content = match fs::read_to_string(&skill_md) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let fm = match model::parse_frontmatter(&content) {
            Ok(fm) => fm,
            Err(_) => {
                errors.push(LintError::MissingFrontmatter {
                    skill: name.clone(),
                    field: "frontmatter (parse error)".into(),
                });
                continue;
            }
        };

        // name must match directory
        match &fm.name {
            Some(n) if n != name => {
                errors.push(LintError::NameMismatch {
                    skill: name.clone(),
                    found: n.clone(),
                    expected: name.clone(),
                });
            }
            None => {
                errors.push(LintError::MissingFrontmatter {
                    skill: name.clone(),
                    field: "name".into(),
                });
            }
            _ => {}
        }

        if fm.description.is_none() {
            errors.push(LintError::MissingFrontmatter {
                skill: name.clone(),
                field: "description".into(),
            });
        }

        // metadata.version and metadata.last_verified
        match &fm.metadata {
            Some(m) => {
                if m.version.is_none() {
                    errors.push(LintError::MissingFrontmatter {
                        skill: name.clone(),
                        field: "metadata.version".into(),
                    });
                }
                if m.last_verified.is_none() {
                    errors.push(LintError::MissingFrontmatter {
                        skill: name.clone(),
                        field: "metadata.last_verified".into(),
                    });
                }
            }
            None => {
                errors.push(LintError::MissingFrontmatter {
                    skill: name.clone(),
                    field: "metadata".into(),
                });
            }
        }
    }
}

fn check_map_integrity(
    map: &SkillMap,
    map_names: &BTreeSet<String>,
    errors: &mut Vec<LintError>,
) {
    // All references must point to existing skills
    for (name, entry) in &map.skills {
        for r in &entry.references {
            if !map_names.contains(r) {
                errors.push(LintError::BrokenReference {
                    skill: name.clone(),
                    target: r.clone(),
                });
            }
        }
    }

    // Every skill must appear in exactly one domain
    let mut domain_covered: BTreeSet<String> = BTreeSet::new();
    for (domain_name, members) in &map.domains {
        for member in members {
            if !map_names.contains(member) {
                errors.push(LintError::GhostDomainEntry {
                    domain: domain_name.clone(),
                    skill: member.clone(),
                });
            }
            domain_covered.insert(member.clone());
        }
    }
    for name in map_names {
        if !domain_covered.contains(name) {
            errors.push(LintError::OrphanDomain(name.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_skill(dir: &Path, name: &str, frontmatter: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\n{frontmatter}\n---\n\n# Body\n"),
        )
        .unwrap();
    }

    fn write_map(dir: &Path, yaml: &str) {
        fs::write(dir.join("skill-map.yaml"), yaml).unwrap();
    }

    #[test]
    fn all_checks_pass_on_valid_setup() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "test-skill",
            "name: test-skill\ndescription: A test\nallowed-tools: Read\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"",
        );
        write_map(dir.path(), r#"
version: "1.0.0"
lastModified: "2026-03-17"
domains:
  meta: [test-skill]
skills:
  test-skill:
    description: A test
    domain: meta
    repo: blackmatter-pleme
    concerns: [testing]
    references: []
    antiOverlap: []
"#);

        let report = check_all(dir.path()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.skills_checked, 1);
    }

    #[test]
    fn detects_missing_map_entry() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "orphan-skill",
            "name: orphan-skill\ndescription: No map entry\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"",
        );
        write_map(dir.path(), "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains: {}\nskills: {}");

        let report = check_all(dir.path()).unwrap();
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| matches!(e, LintError::MissingMapEntry(n) if n == "orphan-skill")));
    }

    #[test]
    fn detects_orphan_map_entry() {
        let dir = TempDir::new().unwrap();
        write_map(dir.path(), r#"
version: "1.0.0"
lastModified: "2026-03-17"
domains:
  meta: [ghost]
skills:
  ghost:
    description: No directory
    domain: meta
    repo: blackmatter-pleme
    concerns: []
    references: []
    antiOverlap: []
"#);

        let report = check_all(dir.path()).unwrap();
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| matches!(e, LintError::OrphanMapEntry(n) if n == "ghost")));
    }

    #[test]
    fn detects_name_mismatch() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "my-skill",
            "name: wrong-name\ndescription: Mismatch\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"",
        );
        write_map(dir.path(), r#"
version: "1.0.0"
lastModified: "2026-03-17"
domains:
  meta: [my-skill]
skills:
  my-skill:
    description: Mismatch
    domain: meta
    repo: blackmatter-pleme
    concerns: []
    references: []
    antiOverlap: []
"#);

        let report = check_all(dir.path()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::NameMismatch { .. })));
    }

    #[test]
    fn detects_broken_reference() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "linker",
            "name: linker\ndescription: Links\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"",
        );
        write_map(dir.path(), r#"
version: "1.0.0"
lastModified: "2026-03-17"
domains:
  meta: [linker]
skills:
  linker:
    description: Links
    domain: meta
    repo: blackmatter-pleme
    concerns: []
    references: [nonexistent]
    antiOverlap: []
"#);

        let report = check_all(dir.path()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::BrokenReference { target, .. } if target == "nonexistent")));
    }

    #[test]
    fn detects_missing_version() {
        let dir = TempDir::new().unwrap();
        write_map(dir.path(), "domains: {}\nskills: {}");

        let report = check_all(dir.path()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::MissingVersion)));
        assert!(report.errors.iter().any(|e| matches!(e, LintError::MissingLastModified)));
    }

    #[test]
    fn detects_orphan_domain() {
        let dir = TempDir::new().unwrap();
        write_skill(
            dir.path(),
            "lonely",
            "name: lonely\ndescription: No domain\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"",
        );
        write_map(dir.path(), r#"
version: "1.0.0"
lastModified: "2026-03-17"
domains: {}
skills:
  lonely:
    description: No domain
    domain: meta
    repo: blackmatter-pleme
    concerns: []
    references: []
    antiOverlap: []
"#);

        let report = check_all(dir.path()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::OrphanDomain(n) if n == "lonely")));
    }
}
