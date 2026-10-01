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

// ═══════════════════════════════════════════════════════════════════
// budget --strict — the listing budget as a gate
// ═══════════════════════════════════════════════════════════════════

/// A skill home holding one skill whose description is `desc_chars` long.
fn skill_with_description(dir: &std::path::Path, name: &str, desc_chars: usize) {
    let skill_dir = dir.join(name);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {}\n---\n\n# Body\n", "d".repeat(desc_chars)),
    )
    .unwrap();
}

fn budget(dir: &std::path::Path, extra: &[&str]) -> assert_cmd::assert::Assert {
    let mut args = vec!["budget", "--skills-dir", dir.to_str().unwrap()];
    args.extend_from_slice(extra);
    Command::cargo_bin("skill-lint").unwrap().args(&args).assert()
}

/// RED. `--strict` gated the TOTAL only, so a description the platform cuts
/// in half — trigger phrases and all — passed the gate as long as the listing
/// as a whole fit. The report printed "entries OVER" and exited 0.
#[test]
fn budget_strict_fails_when_one_entry_is_past_the_per_entry_cap() {
    let dir = TempDir::new().unwrap();
    skill_with_description(dir.path(), "wordy", 50);

    budget(dir.path(), &["--max-desc-chars", "10", "--budget-chars", "100000", "--strict"])
        .failure()
        .stderr(predicate::str::contains("past the 10-char per-entry cap"));
}

#[test]
fn budget_strict_fails_when_the_listing_is_over_budget() {
    let dir = TempDir::new().unwrap();
    skill_with_description(dir.path(), "big", 500);

    budget(dir.path(), &["--budget-chars", "100", "--strict"])
        .failure()
        .stderr(predicate::str::contains("over the 100-char listing budget"));
}

#[test]
fn budget_strict_passes_when_everything_fits() {
    let dir = TempDir::new().unwrap();
    skill_with_description(dir.path(), "small", 20);

    budget(dir.path(), &["--max-desc-chars", "100", "--budget-chars", "100000", "--strict"]).success();
}

/// The default is a report, not a gate — unchanged by `--strict` learning a
/// second failure condition.
#[test]
fn budget_without_strict_reports_and_exits_zero() {
    let dir = TempDir::new().unwrap();
    skill_with_description(dir.path(), "wordy", 500);

    // The listing counts the capped prefix (10 + name + separators = 19), so a
    // 5-char budget is over on the total as well as on the entry.
    budget(dir.path(), &["--max-desc-chars", "10", "--budget-chars", "5"])
        .success()
        .stdout(predicate::str::contains("OVER BUDGET"))
        .stdout(predicate::str::contains("entries OVER"));
}

// ═══════════════════════════════════════════════════════════════════
// claudemd — the anti-regrowth seal
// ═══════════════════════════════════════════════════════════════════

/// A `CLAUDE.md` with an index section holding the given bullets.
fn claude_md(dir: &std::path::Path, name: &str, bullets: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            "# Doc\n\n## ★★ Substrate primitive index\n\n\
             Each line: **rule** + skill (if any) + long-form doc.\n\n{bullets}\n"
        ),
    )
    .unwrap();
    path
}

/// RED. A gate never observed to fail may be checking nothing.
#[test]
fn claudemd_fails_on_an_over_ceiling_entry() {
    let dir = TempDir::new().unwrap();
    let file = claude_md(
        dir.path(),
        "CLAUDE.md",
        &format!("- **★★ Fat — a doctrine.** {}\n", "x".repeat(900)),
    );

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("[claudemd-entry]"))
        .stderr(predicate::str::contains("'Fat'"))
        .stderr(predicate::str::contains("over the 400 B ceiling"))
        .stderr(predicate::str::contains("--write-baseline"));
}

/// GREEN. A compliant file must pass, or the gate is noise rather than a seal.
#[test]
fn claudemd_passes_on_a_compliant_file() {
    let dir = TempDir::new().unwrap();
    let file = claude_md(
        dir.path(),
        "CLAUDE.md",
        "- **★★ Thin — a rule.** Skill: `x`. Doc: `theory/X.md`.\n\n\
         - ★ Also thin — another rule. Doc: `theory/Y.md`.\n",
    );

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 entries"))
        .stderr(predicate::str::contains("all checks passed"));
}

/// The bullet-matching trap, end to end.
///
/// One entry is written `- **`, the next `- ★`. A scanner that only knows
/// `- **` welds the star entry's bytes onto the one above it and reports a
/// phantom entry whose size is the sum of two — which is exactly the 8,434 B
/// entry the original audit of the live file reported and which never existed.
/// The one live star-prefixed entry has since been normalized, so this fixture
/// is the only coverage there is.
#[test]
fn claudemd_counts_a_star_prefixed_bullet_as_its_own_entry() {
    let dir = TempDir::new().unwrap();
    let file = claude_md(
        dir.path(),
        "CLAUDE.md",
        &format!(
            "- **★★ Bold — first.** small\n\n- ★★ Starred — second.** {}\n",
            "y".repeat(900)
        ),
    );

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap()])
        .assert()
        .failure()
        // Both entries are seen…
        .stdout(predicate::str::contains("2 entries"))
        // …and the overage is attributed to the STARRED one, not welded into
        // the bold one above it.
        .stderr(predicate::str::contains("'Starred'"))
        .stderr(predicate::str::contains("'Bold'").not());
}

/// A run over zero files exits non-zero. A linter wired into one repository of
/// seven is green because it never looked at the other six.
#[test]
fn claudemd_fails_when_no_files_are_given() {
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("vacuous pass"));
}

/// Which files were scanned is an OUTPUT of the run, not an assumption.
#[test]
fn claudemd_reports_every_file_it_scanned() {
    let dir = TempDir::new().unwrap();
    let a = claude_md(dir.path(), "a-CLAUDE.md", "- **★ Thin — rule.** doc\n");
    fs::write(dir.path().join("b-CLAUDE.md"), "# No index here\n").unwrap();

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args([
            "claudemd",
            "--file",
            a.to_str().unwrap(),
            "--file",
            dir.path().join("b-CLAUDE.md").to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("scanned 2 file(s)"))
        .stdout(predicate::str::contains("a-CLAUDE.md"))
        .stdout(predicate::str::contains("b-CLAUDE.md"))
        .stdout(predicate::str::contains("no index section matching"));
}

/// A missing file is a hard error, never a silent skip — dropping an unreadable
/// file is how a linter comes to cover fewer files than its operator believes.
#[test]
fn claudemd_fails_on_a_file_that_is_not_there() {
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", "/nonexistent/CLAUDE.md"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("/nonexistent/CLAUDE.md"));
}

/// The baseline-debt contract, both halves: recorded debt does not fail the
/// build, and a NEW violation beside it still does.
#[test]
fn claudemd_baseline_absolves_recorded_debt_but_not_a_new_violation() {
    let dir = TempDir::new().unwrap();
    let baseline = dir.path().join("baseline.txt");
    let file = claude_md(
        dir.path(),
        "CLAUDE.md",
        &format!("- **★★ Old — debt.** {}\n", "x".repeat(900)),
    );

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args([
            "claudemd",
            "--file",
            file.to_str().unwrap(),
            "--write-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Recorded debt: green.
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap(), "--baseline", baseline.to_str().unwrap()])
        .assert()
        .success();

    // A new entry beside it: red.
    claude_md(
        dir.path(),
        "CLAUDE.md",
        &format!(
            "- **★★ Old — debt.** {}\n\n- **★★ New — fresh.** {}\n",
            "x".repeat(900),
            "z".repeat(900)
        ),
    );
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap(), "--baseline", baseline.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("'New'"))
        .stderr(predicate::str::contains("'Old'").not());
}

/// The ratchet — the seal itself. A pure allowlist would let a baselined entry
/// drift from 1,400 B to 8,000 B in silence, which is the exact failure mode
/// this gate exists to close.
#[test]
fn claudemd_fails_when_baselined_debt_grows() {
    let dir = TempDir::new().unwrap();
    let baseline = dir.path().join("baseline.txt");
    let file = claude_md(
        dir.path(),
        "CLAUDE.md",
        &format!("- **★★ Creep — debt.** {}\n", "x".repeat(900)),
    );

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args([
            "claudemd",
            "--file",
            file.to_str().unwrap(),
            "--write-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();

    claude_md(
        dir.path(),
        "CLAUDE.md",
        &format!("- **★★ Creep — debt.** {}\n", "x".repeat(1500)),
    );
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap(), "--baseline", baseline.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("grew to"))
        .stderr(predicate::str::contains("never grow"));
}

/// The whole-file ceiling, and that it is baselineable like everything else.
#[test]
fn claudemd_enforces_the_whole_file_ceiling() {
    let dir = TempDir::new().unwrap();
    let file = claude_md(dir.path(), "CLAUDE.md", "- **★ Thin — rule.** doc\n");
    let big = fs::read_to_string(&file).unwrap() + &"filler ".repeat(2000);
    fs::write(&file, big).unwrap();

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap(), "--max-file-bytes", "1000"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("[claudemd-file]"))
        .stderr(predicate::str::contains("standing tax"));
}

/// Run the seal against the live file and prove the baseline round-trip on real
/// data: record what is there, then re-run and go green.
///
/// It deliberately does NOT assert an exit code on the unbaselined run — the
/// file lives in another repository and its debt moves under this crate's feet.
/// What it asserts is the property that stays true either way: the scan finds
/// the index section, measures entries, and a baseline written from that scan
/// makes the very same scan pass.
#[test]
fn claudemd_round_trips_on_the_real_org_file() {
    let default = format!(
        "{}/code/github/pleme-io/blackmatter-pleme/docs/pleme-io-CLAUDE.md",
        std::env::var("HOME").unwrap_or_default()
    );
    let file =
        std::path::PathBuf::from(std::env::var("SKILL_LINT_REAL_CLAUDEMD").unwrap_or(default));

    if !file.is_file() {
        // Announced, not silent — a skip must never be able to hide as a pass.
        eprintln!(
            "SKIP claudemd_round_trips_on_the_real_org_file: no fixture at {} \
             (set SKILL_LINT_REAL_CLAUDEMD)",
            file.display()
        );
        return;
    }

    let dir = TempDir::new().unwrap();
    let baseline = dir.path().join("baseline.txt");

    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args([
            "claudemd",
            "--file",
            file.to_str().unwrap(),
            "--write-baseline",
            baseline.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("REAL CLAUDE.md run:\n{stdout}");
    assert!(stdout.contains("Substrate primitive index"), "index section not found:\n{stdout}");
    assert!(!stdout.contains("index entries: 0 "), "measured zero entries:\n{stdout}");

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["claudemd", "--file", file.to_str().unwrap(), "--baseline", baseline.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("all checks passed"));
}

// ═══════════════════════════════════════════════════════════════════
// workflows — the no-shell seal at the shape that hides
// ═══════════════════════════════════════════════════════════════════

/// Write a minimal workflow whose single job holds the given step lines.
fn workflow(dir: &std::path::Path, name: &str, steps: &[&str]) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut lines: Vec<&str> =
        vec!["name: CI", "on:", "  push:", "", "jobs:", "  build:", "    runs-on: ubuntu-latest", "    steps:"];
    lines.extend_from_slice(steps);
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

/// RED, at the CLI. The defect this subcommand exists for: a long `run:` block
/// that reads as YAML rather than as a shell script.
#[test]
fn workflows_fails_on_a_long_inline_run() {
    let dir = TempDir::new().unwrap();
    let file = workflow(dir.path(), "rust-auto-release.yml", &[
        "      - name: Resolve test environment",
        "        run: |",
        "          shells=$(nix eval --impure --json --expr 'x')",
        "          if echo \"$shells\" | grep -qx '\"default\"'; then",
        "            echo 'installable=.#default' >> \"$GITHUB_OUTPUT\"",
        "          else",
        "            echo 'installable=github:pleme-io/substrate#release-gate' >> \"$GITHUB_OUTPUT\"",
        "          fi",
    ]);

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["workflows", "--file", file.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("[workflow-run]"))
        .stderr(predicate::str::contains("'Resolve test environment'"))
        .stderr(predicate::str::contains("over the 3-line glue allowance"))
        .stderr(predicate::str::contains("--write-baseline"));
}

/// GREEN. `uses:` steps plus one-line invocations — the target shape — must pass,
/// and a comment-heavy step must pass with them.
#[test]
fn workflows_passes_on_uses_plus_one_line_glue() {
    let dir = TempDir::new().unwrap();
    let file = workflow(dir.path(), "ci.yml", &[
        "      - uses: actions/checkout@v4",
        "      - uses: pleme-io/actions/tatara-script@main",
        "        with:",
        "          script: tools/release.tlisp",
        "      - name: Glue",
        "        run: cargo build --workspace --all-targets",
        "      - name: Documented glue",
        "        run: |",
        "          # Measured 2026-08-06: comments are not taxed, so the WHY",
        "          # can live at the call site where it is read.",
        "          # Five lines of rationale, one line of shell.",
        "          # Four.",
        "          # Five.",
        "          nix run .#gate",
    ]);

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["workflows", "--file", file.to_str().unwrap()])
        .assert()
        .success()
        // One flow-form step (`Glue`) and one block-form step (`Documented
        // glue`), whose five comment lines leave it measuring a single line of
        // shell. Both numbers are asserted because a green run whose counts are
        // wrong is still a green run.
        .stdout(predicate::str::contains("block-form run: steps     1   (0 over"))
        .stdout(predicate::str::contains("flow-form run: steps      1"))
        .stdout(predicate::str::contains("inline shell lines        1"))
        .stderr(predicate::str::contains("all checks passed"));
}

/// The coverage half of the gate, matching `claudemd` exactly.
#[test]
fn workflows_fails_when_no_files_are_given() {
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["workflows"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("vacuous pass"))
        .stderr(predicate::str::contains("a directory is not a file"));
}

#[test]
fn workflows_fails_on_a_file_that_is_not_there() {
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["workflows", "--file", "/nonexistent/ci.yml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("reading /nonexistent/ci.yml"));
}

/// The whole ratchet, end to end at the CLI: record the debt, go green, then
/// grow the block by one line and go red again.
#[test]
fn workflows_baseline_absolves_recorded_debt_and_catches_growth() {
    let dir = TempDir::new().unwrap();
    let baseline = dir.path().join("baseline.txt");
    let steps: Vec<String> = std::iter::once("      - name: Fat".to_owned())
        .chain(std::iter::once("        run: |".to_owned()))
        .chain((0..8).map(|n| format!("          echo line-{n}")))
        .collect();
    let refs: Vec<&str> = steps.iter().map(String::as_str).collect();
    let file = workflow(dir.path(), "ci.yml", &refs);

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args([
            "workflows",
            "--file",
            file.to_str().unwrap(),
            "--write-baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 recorded step(s)"));

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["workflows", "--file", file.to_str().unwrap(), "--baseline", baseline.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("all checks passed"));

    // Grow it. Baselined shell may shrink or hold, never grow.
    let grown: Vec<String> = std::iter::once("      - name: Fat".to_owned())
        .chain(std::iter::once("        run: |".to_owned()))
        .chain((0..9).map(|n| format!("          echo line-{n}")))
        .collect();
    let grown_refs: Vec<&str> = grown.iter().map(String::as_str).collect();
    workflow(dir.path(), "ci.yml", &grown_refs);

    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["workflows", "--file", file.to_str().unwrap(), "--baseline", baseline.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("grew to 9 lines"))
        .stderr(predicate::str::contains("from the 8 recorded"));
}

/// Run the seal against substrate's real workflow corpus and prove the baseline
/// round-trip on real data. Like the `claudemd` sibling it does NOT assert an
/// exit code on the unbaselined run — that corpus lives in another repository and
/// its debt moves under this crate's feet. What it asserts is the property that
/// stays true either way: the scan finds real block-form steps, at least one is
/// over the limit, and a baseline written from that scan makes the same scan pass.
///
/// Measured 2026-08-06 across substrate's 89 workflow files: 120 block-form
/// `run:` steps, 82 over 3 non-comment lines, 1,071 total lines of inline shell;
/// largest were image-push.yml 60, go-binary-release.yml 56,
/// rust-binary-release.yml 48. Those figures are a dated snapshot, so the
/// assertions below are floors rather than equalities.
#[test]
fn workflows_round_trips_on_substrates_real_corpus() {
    let default = format!(
        "{}/code/github/pleme-io/substrate/.github/workflows",
        std::env::var("HOME").unwrap_or_default()
    );
    let root =
        std::path::PathBuf::from(std::env::var("SKILL_LINT_REAL_WORKFLOWS").unwrap_or(default));

    if !root.is_dir() {
        // Announced, not silent — a skip must never be able to hide as a pass.
        eprintln!(
            "SKIP workflows_round_trips_on_substrates_real_corpus: no fixture at {} \
             (set SKILL_LINT_REAL_WORKFLOWS)",
            root.display()
        );
        return;
    }

    let mut files: Vec<String> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let ext = path.extension()?.to_str()?.to_owned();
            (ext == "yml" || ext == "yaml").then(|| path.to_string_lossy().into_owned())
        })
        .collect();
    files.sort();
    assert!(files.len() > 20, "expected a real corpus, found {} file(s)", files.len());

    let dir = TempDir::new().unwrap();
    let baseline = dir.path().join("baseline.txt");

    let mut args: Vec<String> = vec!["workflows".to_owned()];
    for file in &files {
        args.push("--file".to_owned());
        args.push(file.clone());
    }
    let mut record = args.clone();
    record.push("--write-baseline".to_owned());
    record.push(baseline.to_string_lossy().into_owned());

    let out = Command::cargo_bin("skill-lint").unwrap().args(&record).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("REAL substrate workflows run:\n{stdout}");
    assert!(!stdout.contains("block-form run: steps       0"), "measured zero blocks:\n{stdout}");
    let recorded = fs::read_to_string(&baseline).unwrap();
    let rows = recorded.lines().filter(|l| !l.starts_with('#')).count();
    assert!(rows > 20, "expected real inline-shell debt, recorded {rows} row(s):\n{stdout}");

    let mut verify = args;
    verify.push("--baseline".to_owned());
    verify.push(baseline.to_string_lossy().into_owned());
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(&verify)
        .assert()
        .success()
        .stderr(predicate::str::contains("all checks passed"));
}

// ═══════════════════════════════════════════════════════════════════
// chain — what one session actually loads
// ═══════════════════════════════════════════════════════════════════

/// A committed fixture's `home/`, canonical so it compares with what the tool
/// reports. Each case under `tests/fixtures/chain/` is a whole `$HOME`, so the
/// walk stops inside the fixture and never reads this repository's own
/// `CLAUDE.md` or anything above it.
fn chain_home(case: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/chain")
        .join(case)
        .join("home")
        .canonicalize()
        .unwrap()
}

/// Run `chain` over `case` with one `--dir` per entry of `dirs` (relative to
/// the fixture's home).
fn chain(case: &str, dirs: &[&str], extra: &[&str]) -> assert_cmd::assert::Assert {
    let home = chain_home(case);
    let mut args: Vec<String> = vec!["chain".into(), "--home".into(), home.display().to_string()];
    for dir in dirs {
        args.push("--dir".into());
        args.push(home.join(dir).display().to_string());
    }
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    Command::cargo_bin("skill-lint").unwrap().args(&args).assert()
}

/// The `--json` report of a run, whatever its exit status.
fn chain_json(case: &str, dirs: &[&str], extra: &[&str]) -> serde_json::Value {
    let mut extra = extra.to_vec();
    extra.push("--json");
    let out = chain(case, dirs, &extra).get_output().stdout.clone();
    serde_json::from_slice(&out).unwrap_or_else(|e| {
        panic!("--json did not print JSON ({e}):\n{}", String::from_utf8_lossy(&out))
    })
}

/// One session's loaded files as `(path relative to home, origin, imported_by
/// relative to home, line, depth)`, in load order.
fn loaded(json: &serde_json::Value, session: usize, home: &std::path::Path) -> Vec<(String, String, String, u64, u64)> {
    let rel = |v: &serde_json::Value| {
        v.as_str().map_or_else(String::new, |p| {
            std::path::Path::new(p).strip_prefix(home).unwrap_or_else(|_| panic!("{p} is outside the fixture home")).display().to_string()
        })
    };
    json["sessions"][session]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                rel(&f["path"]),
                f["origin"].as_str().unwrap().to_owned(),
                rel(&f["imported_by"]),
                f["line"].as_u64().unwrap_or(0),
                f["depth"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn paths(rows: &[(String, String, String, u64, u64)]) -> Vec<&str> {
    rows.iter().map(|r| r.0.as_str()).collect()
}

/// RED. Global first, then every directory from home down to the session
/// directory, root-most first, in each of its three spellings. A `CLAUDE.md`
/// BELOW the session directory is never loaded at start-up.
#[test]
fn chain_loads_global_then_root_most_to_the_session_dir() {
    let home = chain_home("order");
    let json = chain_json("order", &["a/b/c"], &[]);
    let rows = loaded(&json, 0, &home);
    assert_eq!(
        paths(&rows),
        [
            ".claude/CLAUDE.md",
            "CLAUDE.md",
            "a/CLAUDE.md",
            "a/CLAUDE.local.md",
            "a/b/.claude/CLAUDE.md",
            "a/b/c/CLAUDE.md",
        ]
    );
    assert_eq!(rows[0].1, "global");
    assert!(rows[1..].iter().all(|r| r.1 == "chain"), "{rows:?}");

    let total: u64 = rows.iter().map(|r| fs::metadata(home.join(&r.0)).unwrap().len()).sum();
    assert_eq!(json["sessions"][0]["total_bytes"].as_u64(), Some(total));
}

/// A session started in `$HOME` finds `~/.claude/CLAUDE.md` twice — as the
/// global file and as `$HOME`'s own `.claude/CLAUDE.md`. It is loaded, and
/// counted, once. Each `--dir` is its own session with its own total.
#[test]
fn chain_loads_the_global_once_and_reports_each_dir_as_its_own_session() {
    let home = chain_home("order");
    let json = chain_json("order", &["", "a"], &[]);
    assert_eq!(paths(&loaded(&json, 0, &home)), [".claude/CLAUDE.md", "CLAUDE.md"]);
    assert_eq!(
        paths(&loaded(&json, 1, &home)),
        [".claude/CLAUDE.md", "CLAUDE.md", "a/CLAUDE.md", "a/CLAUDE.local.md"]
    );
}

/// RED. Relative (`docs/x`, `./x`, `../x`) imports resolve against the
/// IMPORTING file's directory, `~/` against home, recursively. A file reached
/// twice (the diamond on `notes/shared.md` and `docs/dot.md`) loads once.
#[test]
fn chain_follows_relative_and_tilde_imports_recursively() {
    let home = chain_home("imports");
    let json = chain_json("imports", &["proj"], &[]);
    let rows = loaded(&json, 0, &home);
    let expected: Vec<(String, String, String, u64, u64)> = [
        (".claude/CLAUDE.md", "global", "", 0, 0),
        ("notes/shared.md", "import", ".claude/CLAUDE.md", 3, 1),
        ("proj/CLAUDE.md", "chain", "", 0, 0),
        ("proj/docs/rel.md", "import", "proj/CLAUDE.md", 3, 1),
        ("proj/docs/sub/nested.md", "import", "proj/docs/rel.md", 3, 2),
        ("proj/docs/dot.md", "import", "proj/docs/rel.md", 4, 2),
    ]
    .iter()
    .map(|(p, o, b, l, d)| ((*p).to_owned(), (*o).to_owned(), (*b).to_owned(), *l, *d))
    .collect();
    assert_eq!(rows, expected);
}

/// An `@path` shown in a fence or an inline code span is an example, not an
/// import; an address and a bare `@` are not paths. Were any of them read as
/// imports, the `*-missing.md` targets would fail the run.
#[test]
fn chain_ignores_imports_in_code_fences_and_inline_code() {
    chain("imports", &["proj"], &[])
        .success()
        .stdout(predicate::str::contains("missing").not())
        .stderr(predicate::str::contains("all checks passed"));
}

/// A cycle costs nothing at load time — the second visit is a no-op — so it is
/// reported, not failed.
#[test]
fn chain_reports_an_import_cycle_without_failing() {
    let home = chain_home("cycle");
    let json = chain_json("cycle", &["proj"], &[]);
    assert_eq!(paths(&loaded(&json, 0, &home)), ["proj/CLAUDE.md", "proj/a.md", "proj/b.md"]);
    let cycles = json["sessions"][0]["cycles"].as_array().unwrap();
    assert_eq!(cycles.len(), 1, "{cycles:?}");
    assert!(cycles[0]["file"].as_str().unwrap().ends_with("proj/b.md"));
    assert!(cycles[0]["target"].as_str().unwrap().ends_with("proj/a.md"));
    assert_eq!(cycles[0]["line"].as_u64(), Some(3));

    chain("cycle", &["proj"], &[]).success().stdout(predicate::str::contains("cycle"));
}

/// RED. A dangling import is a silent no-op in Claude Code; here it is an
/// error, and the file it sits in is still measured.
#[test]
fn chain_fails_on_a_missing_import_target() {
    let home = chain_home("missing");
    chain("missing", &["proj"], &[])
        .failure()
        .stderr(predicate::str::contains("[claudemd-chain]"))
        .stderr(predicate::str::contains("CLAUDE.md:4 imports '@docs/gone.md'"));
    let json = chain_json("missing", &["proj"], &[]);
    assert_eq!(paths(&loaded(&json, 0, &home)), ["proj/CLAUDE.md", "proj/docs/present.md"]);
    assert_eq!(json["ok"].as_bool(), Some(false));
}

/// Five hops load; the sixth is dropped by Claude Code without a word, so it
/// fails here.
#[test]
fn chain_fails_on_an_import_past_the_hop_limit() {
    let home = chain_home("deep");
    let json = chain_json("deep", &["proj"], &[]);
    let rows = loaded(&json, 0, &home);
    assert_eq!(rows.last().map(|r| (r.0.as_str(), r.4)), Some(("proj/d5.md", 5)), "{rows:?}");
    chain("deep", &["proj"], &[])
        .failure()
        .stderr(predicate::str::contains("'@d6.md'"))
        .stderr(predicate::str::contains("5-hop"));
}

/// RED. A session that loads nothing is a vacuous pass, not a success.
#[test]
fn chain_refuses_a_session_that_loads_zero_files() {
    chain("empty", &["proj"], &[])
        .failure()
        .stderr(predicate::str::contains("vacuous pass"));
}

#[test]
fn chain_fails_on_a_dir_that_is_not_there() {
    chain("order", &["no/such/dir"], &[]).failure();
}

/// RED. The ceilings are gates: over the total, or one file over its own.
#[test]
fn chain_fails_over_either_ceiling_and_passes_under_both() {
    chain("order", &["a/b/c"], &["--max-bytes", "10"])
        .failure()
        .stderr(predicate::str::contains("over the 10 B --max-bytes ceiling"));
    chain("order", &["a/b/c"], &["--max-file-bytes", "5"])
        .failure()
        .stderr(predicate::str::contains("over the 5 B --max-file-bytes ceiling"));
    chain("order", &["a/b/c"], &["--max-bytes", "100000", "--max-file-bytes", "1000"])
        .success()
        .stderr(predicate::str::contains("all checks passed"));
}
