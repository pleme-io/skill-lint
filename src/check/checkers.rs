use std::collections::BTreeMap;

use crate::error::{CheckKind, LintError};
use crate::model;

use super::{CheckContext, Checker};

/// Validates that the skill map contains `version` and `lastModified` fields.
pub struct VersionChecker;
impl Checker for VersionChecker {
    fn kind(&self) -> CheckKind { CheckKind::Version }
    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        if ctx.map.version.is_none() {
            errors.push(LintError::MissingVersion { kind: CheckKind::Version });
        }
        if ctx.map.last_modified.is_none() {
            errors.push(LintError::MissingLastModified { kind: CheckKind::Version });
        }
    }
}

/// Sync checker with optional local repo filter.
/// Skills from other repos (different `repo` field) are excluded from
/// the orphan check — they legitimately won't have a local directory.
pub struct SyncChecker;
impl Checker for SyncChecker {
    fn kind(&self) -> CheckKind { CheckKind::Sync }
    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        use std::collections::BTreeSet;

        for name in &ctx.dir_names {
            if !ctx.map_names.contains(name) {
                errors.push(LintError::MissingMapEntry { kind: CheckKind::Sync, name: name.clone() });
            }
        }
        let local_repos: BTreeSet<&str> = ctx.dir_names.iter()
            .filter_map(|d| ctx.map.skills.get(d).map(|e| e.repo.as_str()))
            .collect();
        for name in &ctx.map_names {
            if ctx.dir_names.contains(name) {
                continue;
            }
            if let Some(entry) = ctx.map.skills.get(name)
                && !local_repos.contains(entry.repo.as_str())
            {
                continue;
            }
            errors.push(LintError::OrphanMapEntry { kind: CheckKind::Sync, name: name.clone() });
        }
    }
}

/// Validates `SKILL.md` frontmatter: required fields, name/dir consistency.
pub struct FrontmatterChecker;
impl Checker for FrontmatterChecker {
    fn kind(&self) -> CheckKind { CheckKind::Frontmatter }
    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        for name in &ctx.dir_names {
            let Some(content) = ctx.contents.get(name) else {
                continue;
            };

            let Ok(fm) = model::parse_frontmatter(content) else {
                errors.push(LintError::MissingFrontmatter {
                    kind: CheckKind::Frontmatter,
                    skill: name.clone(),
                    field: "frontmatter (parse error)".into(),
                });
                continue;
            };

            match &fm.name {
                Some(n) if n != name => {
                    errors.push(LintError::NameMismatch {
                        kind: CheckKind::Frontmatter, skill: name.clone(),
                        found: n.clone(), expected: name.clone(),
                    });
                }
                None => {
                    errors.push(LintError::MissingFrontmatter {
                        kind: CheckKind::Frontmatter, skill: name.clone(), field: "name".into(),
                    });
                }
                _ => {}
            }

            if fm.description.is_none() {
                errors.push(LintError::MissingFrontmatter {
                    kind: CheckKind::Frontmatter, skill: name.clone(), field: "description".into(),
                });
            }

            match &fm.metadata {
                Some(m) => {
                    if m.version.is_none() {
                        errors.push(LintError::MissingFrontmatter {
                            kind: CheckKind::Frontmatter, skill: name.clone(),
                            field: "metadata.version".into(),
                        });
                    }
                    if m.last_verified.is_none() {
                        errors.push(LintError::MissingFrontmatter {
                            kind: CheckKind::Frontmatter, skill: name.clone(),
                            field: "metadata.last_verified".into(),
                        });
                    }
                }
                None => {
                    errors.push(LintError::MissingFrontmatter {
                        kind: CheckKind::Frontmatter, skill: name.clone(), field: "metadata".into(),
                    });
                }
            }
        }
    }
}

/// Validates map structural integrity: references, domain index, duplicate concerns.
pub struct MapIntegrityChecker;
impl Checker for MapIntegrityChecker {
    fn kind(&self) -> CheckKind { CheckKind::MapIntegrity }
    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        errors.extend(
            ctx.map.skills.iter()
                .flat_map(|(name, entry)| {
                    entry.references.iter()
                        .filter(|r| !ctx.map_names.contains(*r))
                        .map(move |r| LintError::BrokenReference {
                            kind: CheckKind::MapIntegrity,
                            skill: name.clone(),
                            target: r.clone(),
                        })
                })
        );

        let mut domain_of_skill: BTreeMap<String, String> = BTreeMap::new();
        for (domain_name, members) in &ctx.map.domains {
            for member in members {
                if !ctx.map_names.contains(member) {
                    errors.push(LintError::GhostDomainEntry {
                        kind: CheckKind::MapIntegrity, domain: domain_name.clone(),
                        skill: member.clone(),
                    });
                }
                domain_of_skill.insert(member.clone(), domain_name.clone());
            }
        }

        for name in &ctx.map_names {
            if !domain_of_skill.contains_key(name) {
                errors.push(LintError::OrphanDomain { kind: CheckKind::MapIntegrity, name: name.clone() });
            }
        }

        for (name, entry) in &ctx.map.skills {
            if let Some(listed_domain) = domain_of_skill.get(name)
                && *listed_domain != entry.domain
            {
                errors.push(LintError::DomainMismatch {
                    kind: CheckKind::MapIntegrity,
                    skill: name.clone(),
                    found: entry.domain.clone(),
                    expected: listed_domain.clone(),
                });
            }
        }

        let mut concern_owners: BTreeMap<String, String> = BTreeMap::new();
        for (name, entry) in &ctx.map.skills {
            for concern in &entry.concerns {
                let normalized = concern.to_lowercase();
                if let Some(existing) = concern_owners.get(&normalized) {
                    errors.push(LintError::DuplicateConcern {
                        kind: CheckKind::MapIntegrity, concern: concern.clone(),
                        skill_a: existing.clone(), skill_b: name.clone(),
                    });
                } else {
                    concern_owners.insert(normalized, name.clone());
                }
            }
        }
    }
}

/// Flags skills whose `last_verified` date exceeds a configurable threshold.
pub struct StalenessChecker {
    /// Maximum allowed age in days.
    pub max_days: u32,
    /// Reference date (`YYYY-MM-DD`) used as "today".
    pub today: String,
}

impl StalenessChecker {
    pub(crate) fn days_since(today: &str, date: &str) -> Option<i64> {
        let parse = |s: &str| -> Option<(i64, i64, i64)> {
            let parts: Vec<&str> = s.split('-').collect();
            if parts.len() != 3 { return None; }
            Some((parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?))
        };
        let (ty, tm, td) = parse(today)?;
        let (dy, dm, dd) = parse(date)?;
        Some((ty - dy) * 365 + (tm - dm) * 30 + (td - dd))
    }
}

impl Checker for StalenessChecker {
    fn kind(&self) -> CheckKind { CheckKind::Staleness }
    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        let dates = ctx.last_verified_dates();
        for (name, date) in &dates {
            if let Some(age) = Self::days_since(&self.today, date)
                && age > i64::from(self.max_days)
            {
                errors.push(LintError::Stale {
                    kind: CheckKind::Staleness,
                    skill: name.clone(),
                    last_verified: date.clone(),
                    max_days: self.max_days,
                });
            }
        }
    }
}

/// Checks if any referenced skill was verified more recently than the
/// referencing skill. If so, the referencing skill may need review.
/// Pure data check — no filesystem access, no workspace root needed.
pub struct ReferencesFreshnessChecker;

impl Checker for ReferencesFreshnessChecker {
    fn kind(&self) -> CheckKind { CheckKind::References }
    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        let dates = ctx.last_verified_dates();

        for (name, entry) in &ctx.map.skills {
            let Some(skill_date) = dates.get(name) else { continue };
            for reference in &entry.references {
                let Some(ref_date) = dates.get(reference) else { continue };
                if ref_date > skill_date {
                    errors.push(LintError::ReferenceNewer {
                        kind: CheckKind::References,
                        skill: name.clone(),
                        skill_date: skill_date.clone(),
                        reference: reference.clone(),
                        ref_date: ref_date.clone(),
                    });
                }
            }
        }
    }
}
