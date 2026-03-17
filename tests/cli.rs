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
    let default = format!(
        "{}/code/github/pleme-io/blackmatter-pleme/skills",
        std::env::var("HOME").unwrap_or_default()
    );
    let skills_dir =
        std::path::PathBuf::from(std::env::var("SKILL_LINT_REAL_SKILLS").unwrap_or(default));
    if skills_dir.join("skill-map.yaml").exists() {
        Command::cargo_bin("skill-lint")
            .unwrap()
            .args(["check", "--skills-dir", skills_dir.to_str().unwrap()])
            .assert()
            .success();
    }
}
