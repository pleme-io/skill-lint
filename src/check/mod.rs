mod checkers;
mod fs_source;
pub mod links;
pub mod testing;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::error::{CheckKind, LintError};
use crate::model::{self, SkillMap};

pub use checkers::{
    DiscoveryChecker, FrontmatterChecker, ListingBudgetChecker, MapIntegrityChecker,
    ReferencesFreshnessChecker, StalenessChecker, SyncChecker, VersionChecker,
};
pub use fs_source::FsSource;
pub use links::PathResolutionChecker;

/// Lexically resolve `.` and `..` without touching the filesystem.
///
/// Deterministic by construction: two equal path expressions normalize to the
/// same value whether or not anything exists, which is what lets the in-memory
/// and on-disk oracles agree.
pub(crate) fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

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

    /// Human-readable description of where skills were looked for.
    ///
    /// Reported when discovery finds nothing, so the operator sees which root
    /// was actually searched rather than a bare "0 skills".
    fn origin(&self) -> String { "<in-memory source>".to_owned() }

    /// Resolver for paths mentioned in skill bodies, if this source can resolve
    /// them at all.
    ///
    /// `None` is the honest default: a source with no notion of a filesystem
    /// cannot say whether `theory/THEORY.md` exists, and the path-resolution
    /// check reports nothing rather than reporting everything as broken.
    /// Unknown is not absent.
    fn path_oracle(&self) -> Option<Box<dyn PathOracle>> { None }
}

// ═══════════════════════════════════════════════════════════════════
// PathOracle trait — resolves what a skill body points at
// ═══════════════════════════════════════════════════════════════════

/// Answers existence questions about paths a skill body names.
///
/// Deliberately OWNS its data rather than borrowing the source, so
/// [`CheckContext`] stays free of lifetime parameters and every checker keeps
/// its `&CheckContext`-only signature.
pub trait PathOracle {
    /// Is `rel` present, resolved from `skill`'s own directory?
    fn exists_near_skill(&self, skill: &str, rel: &str) -> bool;

    /// Is `repo` a repository present on this machine?
    ///
    /// This is the gate that keeps the check trustworthy: a `<repo>/<path>`
    /// pointer into a repository nobody has cloned is unknowable, and guessing
    /// "broken" would fire on a reader's checkout rather than on a real defect.
    fn has_repo(&self, repo: &str) -> bool;

    /// Is `rel` — a `<repo>/<path>` string — present under the root that holds
    /// sibling repositories?
    fn exists_under_repo_root(&self, rel: &str) -> bool;
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
    /// Where the source looked for skills — reported when discovery is empty.
    pub origin: String,
    /// Resolver for paths named in skill bodies. `None` when the source cannot
    /// resolve paths, in which case path resolution reports nothing.
    pub oracle: Option<Box<dyn PathOracle>>,
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
        let contents: BTreeMap<String, String> = dir_names
            .iter()
            .filter_map(|name| {
                source.skill_content(name).ok().map(|c| (name.clone(), c))
            })
            .collect();
        Ok(Self {
            map,
            dir_names,
            map_names,
            contents,
            origin: source.origin(),
            oracle: source.path_oracle(),
        })
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
    /// Enable link/path resolution against the backing store.
    ///
    /// Default-ON: a dead pointer has an objective right answer a human can fix
    /// without manufacturing a claim, so it is safe to gate CI on. The flag
    /// exists for the case where the answer is knowably unavailable — a corpus
    /// linted somewhere its sibling repositories are not — not as a way to live
    /// with dead pointers, which is what the per-path `pending-path:` waiver is
    /// for.
    pub path_resolution: bool,
    /// Staleness threshold in days. `None` disables the check.
    pub max_age_days: Option<u32>,
    /// Override "today" for deterministic staleness testing (YYYY-MM-DD).
    pub today: Option<String>,
    /// Hard per-entry skill-listing character cap.
    ///
    /// Claude Code truncates each listing entry at this many characters and
    /// silently discards the rest, so a description over the cap loses whatever
    /// trigger vocabulary sits past it. Configurable because the platform
    /// exposes it as a setting (`skillListingMaxDescChars`); the default
    /// mirrors the documented platform default.
    pub max_desc_chars: usize,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            version: true,
            sync: true,
            frontmatter: true,
            map_integrity: true,
            duplicate_concerns: true,
            path_resolution: true,
            max_age_days: None,
            today: None,
            // Platform default for `skillListingMaxDescChars`.
            max_desc_chars: 1536,
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
        Box::new(DiscoveryChecker),
        Box::new(VersionChecker),
        Box::new(SyncChecker),
        Box::new(FrontmatterChecker),
        Box::new(MapIntegrityChecker),
        Box::new(ListingBudgetChecker { cap: config.max_desc_chars }),
        // Structural, like the four above it: "the file is not there" is an
        // objective fact, so this rides with the always-on suite rather than
        // with the freshness checks gated below.
        Box::new(PathResolutionChecker),
    ];

    if let Some(max_days) = config.max_age_days {
        let today = config.today.clone().unwrap_or_else(|| {
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

        // References-FRESHNESS belongs to the same family as staleness, not to
        // the structural checks: it reads `last_verified` and asks "was the
        // target re-verified more recently than the referrer?" — a question
        // only meaningful once the caller has opted into caring about
        // freshness. Gating it here (rather than always-on) lets a consumer
        // run the STRUCTURAL suite alone — sync, frontmatter, map-integrity,
        // version — all of which have objective right answers a machine can
        // fix, and gate CI on those.
        //
        // Why that split matters: freshness cannot be honestly repaired by a
        // machine. Clearing a staleness or reference-newer finding means a
        // human actually re-read the skill; a bot bumping the date to go green
        // manufactures a false claim, which is worse than the stale one it
        // replaced. A CI gate that forces that bump is a false-claim factory.
        // So freshness is opt-in and advisory; structure is always-on and
        // gateable.
        checkers.push(Box::new(ReferencesFreshnessChecker));
    }

    for checker in &checkers {
        let enabled = match checker.kind() {
            CheckKind::Version => config.version,
            CheckKind::Sync => config.sync,
            CheckKind::Frontmatter => config.frontmatter,
            CheckKind::MapIntegrity => config.map_integrity || config.duplicate_concerns,
            CheckKind::PathResolution => config.path_resolution,
            _ => true,
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
pub fn check_path(skills_dir: impl AsRef<Path>) -> anyhow::Result<Report> {
    let skills_dir = skills_dir.as_ref();
    check_all(&FsSource { skills_dir, map_dir_override: None }, &CheckConfig::default())
}

// ═══════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use super::testing::*;
    use crate::error::PathForm;

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

    // ─── Discovery ────────────────────────────────────────────────

    #[test]
    fn empty_skills_directory_is_an_error() {
        // A run that lints zero skills is a vacuous pass — it reports success
        // having validated nothing. That is worse than no gate at all, so it
        // must fail.
        let source = MockSource::new();
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert!(!report.is_ok(), "zero-skill run must not pass");
        assert_eq!(report.skills_checked, 0);
        assert!(report.errors.iter().any(|e| matches!(e, LintError::NoSkillsFound { .. })));
    }

    #[test]
    fn discovery_checker_independently() {
        let empty = CheckContext::from_source(&MockSource::new()).unwrap();
        let mut errors = Vec::new();
        DiscoveryChecker.check(&empty, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind(), CheckKind::Discovery);

        let populated = CheckContext::from_source(
            &MockSource::new().with_skill("a", "meta", &valid_fm("a")),
        )
        .unwrap();
        let mut errors = Vec::new();
        DiscoveryChecker.check(&populated, &mut errors);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn discovery_error_names_the_searched_root() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("skill-map.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains: {}\nskills: {}\n"
        ).unwrap();
        let report = check_path(dir.path()).unwrap();
        let msg = report.errors.iter()
            .find(|e| matches!(e, LintError::NoSkillsFound { .. }))
            .expect("expected NoSkillsFound")
            .to_string();
        assert!(msg.contains(dir.path().to_str().unwrap()), "path missing from: {msg}");
        assert!(msg.contains("SKILL.md"), "expectation missing from: {msg}");
    }

    #[test]
    fn nested_skills_dir_is_discovered_from_repo_root() {
        // The bare invocation from a repo root: skills live in ./skills, the
        // map at ./skill-map.d. Must find them, not silently find nothing.
        use tempfile::TempDir;
        let root = TempDir::new().unwrap();
        let skills = root.path().join("skills");
        let skill_dir = skills.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"),
            format!("---\n{}\n---\n\n# Body\n", valid_fm("test-skill"))).unwrap();

        let map_dir = root.path().join("skill-map.d");
        fs::create_dir_all(&map_dir).unwrap();
        fs::write(map_dir.join("config.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\n").unwrap();
        fs::write(map_dir.join("meta.yaml"),
            "test-skill:\n  description: A test\n  domain: meta\n  repo: test\n").unwrap();

        // Bare form (root) and explicit form (root/skills) must agree.
        let from_root = check_path(root.path()).unwrap();
        assert!(from_root.is_ok(), "errors: {:?}", from_root.errors);
        assert_eq!(from_root.skills_checked, 1);

        let from_skills = check_path(&skills).unwrap();
        assert!(from_skills.is_ok(), "errors: {:?}", from_skills.errors);
        assert_eq!(from_skills.skills_checked, 1);
    }

    #[test]
    fn explicit_skills_dir_is_never_redirected() {
        // A dir that already holds skills is used as-is, even when it also
        // contains a `skills/` subdirectory — the fallback is a last resort,
        // never an override of what the caller asked for.
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("outer");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("SKILL.md"),
            format!("---\n{}\n---\n", valid_fm("outer"))).unwrap();
        let decoy = dir.path().join("skills").join("inner");
        fs::create_dir_all(&decoy).unwrap();
        fs::write(decoy.join("SKILL.md"),
            format!("---\n{}\n---\n", valid_fm("inner"))).unwrap();
        fs::write(dir.path().join("skill-map.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains:\n  meta: [outer]\nskills:\n  outer:\n    description: A test\n    domain: meta\n    repo: test\n"
        ).unwrap();

        let report = check_path(dir.path()).unwrap();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.skills_checked, 1);
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
        source.map.skills.insert("remote-skill".into(), crate::model::SkillEntry {
            description: "A remote skill".into(),
            domain: "meta".into(),
            repo: "other-repo".into(),
            concerns: vec![],
            references: vec![],
        });
        source.map.domains.get_mut("meta").unwrap().push("remote-skill".into());
        let report = check_all(&source, &CheckConfig::default()).unwrap();
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
    fn description_within_listing_cap_passes() {
        let fm = format!("name: fits\ndescription: {}", "x".repeat(1000));
        let source = MockSource::new().with_skill("fits", "meta", &fm);
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::ListingBudget).len(), 0);
    }

    /// The gate must be observed RED against a deliberately-broken input.
    ///
    /// A check that has never failed may be checking nothing; this pins the
    /// failing direction so a future refactor cannot silently neuter it.
    #[test]
    fn description_over_listing_cap_is_caught() {
        let fm = format!("name: toolong\ndescription: {}", "x".repeat(2000));
        let source = MockSource::new().with_skill("toolong", "meta", &fm);
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        let errs = report.errors_of(CheckKind::ListingBudget);
        assert_eq!(errs.len(), 1, "expected exactly one listing-budget error, got {errs:?}");
        assert!(matches!(errs[0],
            LintError::DescriptionTooLong { chars: 2000, cap: 1536, over: 464, .. }),
            "wrong measurement: {:?}", errs[0]);
    }

    /// The cap is configurable, so a consumer can gate tighter than the
    /// platform default without patching the checker.
    #[test]
    fn listing_cap_is_configurable() {
        let fm = format!("name: mid\ndescription: {}", "x".repeat(600));
        let source = MockSource::new().with_skill("mid", "meta", &fm);
        let tight = CheckConfig { max_desc_chars: 500, ..CheckConfig::default() };
        assert_eq!(check_all(&source, &tight).unwrap().errors_of(CheckKind::ListingBudget).len(), 1);
        let loose = CheckConfig { max_desc_chars: 700, ..CheckConfig::default() };
        assert_eq!(check_all(&source, &loose).unwrap().errors_of(CheckKind::ListingBudget).len(), 0);
    }

    // ─── Path resolution ─────────────────────────────────────────
    //
    // Every case below is pinned in BOTH directions. A gate never observed to
    // fail may be checking nothing, and a gate never observed to pass may be
    // checking everything — either way it stops being believed.

    /// RED: a relative link at a file that is not there is caught.
    #[test]
    fn dead_relative_link_is_caught() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "see [the notes](./references/notes.md)");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        let errs = report.errors_of(CheckKind::PathResolution);
        assert_eq!(errs.len(), 1, "expected exactly one, got {errs:?}");
        assert!(
            matches!(errs[0], LintError::UnresolvedPath { path, form, .. }
                if path == "./references/notes.md" && *form == PathForm::RelativeLink),
            "wrong finding: {:?}", errs[0]
        );
    }

    /// GREEN: the same link, with the file present, is silent.
    #[test]
    fn live_relative_link_passes() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "see [the notes](./references/notes.md)")
            .with_skill_file("alpha/references/notes.md");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 0);
    }

    /// RED: a repo-relative path into a cloned repo that lacks the file.
    #[test]
    fn dead_repo_path_is_caught() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "canonical: `theory/GHOST.md`")
            .with_repo("theory");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        let errs = report.errors_of(CheckKind::PathResolution);
        assert_eq!(errs.len(), 1, "expected exactly one, got {errs:?}");
        assert!(
            matches!(errs[0], LintError::UnresolvedPath { path, form, .. }
                if path == "theory/GHOST.md" && *form == PathForm::RepoPath),
            "wrong finding: {:?}", errs[0]
        );
    }

    /// GREEN: the same path, with the file present.
    #[test]
    fn live_repo_path_passes() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "canonical: `theory/THEORY.md`")
            .with_repo_file("theory/THEORY.md");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 0);
    }

    /// THE GATE — the property the whole check's trustworthiness rests on.
    ///
    /// The identical body that fires in `dead_repo_path_is_caught` must be
    /// SILENT when the repository is not cloned. Otherwise the check reports on
    /// the reader's checkout rather than on the corpus, everyone learns to
    /// ignore it, and it gets switched off — taking the true findings with it.
    #[test]
    fn repo_path_into_an_uncloned_repo_is_never_reported() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "canonical: `theory/GHOST.md`");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(
            report.errors_of(CheckKind::PathResolution).len(), 0,
            "an uncloned repo must be unknowable, not broken"
        );
    }

    /// A root segment that is both a repo name and a conventional in-repo
    /// directory cannot be attributed, so it is not reported. `docs/` alone
    /// produced 22 misattributions on the real corpus.
    #[test]
    fn ambiguous_root_segment_is_never_reported() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "see `docs/arch/landmarks.md`")
            .with_repo("docs");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 0);
    }

    /// A path shown inside a fence is an example, not a pointer.
    #[test]
    fn fenced_paths_are_out_of_scope() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "```\n`theory/GHOST.md`\n[x](./gone.md)\n```")
            .with_repo("theory");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 0);
    }

    /// The waiver excuses the path it NAMES — and only that one.
    #[test]
    fn waiver_clears_exactly_the_path_it_names() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body(
                "alpha",
                "uses `theory/GHOST.md` and `theory/OTHER.md`\n\n\
                 pending-path: theory/GHOST.md — lands with the next theory pass",
            )
            .with_repo("theory");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        let errs = report.errors_of(CheckKind::PathResolution);
        assert_eq!(errs.len(), 1, "waiver must not silence the whole skill: {errs:?}");
        assert!(
            matches!(errs[0], LintError::UnresolvedPath { path, .. } if path == "theory/OTHER.md"),
            "wrong survivor: {:?}", errs[0]
        );
    }

    /// A source locator addresses a position inside a file; the file is what
    /// resolves. Without stripping, 24 live corpus pointers read as dead.
    #[test]
    fn source_locator_does_not_make_a_live_path_dead() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "see `samba/src/config.rs:190-205` and `samba/tests/e2e.rs::paced`")
            .with_repo_file("samba/src/config.rs")
            .with_repo_file("samba/tests/e2e.rs");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 0);
    }

    /// One pointer repeated is one defect with one fix.
    #[test]
    fn a_repeated_dead_path_is_reported_once() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "`theory/GHOST.md` again `theory/GHOST.md` and `theory/GHOST.md`")
            .with_repo("theory");
        let report = check_all(&source, &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 1);
    }

    #[test]
    fn path_resolution_check_disabled() {
        let source = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "`theory/GHOST.md`")
            .with_repo("theory");
        let config = CheckConfig { path_resolution: false, ..CheckConfig::default() };
        let report = check_all(&source, &config).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 0);
    }

    /// A source that cannot resolve paths reports NOTHING, rather than
    /// reporting every path as broken. Unknown is not absent.
    #[test]
    fn a_source_without_an_oracle_reports_nothing() {
        struct Blind(MockSource);
        impl SkillSource for Blind {
            fn skill_map(&self) -> anyhow::Result<SkillMap> { self.0.skill_map() }
            fn skill_dirs(&self) -> anyhow::Result<BTreeSet<String>> { self.0.skill_dirs() }
            fn skill_content(&self, name: &str) -> anyhow::Result<String> {
                self.0.skill_content(name)
            }
            // path_oracle() left at its default None.
        }
        let inner = MockSource::new()
            .with_skill("alpha", "meta", &valid_fm("alpha"))
            .with_body("alpha", "`theory/GHOST.md` and [x](./gone.md)")
            .with_repo("theory");
        let report = check_all(&Blind(inner), &CheckConfig::default()).unwrap();
        assert_eq!(report.errors_of(CheckKind::PathResolution).len(), 0);
        assert!(report.is_ok(), "errors: {:?}", report.errors);
    }

    /// End-to-end on real disk: proves `discover_repo_root` finds the root that
    /// holds sibling repositories, which the in-memory oracle cannot exercise.
    #[test]
    fn filesystem_path_resolution_end_to_end() {
        use tempfile::TempDir;
        let org = TempDir::new().unwrap();

        // <org>/blackmatter/.git         — the repo the skills live in
        // <org>/blackmatter/skills/alpha — the skill
        // <org>/theory/THEORY.md         — a sibling repo with a real file
        let repo = org.path().join("blackmatter");
        let skills = repo.join("skills");
        let skill_dir = skills.join("alpha");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(repo.join(".git"), "gitdir: elsewhere\n").unwrap();
        fs::create_dir_all(org.path().join("theory")).unwrap();
        fs::write(org.path().join("theory").join("THEORY.md"), "# T\n").unwrap();
        fs::write(skills.join("skill-map.yaml"),
            "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains:\n  meta: [alpha]\nskills:\n  alpha:\n    description: A test\n    domain: meta\n    repo: test\n"
        ).unwrap();

        let live = "see `theory/THEORY.md`";
        let dead = "see `theory/THEORY.md` and `theory/GHOST.md` and [x](../beta/SKILL.md)";

        fs::write(skill_dir.join("SKILL.md"),
            format!("---\n{}\n---\n\n{live}\n", valid_fm("alpha"))).unwrap();
        let green = check_path(&skills).unwrap();
        assert_eq!(green.errors_of(CheckKind::PathResolution).len(), 0,
            "live pointers must be silent: {:?}", green.errors);

        fs::write(skill_dir.join("SKILL.md"),
            format!("---\n{}\n---\n\n{dead}\n", valid_fm("alpha"))).unwrap();
        let red = check_path(&skills).unwrap();
        let errs = red.errors_of(CheckKind::PathResolution);
        assert_eq!(errs.len(), 2, "expected the ghost and the bad link: {errs:?}");
        assert!(errs.iter().any(|e| matches!(e,
            LintError::UnresolvedPath { path, .. } if path == "theory/GHOST.md")));
        assert!(errs.iter().any(|e| matches!(e,
            LintError::UnresolvedPath { path, .. } if path == "../beta/SKILL.md")));
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

        let skill_dir = dir.path().join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), format!("---\n{}\n---\n\n# Body\n", valid_fm("test-skill"))).unwrap();

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
        let report = check_all(&source, &CheckConfig {
            // Reference-freshness is a FRESHNESS check — opt in, as a real
            // consumer must. Default has max_age_days: None (structure only).
            max_age_days: Some(90),
            ..CheckConfig::default()
        })
        .unwrap();
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
        // Default-ON: structural, objectively fixable, safe to gate CI on.
        assert!(cfg.path_resolution);
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
        // A directory without SKILL.md is not a skill — hence 0 discovered —
        // and discovering 0 is itself the (only) error.
        assert_eq!(report.skills_checked, 0);
        assert!(!report.is_ok());
        assert_eq!(report.errors_of(CheckKind::Discovery).len(), 1);
        assert_eq!(report.errors.len(), 1, "unexpected extras: {:?}", report.errors);
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
        let source = MockSource::new()
            .with_skill("a", "meta",
                "name: a\ndescription: A\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-17\"")
            .with_skill("b", "meta",
                "name: b\ndescription: B\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-10\"")
            .with_skill("c", "meta",
                "name: c\ndescription: C\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-03-05\"")
            .with_reference("b", "a")
            .with_reference("c", "b");
        let report = check_all(&source, &CheckConfig {
            // Reference-freshness is a FRESHNESS check — opt in, as a real
            // consumer must. Default has max_age_days: None (structure only).
            max_age_days: Some(90),
            ..CheckConfig::default()
        })
        .unwrap();
        let watch_errors = report.errors_of(CheckKind::References);
        assert!(watch_errors.iter().any(|e| matches!(e,
            LintError::ReferenceNewer { skill, reference, .. }
            if skill == "b" && reference == "a"
        )));
        assert!(watch_errors.iter().any(|e| matches!(e,
            LintError::ReferenceNewer { skill, reference, .. }
            if skill == "c" && reference == "b"
        )));
    }
}
