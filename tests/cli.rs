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

/// A skill body that points at a file which is not there fails the run, and the
/// message carries the path plus the waiver that would excuse it.
///
/// The RED half of the path-resolution gate. A gate never observed to fail may
/// be checking nothing.
#[test]
fn check_fails_on_a_dead_relative_link() {
    let dir = TempDir::new().unwrap();
    valid_skill(dir.path(), "alpha");
    valid_map(dir.path(), &[("alpha", "meta")]);
    let skill = dir.path().join("alpha").join("SKILL.md");
    let body = fs::read_to_string(&skill).unwrap() + "\nsee [notes](./references/notes.md)\n";
    fs::write(&skill, body).unwrap();

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("[path-resolution]"))
        .stderr(predicate::str::contains("./references/notes.md"))
        .stderr(predicate::str::contains("pending-path:"));
}

/// The GREEN half, twice over: a link whose target exists passes, and a link
/// whose target is declared absent with `pending-path:` passes too.
#[test]
fn check_passes_on_live_and_waived_paths() {
    let dir = TempDir::new().unwrap();
    valid_skill(dir.path(), "alpha");
    valid_map(dir.path(), &[("alpha", "meta")]);

    let refs = dir.path().join("alpha").join("references");
    fs::create_dir_all(&refs).unwrap();
    fs::write(refs.join("notes.md"), "# Notes\n").unwrap();

    let skill = dir.path().join("alpha").join("SKILL.md");
    let body = fs::read_to_string(&skill).unwrap()
        + "\nsee [notes](./references/notes.md) and [soon](./references/soon.md)\n\n\
           pending-path: ./references/soon.md — lands with the next pass\n";
    fs::write(&skill, body).unwrap();

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("all checks passed"));
}

/// The waiver is scoped to the path it names — it does not silence the skill.
#[test]
fn check_still_fails_on_an_unwaived_sibling_path() {
    let dir = TempDir::new().unwrap();
    valid_skill(dir.path(), "alpha");
    valid_map(dir.path(), &[("alpha", "meta")]);

    let skill = dir.path().join("alpha").join("SKILL.md");
    let body = fs::read_to_string(&skill).unwrap()
        + "\n[a](./gone-a.md) and [b](./gone-b.md)\n\npending-path: ./gone-a.md — accepted\n";
    fs::write(&skill, body).unwrap();

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("./gone-b.md"))
        .stderr(predicate::str::contains("./gone-a.md' does not resolve").not());
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
        .args([
            "check",
            "--skills-dir",
            skills_dir.to_str().unwrap(),
            // BASELINE DEBT, not a claim of cleanliness. Measured 2026-07-27,
            // the day path resolution landed: the real corpus carries 22 dead
            // pointers across 144 skills (3 relative links, 19 repo-relative
            // paths) — pre-existing defects in a repository this crate does not
            // own and must not silently "fix" by creating the files.
            //
            // This test's contract is the STRUCTURAL suite — sync, frontmatter,
            // map-integrity, version, listing-budget — and that contract is
            // still asserted in full. The companion test below runs the corpus
            // WITH resolution on, so the new check is exercised against real
            // data rather than only against fixtures. Drop this flag once the
            // 22 are fixed or waived upstream; do not use it to keep them.
            "--skip-path-resolution",
        ])
        .assert()
        .success()
        // Not merely exit-0: assert it actually linted something. Without this
        // the test would have gone green on the very "0 skills" vacuous pass
        // the DiscoveryChecker now forbids.
        .stderr(predicate::str::contains("all checks passed"))
        .stderr(predicate::str::contains("(0 skills)").not());
}

/// Run the real corpus WITH path resolution on, so the check meets real data.
///
/// It deliberately asserts neither success nor failure on the count: the corpus
/// lives in another repository and its dead-pointer debt moves under this
/// crate's feet in both directions. What it DOES assert is the property that
/// stays true either way — that path resolution is the only outstanding class,
/// i.e. no other check regressed behind the skip flag the test above passes.
/// The findings are printed so the debt stays visible rather than hidden by a
/// flag (`cargo test -- --nocapture`).
#[test]
fn real_skills_carry_no_error_class_other_than_path_resolution() {
    let default = format!(
        "{}/code/github/pleme-io/blackmatter-pleme/skills",
        std::env::var("HOME").unwrap_or_default()
    );
    let skills_dir =
        std::path::PathBuf::from(std::env::var("SKILL_LINT_REAL_SKILLS").unwrap_or(default));

    if !skills_dir.is_dir() {
        eprintln!(
            "SKIP real_skills_carry_no_error_class_other_than_path_resolution: \
             no fixture at {}",
            skills_dir.display()
        );
        return;
    }

    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["check", "--skills-dir", skills_dir.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!("REAL CORPUS path-resolution run:\n{stderr}");

    for other in ["[sync]", "[frontmatter]", "[map-integrity]", "[version]", "[discovery]", "[listing-budget]"] {
        assert!(!stderr.contains(other), "unexpected {other} error on the real corpus:\n{stderr}");
    }
}
