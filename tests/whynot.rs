use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;

fn fix() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/whynot") }

const BUDGET: &str = "218";
const LEXICAL: &[&str] = &["gap", "outranked", "rival"];

fn why_not(skill: &str) -> String {
    let f = fix();
    let mut cmd = Command::cargo_bin("skill-lint").unwrap();
    cmd.args(["why-not", "--skill", skill]);
    if LEXICAL.contains(&skill) {
        cmd.arg("--prompt-file").arg(f.join("prompt.txt"));
    }
    let out = cmd
        .arg("--skills-dir")
        .arg(f.join("deployed"))
        .arg("--workspace-root")
        .arg(f.join("ws"))
        .arg("--source-dir")
        .arg(f.join("ws/blackmatter-fixture/skills"))
        .arg("--map-dir")
        .arg(f.join("ws/blackmatter-fixture/skill-map.d"))
        .arg("--plugin-dir")
        .arg(f.join("no-plugins"))
        .arg("--router-root")
        .arg(f.join("router"))
        .arg("--project-skills-dir")
        .arg(f.join("no-project"))
        .arg("--log")
        .arg(f.join("usage.jsonl"))
        .args(["--budget-chars", BUDGET, "--now", "2026-10-21T00:00:00Z"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().replace(f.to_str().unwrap(), "$FIX")
}

fn golden(skill: &str) {
    let got = why_not(skill);
    let path = fix().join("golden").join(format!("{skill}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &got).unwrap();
    }
    let want = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(got, want, "golden drift for {skill}; rerun with UPDATE_GOLDEN=1 and review the diff");
}

#[test]
fn golden_not_deployed() { golden("ghost"); }

#[test]
fn golden_dropped_by_budget() { golden("dropped"); }

#[test]
fn golden_lexical_gap() { golden("gap"); }

#[test]
fn golden_outranked() { golden("outranked"); }

#[test]
fn golden_stale_deploy() { golden("stale"); }

#[test]
fn golden_healthy_best_match() { golden("rival"); }

#[test]
fn each_failure_class_leads_its_own_diagnosis() {
    for (skill, class) in [("ghost", "not_deployed"), ("dropped", "dropped_by_budget"), ("gap", "lexical_gap"), ("outranked", "outranked"), ("stale", "stale_deploy")] {
        let text = why_not(skill);
        assert!(text.contains(&format!("] {class}")), "{skill} should report {class}:\n{text}");
    }
}

#[test]
fn json_output_parses_and_lists_ranked_reasons() {
    let f = fix();
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["why-not", "--skill", "ghost", "--json", "--skills-dir"])
        .arg(f.join("deployed"))
        .arg("--workspace-root")
        .arg(f.join("ws"))
        .arg("--source-dir")
        .arg(f.join("ws/blackmatter-fixture/skills"))
        .arg("--router-root")
        .arg(f.join("router"))
        .arg("--log")
        .arg(f.join("usage.jsonl"))
        .write_stdin("")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["reasons"][0]["class"], "not_deployed");
    assert!(v["lexical"].is_null());
}
