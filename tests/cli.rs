use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn valid_skill(dir: &std::path::Path, name: &str) {
    let skill_dir = dir.join(name);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: A {name} skill\nmetadata:\n  version: \"1.0.0\"\n  last_verified: \"2026-01-01\"\n---\n\n# Body\n"
        ),
    )
    .unwrap();
}

fn valid_map(dir: &std::path::Path, skills: &[(&str, &str)]) {
    let domain_entries: String = skills
        .iter()
        .map(|(name, domain)| format!("    {domain}: [{name}]"))
        .collect::<Vec<_>>()
        .join("\n");
    let skill_entries: String = skills
        .iter()
        .map(|(name, domain)| {
            format!(
                "  {name}:\n    description: A {name}\n    domain: {domain}\n    repo: test"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        dir.join("skill-map.yaml"),
        format!("version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains:\n{domain_entries}\nskills:\n{skill_entries}\n"),
    )
    .unwrap();
}

#[test]
fn check_succeeds_on_valid_setup() {
    let dir = TempDir::new().unwrap();
    valid_skill(dir.path(), "alpha");
    valid_map(dir.path(), &[("alpha", "meta")]);

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("all checks passed"));
}

#[test]
fn check_fails_on_missing_map() {
    let dir = TempDir::new().unwrap();
    valid_skill(dir.path(), "alpha");
    // No skill-map.yaml

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .failure();
}

#[test]
fn check_fails_when_no_skills_are_discovered() {
    // The seal: a run that lints nothing must never report success. Before
    // this, `skill-lint check` from a root whose skills live elsewhere printed
    // "all checks passed (0 skills)" and exited 0 — a gate that validates
    // nothing while manufacturing confidence.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("skill-map.yaml"),
        "version: \"1.0.0\"\nlastModified: \"2026-03-17\"\ndomains: {}\nskills: {}\n",
    )
    .unwrap();

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no skills found"))
        .stderr(predicate::str::contains(dir.path().to_str().unwrap()));
}

#[test]
fn check_discovers_nested_skills_dir_from_repo_root() {
    // The bare invocation from a repo root finds ./skills — same verdict as
    // pointing at it explicitly.
    let root = TempDir::new().unwrap();
    let skills = root.path().join("skills");
    fs::create_dir_all(&skills).unwrap();
    valid_skill(&skills, "alpha");
    valid_map(root.path(), &[("alpha", "meta")]);

    for target in [root.path(), skills.as_path()] {
        Command::cargo_bin("skill-lint")
            .unwrap()
            .args(["check", "--skills-dir", target.to_str().unwrap()])
            .assert()
            .success()
            .stderr(predicate::str::contains("all checks passed (1 skills)"));
    }
}

#[test]
fn check_fails_on_orphan_skill() {
    let dir = TempDir::new().unwrap();
    valid_skill(dir.path(), "alpha");
    valid_skill(dir.path(), "orphan");
    valid_map(dir.path(), &[("alpha", "meta")]);

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("orphan"));
}

#[test]
fn check_reports_error_count() {
    let dir = TempDir::new().unwrap();
    // Map with no version and a ghost entry — at least 3 errors
    fs::write(
        dir.path().join("skill-map.yaml"),
        "domains:\n  meta: [ghost]\nskills:\n  ghost:\n    description: X\n    domain: meta\n    repo: t\n",
    )
    .unwrap();

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error(s)"));
}

#[test]
fn check_passes_on_real_skills() {
    // Run against the actual blackmatter-pleme skills if available.
    // Uses SKILL_LINT_REAL_SKILLS env var or default path.
    //
    // This gate used to be `skills_dir.join("skill-map.yaml").exists()` — the
    // real repo carries a split `skill-map.d/`, so the condition was never
    // true and the test never once executed. That is the same defect this
    // tool now refuses in its own subject: a check that reports success
    // without checking anything. Two rules keep it closed:
    //
    //   1. Gate on the FIXTURE being absent, never on where the map happens
    //      to live. Locating the map is the production lookup's job — the CLI
    //      below performs it. A second copy of that resolution here is what
    //      drifted out from under the real one in the first place.
    //   2. When the fixture IS present, run and assert. A missing or
    //      unresolvable map is then a failure, not a silent skip.
    let default = format!(
        "{}/code/github/pleme-io/blackmatter-pleme/skills",
        std::env::var("HOME").unwrap_or_default()
    );
    let skills_dir =
        std::path::PathBuf::from(std::env::var("SKILL_LINT_REAL_SKILLS").unwrap_or(default));

    if !skills_dir.is_dir() {
        // Legitimate skip: a machine without blackmatter-pleme checked out.
        // Announced, not silent — visible under `cargo test -- --nocapture`
        // (or `--show-output`), so a skip can never hide as a pass again.
        eprintln!(
            "SKIP check_passes_on_real_skills: no fixture at {} \
             (set SKILL_LINT_REAL_SKILLS to point at a real skills dir)",
            skills_dir.display()
        );
        return;
    }

    eprintln!("RUN check_passes_on_real_skills against {}", skills_dir.display());
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", skills_dir.to_str().unwrap()])
        .assert()
        .success()
        // Not merely exit-0: assert it actually linted something. Without this
        // the test would have gone green on the very "0 skills" vacuous pass
        // the DiscoveryChecker now forbids.
        .stderr(predicate::str::contains("all checks passed"))
        .stderr(predicate::str::contains("(0 skills)").not());
}
