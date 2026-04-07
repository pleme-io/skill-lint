use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::error::{CheckKind, LintError};
use crate::model::{self, SkillEntry, SkillMap};

// ═══════════════════════════════════════════════════════════════════
// SkillSource trait — abstracts I/O for testability
// ═══════════════════════════════════════════════════════════════════

/// Abstraction over skill data sources. Implement this for custom
/// backends (filesystem, in-memory, S3, archives).
pub trait SkillSource {
    /// Load the merged skill map.
    ///
    /// # Errors
    ///
    /// Returns an error if the backing store is unreadable or malformed.
    fn skill_map(&self) -> anyhow::Result<SkillMap>;

    /// List skill directory names present in the source.
    ///
    /// # Errors
    ///
    /// Returns an error if directory listing fails.
    fn skill_dirs(&self) -> anyhow::Result<BTreeSet<String>>;

    /// Read the `SKILL.md` content for a single skill.
    ///
    /// # Errors
    ///
    /// Returns an error if the skill content cannot be read.
    fn skill_content(&self, name: &str) -> anyhow::Result<String>;
}

// ═══════════════════════════════════════════════════════════════════
// Checker trait — composable, individually testable checks
// ═══════════════════════════════════════════════════════════════════

/// A single composable check. Implement to add custom validation.
pub trait Checker {
    /// The check category this checker belongs to.
    fn kind(&self) -> CheckKind;
    /// Run validation against the shared context, appending any errors found.
    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>);
}

/// Shared context built once, passed to all checkers.
#[must_use]
pub struct CheckContext {
    /// The deserialized skill map.
    pub map: SkillMap,
    /// Skill directory names found on disk.
    pub dir_names: BTreeSet<String>,
    /// Skill names present in the map.
    pub map_names: BTreeSet<String>,
    /// `SKILL.md` contents keyed by skill name.
    pub contents: BTreeMap<String, String>,
}

impl CheckContext {
    /// Build context from a skill source.
    ///
    /// # Errors
    ///
    /// Returns an error if the source can't be read.
    pub fn from_source(source: &dyn SkillSource) -> anyhow::Result<Self> {
        let map = source.skill_map()?;
        let dir_names = source.skill_dirs()?;
        let map_names: BTreeSet<String> = map.skills.keys().cloned().collect();
        let mut contents = BTreeMap::new();
        for name in &dir_names {
            if let Ok(c) = source.skill_content(name) {
                contents.insert(name.clone(), c);
            }
        }
        Ok(Self { map, dir_names, map_names, contents })
    }

    /// Build a map of skill name → `last_verified` date from parsed frontmatter.
    ///
    /// Skips skills whose content is missing or whose frontmatter cannot be parsed.
    #[must_use]
    pub fn last_verified_dates(&self) -> BTreeMap<String, String> {
        self.dir_names
            .iter()
            .filter_map(|name| {
                let content = self.contents.get(name)?;
                let fm = model::parse_frontmatter(content).ok()?;
                let date = fm.metadata?.last_verified?;
                Some((name.clone(), date))
            })
            .collect()
    }
}

// ═══════════════════════════════════════════════════════════════════
// CheckConfig — enable/disable individual checks
// ═══════════════════════════════════════════════════════════════════

/// Configuration for which checks to run.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct CheckConfig {
    /// Enable version/`lastModified` presence check.
    pub version: bool,
    /// Enable sync check (dir ↔ map entry parity).
    pub sync: bool,
    /// Enable frontmatter validation.
    pub frontmatter: bool,
    /// Enable map integrity (references, domains).
    pub map_integrity: bool,
    /// Enable duplicate-concern detection.
    pub duplicate_concerns: bool,
    /// Staleness threshold in days. `None` disables the check.
    pub max_age_days: Option<u32>,
    /// Override "today" for deterministic staleness testing (YYYY-MM-DD).
    pub today: Option<String>,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            version: true,
            sync: true,
            frontmatter: true,
            map_integrity: true,
            duplicate_concerns: true,
            max_age_days: None,
            today: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Built-in checkers
// ═══════════════════════════════════════════════════════════════════

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
        // Every local skill dir must have a map entry
        for name in &ctx.dir_names {
            if !ctx.map_names.contains(name) {
                errors.push(LintError::MissingMapEntry { kind: CheckKind::Sync, name: name.clone() });
            }
        }
        // Every map entry must have a local dir OR be from a different repo.
        // Collect repos that have at least one local directory — these are "local repos."
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
        // References must point to existing skills
        for (name, entry) in &ctx.map.skills {
            for r in &entry.references {
                if !ctx.map_names.contains(r) {
                    errors.push(LintError::BrokenReference {
                        kind: CheckKind::MapIntegrity, skill: name.clone(), target: r.clone(),
                    });
                }
            }
        }

        // Domain index consistency
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

        // Every skill must appear in domains index
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

        // Duplicate concerns (case-insensitive)
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
    fn days_since(today: &str, date: &str) -> Option<i64> {
        // Simple YYYY-MM-DD date diff (no chrono dependency)
        let parse = |s: &str| -> Option<(i64, i64, i64)> {
            let parts: Vec<&str> = s.split('-').collect();
            if parts.len() != 3 { return None; }
            Some((parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?))
        };
        let (ty, tm, td) = parse(today)?;
        let (dy, dm, dd) = parse(date)?;
        // Approximate: good enough for staleness thresholds
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

// ═══════════════════════════════════════════════════════════════════
// Report
// ═══════════════════════════════════════════════════════════════════

/// Aggregated lint results from a check run.
#[must_use]
pub struct Report {
    /// All errors discovered during the run.
    pub errors: Vec<LintError>,
    /// Number of skill directories examined.
    pub skills_checked: usize,
}

impl Report {
    /// Create an empty report for the given number of checked skills.
    #[must_use]
    pub fn new(skills_checked: usize) -> Self {
        Self { errors: Vec::new(), skills_checked }
    }

    /// Returns `true` when no errors were found.
    #[must_use]
    pub fn is_ok(&self) -> bool { self.errors.is_empty() }

    /// Filter errors by [`CheckKind`].
    #[must_use]
    pub fn errors_of(&self, kind: CheckKind) -> Vec<&LintError> {
        self.errors.iter().filter(|e| e.kind() == kind).collect()
    }
}

// ═══════════════════════════════════════════════════════════════════
// Orchestrator
// ═══════════════════════════════════════════════════════════════════

/// Run configured checks against a skill source.
///
/// # Errors
///
/// Returns an error if the source can't be read.
pub fn check_all(source: &dyn SkillSource, config: &CheckConfig) -> anyhow::Result<Report> {
    let ctx = CheckContext::from_source(source)?;
    let mut report = Report::new(ctx.dir_names.len());

    let mut checkers: Vec<Box<dyn Checker>> = vec![
        Box::new(VersionChecker),
        Box::new(SyncChecker),
        Box::new(FrontmatterChecker),
        Box::new(MapIntegrityChecker),
    ];

    if let Some(max_days) = config.max_age_days {
        let today = config.today.clone().unwrap_or_else(|| {
            // Default to current date
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let days = now / 86400;
            let y = 1970 + days / 365;
            let rem = days % 365;
            let m = rem / 30 + 1;
            let d = rem % 30 + 1;
            format!("{y}-{m:02}-{d:02}")
        });
        checkers.push(Box::new(StalenessChecker { max_days, today }));
    }

    // Always check references freshness — pure data, no config needed
    checkers.push(Box::new(ReferencesFreshnessChecker));

    for checker in &checkers {
        let enabled = match checker.kind() {
            CheckKind::Version => config.version,
            CheckKind::Sync => config.sync,
            CheckKind::Frontmatter => config.frontmatter,
            CheckKind::MapIntegrity => config.map_integrity || config.duplicate_concerns,
            CheckKind::Staleness | CheckKind::References => true,
        };
        if enabled {
            checker.check(&ctx, &mut report.errors);
        }
    }

    Ok(report)
}

/// Convenience: all checks, filesystem source.
///
/// # Errors
///
/// Returns an error if the directory or map can't be read.
pub fn check_path(skills_dir: &Path) -> anyhow::Result<Report> {
    check_all(&FsSource { skills_dir, map_dir_override: None }, &CheckConfig::default())
}

// ═══════════════════════════════════════════════════════════════════
// Filesystem source
// ═══════════════════════════════════════════════════════════════════

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

            let mut members = Vec::new();
            for (name, entry) in domain_skills {
                members.push(name.clone());
                skills.insert(name, entry);
            }
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
        // 1. Explicit override
        if let Some(dir) = self.map_dir_override {
            anyhow::ensure!(dir.is_dir(), "map-dir {} does not exist", dir.display());
            return Self::load_split_map(dir);
        }

        // 2. skill-map.d/ inside skills dir
        let map_dir = self.skills_dir.join("skill-map.d");
        if map_dir.is_dir() {
            return Self::load_split_map(&map_dir);
        }

        // 3. skill-map.d/ as sibling of skills dir
        if let Some(parent) = self.skills_dir.parent() {
            let sibling = parent.join("skill-map.d");
            if sibling.is_dir() {
                return Self::load_split_map(&sibling);
            }
        }

        // 4. Legacy: single skill-map.yaml
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

// ═══════════════════════════════════════════════════════════════════
// Testing module — exported for downstream test reuse
// ═══════════════════════════════════════════════════════════════════

pub mod testing {
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
}

// ═══════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use super::testing::*;

    // ─── Happy path ───────────────────────────────────────────────

    #[test]
    fn all_checks_pass() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_skill("beta", "tools", &valid_fm("beta"));
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.skills_checked, 2);
    }

    #[test]
    fn empty_skills_directory() {
        let source = MockSource::new();
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.skills_checked, 0);
    }

    // ─── Version checks ──────────────────────────────────────────

    #[test]
    fn missing_version() {
        let source = MockSource::new().without_version();
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::Version).len(), 2);
    }

    #[test]
    fn version_check_disabled() {
        let source = MockSource::new().without_version();
        let config = CheckConfig { version: false, ..Default::default() };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::Version).len(), 0);
    }

    // ─── Sync checks ─────────────────────────────────────────────

    #[test]
    fn missing_map_entry() {
        let mut source = MockSource::new().with_skill("mapped", "meta", &valid_fm("mapped"));
        source.dirs.insert("orphan".into());
        source.contents.insert("orphan".into(), format!("---\n{}\n---\n", valid_fm("orphan")));
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingMapEntry { name, .. } if name == "orphan")));
    }

    #[test]
    fn orphan_map_entry() {
        // Need a local skill to establish the local repo ("test"),
        // so ghost (same repo, no dir) is flagged as orphan
        let source = MockSource::new()
            .with_skill("local", "meta", &valid_fm("local"))
            .with_skill("ghost", "meta", &valid_fm("ghost"))
            .without_dir("ghost");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::OrphanMapEntry { name, .. } if name == "ghost")));
    }

    #[test]
    fn remote_repo_skill_not_flagged_as_orphan() {
        let mut source = MockSource::new()
            .with_skill("local-skill", "meta", &valid_fm("local-skill"));
        // Add a map entry for a skill from a different repo (no local dir expected)
        source.map.skills.insert("remote-skill".into(), crate::model::SkillEntry {
            description: "A remote skill".into(),
            domain: "meta".into(),
            repo: "other-repo".into(),
            concerns: vec![],
            references: vec![],
        });
        source.map.domains.get_mut("meta").unwrap().push("remote-skill".into());
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        // remote-skill should NOT be flagged as orphan
        assert!(!report.errors.iter().any(|e| matches!(e,
            LintError::OrphanMapEntry { name, .. } if name == "remote-skill")));
    }

    #[test]
    fn sync_check_disabled() {
        let source = MockSource::new()
            .with_skill("ghost", "meta", &valid_fm("ghost"))
            .without_dir("ghost");
        let config = CheckConfig { sync: false, ..Default::default() };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::Sync).len(), 0);
    }

    // ─── Frontmatter checks ──────────────────────────────────────

    #[test]
    fn name_mismatch() {
        let source = MockSource::new()
            .with_skill("my-skill", "meta", "name: wrong-name\ndescription: X\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::NameMismatch { .. })));
    }

    #[test]
    fn missing_description() {
        let source = MockSource::new()
            .with_skill("no-desc", "meta", "name: no-desc\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "description")));
    }

    #[test]
    fn missing_metadata() {
        let source = MockSource::new()
            .with_skill("no-meta", "meta", "name: no-meta\ndescription: X");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "metadata")));
    }

    #[test]
    fn missing_metadata_version() {
        let source = MockSource::new()
            .with_skill("no-ver", "meta", "name: no-ver\ndescription: X\nmetadata:\n  last_verified: \"2026-01-01\"");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "metadata.version")));
    }

    #[test]
    fn unparseable_frontmatter() {
        let source = MockSource::new()
            .with_skill("broken", "meta", &valid_fm("broken"))
            .with_raw_content("broken", "no delimiters here");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field.contains("parse error"))));
    }

    #[test]
    fn multiple_frontmatter_errors_per_skill() {
        let source = MockSource::new()
            .with_skill("bad", "meta", "name: wrong-name");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        let fm_errors = report.errors_of(CheckKind::Frontmatter);
        assert!(fm_errors.len() >= 2, "expected >= 2 frontmatter errors, got {}: {:?}", fm_errors.len(), fm_errors);
    }

    #[test]
    fn frontmatter_check_disabled() {
        let source = MockSource::new()
            .with_skill("bad", "meta", "name: wrong");
        let config = CheckConfig { frontmatter: false, ..Default::default() };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::Frontmatter).len(), 0);
    }

    // ─── Map integrity checks ────────────────────────────────────

    #[test]
    fn broken_reference() {
        let source = MockSource::new()
            .with_skill("linker", "meta", &valid_fm("linker"))
            .with_reference("linker", "nonexistent");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::BrokenReference { target, .. } if target == "nonexistent")));
    }

    #[test]
    fn valid_reference() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_reference("a", "b");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
    }

    #[test]
    fn circular_reference_allowed() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_reference("a", "b")
            .with_reference("b", "a");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
    }

    #[test]
    fn orphan_domain() {
        let source = MockSource::new()
            .with_skill("lonely", "meta", &valid_fm("lonely"))
            .without_domain_entry("lonely");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::OrphanDomain { name, .. } if name == "lonely")));
    }

    #[test]
    fn domain_mismatch() {
        let mut source = MockSource::new()
            .with_skill("misplaced", "rust", &valid_fm("misplaced"));
        source.map.domains.clear();
        source.map.domains.insert("go".into(), vec!["misplaced".into()]);
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::DomainMismatch { skill, found, expected, .. }
            if skill == "misplaced" && found == "rust" && expected == "go")));
    }

    #[test]
    fn ghost_domain_entry() {
        let mut source = MockSource::new()
            .with_skill("real", "meta", &valid_fm("real"));
        source.map.domains.get_mut("meta").unwrap().push("ghost".into());
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::GhostDomainEntry { skill, .. } if skill == "ghost")));
    }

    #[test]
    fn duplicate_concern_detected() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_concern("a", "Cargo.toml")
            .with_concern("b", "Cargo.toml");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::DuplicateConcern { concern, .. } if concern == "Cargo.toml")));
    }

    #[test]
    fn duplicate_concern_case_insensitive() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_concern("a", "Docker")
            .with_concern("b", "docker");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::DuplicateConcern { .. })));
    }

    #[test]
    fn unique_concerns_pass() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "meta", &valid_fm("b"))
            .with_concern("a", "Cargo.toml")
            .with_concern("b", "package.json");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors_of(CheckKind::MapIntegrity).is_empty());
    }

    #[test]
    fn map_integrity_check_disabled() {
        let source = MockSource::new()
            .with_skill("linker", "meta", &valid_fm("linker"))
            .with_reference("linker", "nonexistent");
        let config = CheckConfig { map_integrity: false, duplicate_concerns: false, ..Default::default() };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::MapIntegrity).len(), 0);
    }

    // ─── Report ──────────────────────────────────────────────────

    #[test]
    fn report_filters_by_kind() {
        let source = MockSource::new()
            .with_skill("a", "meta", "name: wrong\ndescription: X\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
            .without_version();
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::Version).len(), 2);
        assert_eq!(report.errors_of(CheckKind::Frontmatter).len(), 1);
        assert_eq!(report.errors_of(CheckKind::Sync).len(), 0);
    }

    // ─── Determinism ─────────────────────────────────────────────

    #[test]
    fn check_ordering_is_deterministic() {
        let source = MockSource::new()
            .with_skill("a", "meta", "name: wrong\ndescription: X")
            .with_skill("b", "meta", &valid_fm("b"))
            .with_reference("b", "nonexistent")
            .without_version();
        let r1 = check_all(&source, &CheckConfig::default()).unwrap();
        let r2 = check_all(&source, &CheckConfig::default()).unwrap();
        let e1: Vec<String> = r1.errors.iter().map(ToString::to_string).collect();
        let e2: Vec<String> = r2.errors.iter().map(ToString::to_string).collect();
        assert_eq!(e1, e2);
    }

    // ─── Filesystem integration ──────────────────────────────────

    #[test]
    fn filesystem_source_works() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), format!("---\n{}\n---\n\n# Body\n", valid_fm("test-skill"))).unwrap();
        fs::write(dir.path().join("skill-map.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains:\n  meta: [test-skill]\nskills:\n  test-skill:\n    description: A test\n    domain: meta\n    repo: test\n"
        ).unwrap();
        let report = check_path(dir.path()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
    }

    // ─── Individual checker tests ────────────────────────────────

    #[test]
    fn version_checker_independently() {
        let source = MockSource::new().without_version();
        let ctx = CheckContext::from_source(&source).unwrap();
        let mut errors = Vec::new();
        VersionChecker.check(&ctx, &mut errors);
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().all(|e| e.kind() == CheckKind::Version));
    }

    #[test]
    fn sync_checker_independently() {
        let mut source = MockSource::new().with_skill("a", "meta", &valid_fm("a"));
        source.dirs.insert("orphan".into());
        let ctx = CheckContext::from_source(&source).unwrap();
        let mut errors = Vec::new();
        SyncChecker.check(&ctx, &mut errors);
        assert!(errors.iter().any(|e| matches!(e, LintError::MissingMapEntry { .. })));
    }

    // ─── Staleness checks ────────────────────────────────────────

    #[test]
    fn stale_skill_detected() {
        let source = MockSource::new()
            .with_skill("old", "meta", "name: old\ndescription: Old\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2025-01-01\"");
        let config = CheckConfig {
            max_age_days: Some(90),
            today: Some("2026-03-17".into()),
            ..Default::default()
        };
        let report = check_all(&source, &config).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e, LintError::Stale { skill, .. } if skill == "old")));
    }

    #[test]
    fn fresh_skill_not_stale() {
        let source = MockSource::new()
            .with_skill("fresh", "meta", "name: fresh\ndescription: Fresh\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-15\"");
        let config = CheckConfig {
            max_age_days: Some(90),
            today: Some("2026-03-17".into()),
            ..Default::default()
        };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::Staleness).len(), 0);
    }

    #[test]
    fn staleness_disabled_by_default() {
        let source = MockSource::new()
            .with_skill("old", "meta", "name: old\ndescription: Old\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2020-01-01\"");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::Staleness).len(), 0);
    }

    #[test]
    fn staleness_checker_deterministic_with_fixed_today() {
        let source = MockSource::new()
            .with_skill("a", "meta", "name: a\ndescription: A\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2025-06-01\"");
        let config = CheckConfig {
            max_age_days: Some(90),
            today: Some("2026-03-17".into()),
            ..Default::default()
        };
        let r1 = check_all(&source, &config).unwrap();
        let r2 = check_all(&source, &config).unwrap();
        assert_eq!(r1.errors.len(), r2.errors.len());
    }

    #[test]
    fn days_since_calculation() {
        assert_eq!(StalenessChecker::days_since("2026-03-17", "2026-03-17"), Some(0));
        assert_eq!(StalenessChecker::days_since("2026-03-17", "2026-03-07"), Some(10));
        assert_eq!(StalenessChecker::days_since("2026-03-17", "2025-03-17"), Some(365));
        assert!(StalenessChecker::days_since("bad", "2026-03-17").is_none());
    }

    // ─── Split map filesystem integration ────────────────────────

    #[test]
    fn split_map_filesystem_works() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();

        // Create skill directory
        let skill_dir = dir.path().join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), format!("---\n{}\n---\n\n# Body\n", valid_fm("test-skill"))).unwrap();

        // Create skill-map.d/ with config + domain file
        let map_dir = dir.path().join("skill-map.d");
        fs::create_dir_all(&map_dir).unwrap();
        fs::write(map_dir.join("config.yaml"), "version: \"2.0.0\"\nlastModified: \"2026-03-17\"\n").unwrap();
        fs::write(map_dir.join("meta.yaml"),
            "test-skill:\n  description: A test\n  domain: meta\n  repo: test\n  concerns: [testing]\n  references: []\n"
        ).unwrap();

        let report = check_path(dir.path()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.skills_checked, 1);
    }

    // ─── References freshness checks ─────────────────────────────

    #[test]
    fn reference_newer_detected() {
        let source = MockSource::new()
            .with_skill("old-skill", "meta",
                "name: old-skill\ndescription: Old\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
            .with_skill("new-skill", "meta",
                "name: new-skill\ndescription: New\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-15\"")
            .with_reference("old-skill", "new-skill");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::ReferenceNewer { skill, reference, .. }
            if skill == "old-skill" && reference == "new-skill"
        )));
    }

    #[test]
    fn reference_older_no_error() {
        let source = MockSource::new()
            .with_skill("new-skill", "meta",
                "name: new-skill\ndescription: New\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-15\"")
            .with_skill("old-skill", "meta",
                "name: old-skill\ndescription: Old\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
            .with_reference("new-skill", "old-skill");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::References).len(), 0);
    }

    #[test]
    fn reference_same_date_no_error() {
        let source = MockSource::new()
            .with_skill("a", "meta",
                "name: a\ndescription: A\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-17\"")
            .with_skill("b", "meta",
                "name: b\ndescription: B\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-17\"")
            .with_reference("a", "b");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::References).len(), 0);
    }

    // ─── Individual checker tests (additional) ──────────────────

    #[test]
    fn frontmatter_checker_independently() {
        let source = MockSource::new()
            .with_skill("bad", "meta", "name: wrong");
        let ctx = CheckContext::from_source(&source).unwrap();
        let mut errors = Vec::new();
        FrontmatterChecker.check(&ctx, &mut errors);
        assert!(errors.iter().any(|e| matches!(e, LintError::NameMismatch { .. })));
        assert!(errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "metadata")));
    }

    #[test]
    fn map_integrity_checker_independently() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_reference("a", "nonexistent");
        let ctx = CheckContext::from_source(&source).unwrap();
        let mut errors = Vec::new();
        MapIntegrityChecker.check(&ctx, &mut errors);
        assert!(errors.iter().any(|e| matches!(e, LintError::BrokenReference { .. })));
    }

    #[test]
    fn staleness_checker_independently() {
        let source = MockSource::new()
            .with_skill("old", "meta",
                "name: old\ndescription: Old\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2025-01-01\"");
        let ctx = CheckContext::from_source(&source).unwrap();
        let mut errors = Vec::new();
        let checker = StalenessChecker { max_days: 90, today: "2026-03-17".into() };
        checker.check(&ctx, &mut errors);
        assert!(errors.iter().any(|e| matches!(e, LintError::Stale { skill, .. } if skill == "old")));
    }

    #[test]
    fn references_freshness_checker_independently() {
        let source = MockSource::new()
            .with_skill("old-skill", "meta",
                "name: old-skill\ndescription: Old\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
            .with_skill("new-skill", "meta",
                "name: new-skill\ndescription: New\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-15\"")
            .with_reference("old-skill", "new-skill");
        let ctx = CheckContext::from_source(&source).unwrap();
        let mut errors = Vec::new();
        ReferencesFreshnessChecker.check(&ctx, &mut errors);
        assert!(errors.iter().any(|e| matches!(e,
            LintError::ReferenceNewer { skill, reference, .. }
            if skill == "old-skill" && reference == "new-skill"
        )));
    }

    // ─── CheckConfig defaults ─────────────────────────────────────

    #[test]
    fn check_config_defaults() {
        let cfg = CheckConfig::default();
        assert!(cfg.version);
        assert!(cfg.sync);
        assert!(cfg.frontmatter);
        assert!(cfg.map_integrity);
        assert!(cfg.duplicate_concerns);
        assert!(cfg.max_age_days.is_none());
        assert!(cfg.today.is_none());
    }

    // ─── Report ──────────────────────────────────────────────────

    #[test]
    fn report_new_is_ok() {
        let report = Report::new(5);
        assert!(report.is_ok());
        assert_eq!(report.skills_checked, 5);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn report_not_ok_with_errors() {
        let mut report = Report::new(1);
        report.errors.push(LintError::MissingVersion { kind: CheckKind::Version });
        assert!(!report.is_ok());
    }

    // ─── CheckContext construction ────────────────────────────────

    #[test]
    fn check_context_populates_map_names() {
        let source = MockSource::new()
            .with_skill("a", "meta", &valid_fm("a"))
            .with_skill("b", "tools", &valid_fm("b"));
        let ctx = CheckContext::from_source(&source).unwrap();
        assert!(ctx.map_names.contains("a"));
        assert!(ctx.map_names.contains("b"));
        assert_eq!(ctx.map_names.len(), 2);
    }

    #[test]
    fn check_context_loads_contents() {
        let source = MockSource::new()
            .with_skill("s", "meta", &valid_fm("s"));
        let ctx = CheckContext::from_source(&source).unwrap();
        assert!(ctx.contents.get("s").unwrap().contains("name: s"));
    }

    // ─── CheckContext helpers ──────────────────────────────────────

    #[test]
    fn last_verified_dates_extracts_dates() {
        let source = MockSource::new()
            .with_skill("a", "meta",
                "name: a\ndescription: A\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
            .with_skill("b", "meta",
                "name: b\ndescription: B\nmetadata:\n  version: \"1.0.0\"");
        let ctx = CheckContext::from_source(&source).unwrap();
        let dates = ctx.last_verified_dates();
        assert_eq!(dates.get("a").map(String::as_str), Some("2026-01-01"));
        assert!(dates.get("b").is_none());
    }

    // ─── Staleness edge cases ─────────────────────────────────────

    #[test]
    fn days_since_different_months() {
        assert_eq!(StalenessChecker::days_since("2026-06-15", "2026-03-15"), Some(90));
    }

    #[test]
    fn days_since_end_of_year() {
        assert_eq!(StalenessChecker::days_since("2027-01-01", "2026-12-01"), Some(365 + (-11) * 30));
    }

    #[test]
    fn days_since_invalid_format() {
        assert!(StalenessChecker::days_since("2026-03", "2026-03-17").is_none());
        assert!(StalenessChecker::days_since("not-a-date", "also-bad").is_none());
        assert!(StalenessChecker::days_since("", "").is_none());
    }

    #[test]
    fn staleness_exactly_at_threshold() {
        let source = MockSource::new()
            .with_skill("edge", "meta",
                "name: edge\ndescription: E\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"");
        let config = CheckConfig {
            max_age_days: Some(75),
            today: Some("2026-03-17".into()),
            ..Default::default()
        };
        let report = check_all(&source, &config).unwrap();
        let stale_errors = report.errors_of(CheckKind::Staleness);
        assert_eq!(stale_errors.len(), 1);
    }

    #[test]
    fn staleness_skips_unparseable_frontmatter() {
        let source = MockSource::new()
            .with_skill("broken", "meta", &valid_fm("broken"))
            .with_raw_content("broken", "no delimiters");
        let config = CheckConfig {
            max_age_days: Some(1),
            today: Some("2026-03-17".into()),
            ..Default::default()
        };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::Staleness).len(), 0);
    }

    #[test]
    fn staleness_skips_missing_last_verified() {
        let source = MockSource::new()
            .with_skill("no-date", "meta",
                "name: no-date\ndescription: X\nmetadata:\n  version: \"1.0.0\"");
        let config = CheckConfig {
            max_age_days: Some(1),
            today: Some("2026-03-17".into()),
            ..Default::default()
        };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::Staleness).len(), 0);
    }

    // ─── Frontmatter missing last_verified ────────────────────────

    #[test]
    fn missing_metadata_last_verified() {
        let source = MockSource::new()
            .with_skill("no-lv", "meta",
                "name: no-lv\ndescription: X\nmetadata:\n  version: \"1.0.0\"");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.errors.iter().any(|e| matches!(e,
            LintError::MissingFrontmatter { field, .. } if field == "metadata.last_verified")));
    }

    // ─── References edge cases ────────────────────────────────────

    #[test]
    fn reference_to_skill_without_date_is_ignored() {
        let source = MockSource::new()
            .with_skill("dated", "meta",
                "name: dated\ndescription: D\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"")
            .with_skill("undated", "meta",
                "name: undated\ndescription: U\nmetadata:\n  version: \"1.0.0\"")
            .with_reference("dated", "undated");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::References).len(), 0);
    }

    #[test]
    fn reference_from_skill_without_date_is_ignored() {
        let source = MockSource::new()
            .with_skill("undated", "meta",
                "name: undated\ndescription: U\nmetadata:\n  version: \"1.0.0\"")
            .with_skill("dated", "meta",
                "name: dated\ndescription: D\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-15\"")
            .with_reference("undated", "dated");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::References).len(), 0);
    }

    // ─── Filesystem edge cases ────────────────────────────────────

    #[test]
    fn filesystem_dir_without_skill_md_ignored() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let not_a_skill = dir.path().join("not-a-skill");
        fs::create_dir_all(&not_a_skill).unwrap();
        fs::write(not_a_skill.join("README.md"), "not a skill").unwrap();
        fs::write(dir.path().join("skill-map.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains: {}\nskills: {}\n"
        ).unwrap();
        let report = check_path(dir.path()).unwrap();
        assert!(report.is_ok());
        assert_eq!(report.skills_checked, 0);
    }

    #[test]
    fn filesystem_sibling_map_dir() {
        use tempfile::TempDir;
        let root = TempDir::new().unwrap();
        let skills_dir = root.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        let skill_dir = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"),
            format!("---\n{}\n---\n\n# Body\n", valid_fm("test-skill"))).unwrap();

        let map_dir = root.path().join("skill-map.d");
        fs::create_dir_all(&map_dir).unwrap();
        fs::write(map_dir.join("config.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\n").unwrap();
        fs::write(map_dir.join("meta.yaml"),
            "test-skill:\n  description: A test\n  domain: meta\n  repo: test\n").unwrap();

        let report = check_path(&skills_dir).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.skills_checked, 1);
    }

    #[test]
    fn filesystem_map_dir_override() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let map_dir = dir.path().join("custom-map");
        fs::create_dir_all(&map_dir).unwrap();
        fs::write(map_dir.join("config.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\n").unwrap();

        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let skill_dir = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"),
            format!("---\n{}\n---\n\n# Body\n", valid_fm("test-skill"))).unwrap();

        fs::write(map_dir.join("meta.yaml"),
            "test-skill:\n  description: A test\n  domain: meta\n  repo: test\n").unwrap();

        let source = FsSource { skills_dir: &skills_dir, map_dir_override: Some(&map_dir) };
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
    }

    #[test]
    fn filesystem_missing_map_returns_error() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let result = check_path(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn filesystem_map_dir_override_nonexistent_fails() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let bad_map = dir.path().join("nonexistent");
        let source = FsSource { skills_dir: &skills_dir, map_dir_override: Some(&bad_map) };
        assert!(check_all(&source, &CheckConfig::default()).is_err());
    }

    // ─── References freshness cascades ────────────────────────────

    #[test]
    fn reference_freshness_cascades() {
        // c references b, b references a. a is newest.
        // b should be flagged (a is newer), c should be flagged (b is older but doesn't matter —
        // c only checks its direct references)
        let source = MockSource::new()
            .with_skill("a", "meta",
                "name: a\ndescription: A\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-17\"")
            .with_skill("b", "meta",
                "name: b\ndescription: B\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-10\"")
            .with_skill("c", "meta",
                "name: c\ndescription: C\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-05\"")
            .with_reference("b", "a")
            .with_reference("c", "b");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        let watch_errors = report.errors_of(CheckKind::References);
        // b is flagged because a (2026-03-17) > b (2026-03-10)
        assert!(watch_errors.iter().any(|e| matches!(e,
            LintError::ReferenceNewer { skill, reference, .. }
            if skill == "b" && reference == "a"
        )));
        // c is NOT flagged because b (2026-03-10) < c... wait, c is 2026-03-05 which is older than b 2026-03-10
        // so c IS flagged because b (2026-03-10) > c (2026-03-05)
        assert!(watch_errors.iter().any(|e| matches!(e,
            LintError::ReferenceNewer { skill, reference, .. }
            if skill == "c" && reference == "b"
        )));
    }
}
