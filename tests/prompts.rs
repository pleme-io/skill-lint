use assert_cmd::Command;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Env {
    home: TempDir,
    state: TempDir,
}

impl Env {
    fn new() -> Self {
        let e = Self { home: TempDir::new().unwrap(), state: TempDir::new().unwrap() };
        for (n, d) in [("helm", "Create Helm charts and releases for kubernetes"), ("rusty", "Rust binary or library tool")] {
            let dir = e.skills().join(n);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), format!("---\nname: {n}\ndescription: \"{d}\"\n---\nbody\n")).unwrap();
        }
        e
    }

    fn skills(&self) -> PathBuf { self.home.path().join(".claude/skills") }

    fn log(&self) -> PathBuf { self.state.path().join("skill-lint/usage.jsonl") }

    fn record(&self, event: &serde_json::Value, extra: &[&str]) {
        let mut args = vec!["usage", "record"];
        args.extend_from_slice(extra);
        let out = Command::cargo_bin("skill-lint")
            .unwrap()
            .args(&args)
            .env("HOME", self.home.path())
            .env("XDG_STATE_HOME", self.state.path())
            .write_stdin(event.to_string())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
    }

    fn prompt(&self, session: &str, text: &str) { self.record(&json!({"hook_event_name": "UserPromptSubmit", "session_id": session, "prompt": text}), &[]); }

    fn skill_call(&self, session: &str, skill: &str) {
        self.record(&json!({"hook_event_name": "PreToolUse", "session_id": session, "tool_name": "Skill", "tool_input": {"skill": skill}}), &[]);
    }

    fn misses(&self, extra: &[&str]) -> String {
        let out = Command::cargo_bin("skill-lint")
            .unwrap()
            .args(["usage", "misses", "--skills-dir"])
            .arg(self.skills())
            .arg("--workspace-root")
            .arg(self.home.path().join("ws"))
            .arg("--log")
            .arg(self.log())
            .args(extra)
            .env("HOME", self.home.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }
}

fn all_files(root: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(root).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

const TOKENS: &[&str] = &[concat!("ghp", "_FAKEFAKEFAKEFAKEFAKEFAKEFAKE0123456789"), concat!("AKI", "AFAKEFAKEFAKE1234"), concat!("sk-", "ant-api03-ZZZZZZZZZZZZZZZZZZZZ"), "0123456789abcdef0123456789abcdef"];

#[test]
fn a_prompt_with_secret_looking_tokens_leaves_no_trace_in_the_log() {
    let env = Env::new();
    let prompt = format!("zebracorn create helm charts release for kubernetes {}", TOKENS.join(" "));
    env.prompt("s1", &prompt);
    let log = fs::read_to_string(env.log()).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
    for needle in TOKENS.iter().flat_map(|t| [*t, &t[..8]]).chain(["zebracorn", "FAKEFAKE", "ZZZZZZ", "create helm"]) {
        assert!(!log.contains(needle), "log leaks {needle:?}: {log}");
    }
    let v: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(v["candidates"][0]["skill"], "helm");
    assert!(v["candidates"][0]["permille"].as_u64().unwrap() > 0);
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["candidates", "event", "id", "session_id", "ts"]);
}

#[test]
fn nothing_under_the_state_or_home_dirs_holds_the_prompt_or_a_token() {
    let env = Env::new();
    env.prompt("s1", &format!("zebracorn create helm charts release kubernetes {}", TOKENS.join(" ")));
    let mut files = Vec::new();
    all_files(env.state.path(), &mut files);
    all_files(env.home.path(), &mut files);
    assert!(!files.is_empty());
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        for needle in TOKENS.iter().copied().chain(["zebracorn"]) {
            assert!(!text.contains(needle), "{} leaks {needle}", f.display());
        }
    }
}

#[test]
fn why_not_never_prints_a_token_from_the_prompt() {
    let env = Env::new();
    let prompt = format!("zebracorn create helm charts release kubernetes {}", TOKENS.join(" "));
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["why-not", "--skill", "rusty", "--skills-dir"])
        .arg(env.skills())
        .arg("--workspace-root")
        .arg(env.home.path().join("ws"))
        .arg("--log")
        .arg(env.log())
        .env("HOME", env.home.path())
        .write_stdin(prompt)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    for t in TOKENS {
        assert!(!text.contains(&t[..8]), "{text}");
    }
    assert!(text.contains("prompt match"), "{text}");
}

#[test]
fn a_prompt_that_only_carries_a_secret_writes_no_line() {
    let env = Env::new();
    env.prompt("s1", &format!("here {}", TOKENS[0]));
    assert!(!env.log().exists());
}

#[test]
fn no_prompts_turns_the_candidate_line_off_and_keeps_the_skill_line() {
    let env = Env::new();
    let ev = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "create helm charts release kubernetes"});
    env.record(&ev, &["--no-prompts"]);
    assert!(!env.log().exists());
    env.skill_call("s", "helm");
    assert_eq!(fs::read_to_string(env.log()).unwrap().lines().count(), 1);
}

#[test]
fn an_older_binary_flag_set_still_exits_zero_and_silent() {
    let env = Env::new();
    env.record(&json!({"hook_event_name": "UserPromptSubmit", "prompt": "create helm charts"}), &["--some-future-flag"]);
}

#[test]
fn misses_lists_the_prompt_that_fired_nothing_and_not_the_one_that_did() {
    let env = Env::new();
    let text = "create helm charts release for kubernetes";
    env.prompt("s1", text);
    env.prompt("s2", text);
    env.skill_call("s2", "helm");
    let out = env.misses(&["--window", "30"]);
    assert!(out.contains("2 prompts, 2 with a strong candidate"), "{out}");
    assert!(out.contains("helm: 1 of 2 strong prompts missed (500/1000)"), "{out}");
    assert!(!out.contains("create helm"), "{out}");
}

#[test]
fn misses_json_carries_ids_and_scores_and_no_prompt_text() {
    let env = Env::new();
    env.prompt("s1", &format!("zebracorn create helm charts release kubernetes {}", TOKENS[0]));
    let out = env.misses(&["--json", "--window", "all"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["skills"][0]["skill"], "helm");
    assert_eq!(v["skills"][0]["prompts"][0]["id"].as_str().unwrap().len(), 16);
    assert!(!out.contains("zebracorn") && !out.contains("FAKEFAKE"));
}

#[test]
fn misses_names_a_skill_that_is_no_longer_deployed_as_not_deployed() {
    let env = Env::new();
    env.prompt("s1", "create helm charts release for kubernetes");
    fs::remove_dir_all(env.skills().join("helm")).unwrap();
    let out = env.misses(&["--window", "all"]);
    assert!(out.contains("dominant reason not_deployed"), "{out}");
}
