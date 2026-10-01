use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixtures() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/usage") }

fn mcp_fixture(name: &str) -> PathBuf { fixtures().join("mcp").join(name) }

fn event(name: &str) -> Vec<u8> { fs::read(fixtures().join("events").join(name)).unwrap() }

fn record(state: &Path, stdin: Vec<u8>) -> PathBuf {
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["usage", "record"])
        .env("HOME", fixtures().join("home"))
        .env("XDG_STATE_HOME", state)
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "record must always exit 0: {out:?}");
    assert!(out.stdout.is_empty(), "record must never print: {:?}", String::from_utf8_lossy(&out.stdout));
    state.join("skill-lint/usage.jsonl")
}

fn lines(log: &Path) -> Vec<Value> {
    fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}

fn records_nothing(stdin: Vec<u8>) {
    let state = TempDir::new().unwrap();
    let log = record(state.path(), stdin);
    assert!(!log.exists(), "expected no write, found {:?}", fs::read_to_string(&log));
}

#[test]
fn an_mcp_tool_call_is_recorded_with_server_and_tool() {
    let state = TempDir::new().unwrap();
    let log = record(state.path(), event("pre-tool-use-mcp.json"));
    let got = lines(&log);
    assert_eq!(got.len(), 1);
    let line = &got[0];
    assert_eq!(line["event"], "PreToolUse");
    assert_eq!(line["trigger"], "mcp");
    assert_eq!(line["server"], "search-srv");
    assert_eq!(line["tool"], "query");
    assert_eq!(line["session_id"], "sess-5");
    assert_eq!(line["cwd"], "/home/u/repo");
    assert_eq!(line["transcript_path"], "/home/u/.claude/projects/-home-u-repo/sess-5.jsonl");
    assert!(line.get("skill").is_none(), "{line}");
}

#[test]
fn an_mcp_tool_call_never_stores_its_arguments() {
    let state = TempDir::new().unwrap();
    let log = record(state.path(), event("pre-tool-use-mcp.json"));
    let text = fs::read_to_string(&log).unwrap();
    assert!(!text.contains("SECRET"), "{text}");
    assert!(lines(&log)[0].get("tool_input").is_none(), "{text}");
}

#[test]
fn a_server_name_with_single_underscores_splits_at_the_double_underscore() {
    let state = TempDir::new().unwrap();
    let got = lines(&record(state.path(), event("pre-tool-use-mcp-underscored-server.json")));
    assert_eq!(got[0]["server"], "hosted_Docs_srv");
    assert_eq!(got[0]["tool"], "read_page");
}

#[test]
fn a_post_tool_use_mcp_event_is_not_counted_twice() { records_nothing(event("post-tool-use-mcp.json")); }

#[test]
fn an_mcp_name_without_a_tool_records_nothing() { records_nothing(event("pre-tool-use-mcp-no-tool.json")); }

#[test]
fn an_mcp_name_without_a_server_records_nothing() { records_nothing(event("pre-tool-use-mcp-empty-server.json")); }

#[test]
fn skill_and_mcp_calls_share_one_log() {
    let state = TempDir::new().unwrap();
    record(state.path(), event("pre-tool-use-skill.json"));
    let log = record(state.path(), event("pre-tool-use-mcp.json"));
    let triggers: Vec<String> = lines(&log).iter().map(|l| l["trigger"].as_str().unwrap().to_owned()).collect();
    assert_eq!(triggers, ["tool", "mcp"]);
}

#[test]
fn the_skill_report_skips_mcp_lines_without_calling_them_malformed() {
    let skills = fixtures().join("home/.claude/skills");
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["usage", "report", "--json", "--since", "30d", "--now", "2026-10-01T00:00:00Z", "--log"])
        .arg(fixtures().join("usage.jsonl"))
        .arg("--skills-dir")
        .arg(&skills)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["malformed_lines"], 2);
    assert_eq!(json["mcp_lines"], 2);
    assert_eq!(json["events_in_window"], 5);
}

fn mcp_report(home: Option<&Path>, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::cargo_bin("skill-lint").unwrap();
    cmd.args(["usage", "mcp-report", "--now", "2026-10-01T00:00:00Z", "--log"]).arg(mcp_fixture("usage.jsonl"));
    cmd.args(extra);
    match home {
        Some(h) => cmd.env("HOME", h),
        None => cmd.env("HOME", "/nonexistent-home"),
    };
    cmd.output().unwrap()
}

fn mcp_json(home: Option<&Path>, extra: &[&str]) -> Value {
    let mut extra = extra.to_vec();
    extra.push("--json");
    let out = mcp_report(home, &extra);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn configured_args() -> Vec<String> {
    vec![
        "--since".into(),
        "30d".into(),
        "--config".into(),
        mcp_fixture("claude.json").display().to_string(),
        "--tools".into(),
        mcp_fixture("tools.txt").display().to_string(),
    ]
}

fn full_json() -> Value {
    let args = configured_args();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    mcp_json(None, &refs)
}

fn names(v: &Value, key: &str) -> Vec<String> {
    v[key].as_array().unwrap().iter().map(|e| e["server"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn mcp_report_counts_calls_and_distinct_tools_per_server() {
    let json = full_json();
    assert_eq!(json["since"], "2026-09-01T00:00:00Z");
    assert_eq!(json["events_in_window"], 6);
    assert_eq!(json["lines_read"], 11);
    assert_eq!(json["malformed_lines"], 3);
    assert_eq!(json["skill_lines"], 1);
    assert_eq!(names(&json, "used"), ["alpha-srv", "beta-srv", "dotted_srv", "hosted_Docs_srv"]);
    let alpha = &json["used"][0];
    assert_eq!(alpha["calls"], 3);
    assert_eq!(alpha["distinct_tools"], 2);
    assert_eq!(alpha["tools_used"], serde_json::json!(["get", "list"]));
    assert_eq!(alpha["last_used"], "2026-09-30T10:00:00Z");
    assert_eq!(alpha["tool_count"], 3);
    assert_eq!(alpha["config_name"], "alpha-srv");
}

#[test]
fn mcp_report_matches_a_configured_name_through_tool_name_normalisation() {
    let json = full_json();
    let dotted = json["used"].as_array().unwrap().iter().find(|u| u["server"] == "dotted_srv").unwrap();
    assert_eq!(dotted["config_name"], "dotted.srv");
    let hosted = json["used"].as_array().unwrap().iter().find(|u| u["server"] == "hosted_Docs_srv").unwrap();
    assert!(hosted["config_name"].is_null(), "{hosted}");
}

#[test]
fn mcp_report_ranks_configured_but_unused_servers_by_tool_count() {
    let json = full_json();
    assert_eq!(names(&json, "unused"), ["gamma-srv", "delta-srv", "epsilon-srv"]);
    assert_eq!(json["unused"][0]["tool_count"], 5);
    assert_eq!(json["unused"][0]["last_seen"], "2026-07-01T00:00:00Z");
    assert_eq!(json["unused"][1]["tool_count"], 2);
    assert!(json["unused"][1]["last_seen"].is_null());
    assert!(json["unused"][2]["tool_count"].is_null());
}

#[test]
fn mcp_report_reads_only_the_top_level_server_map() {
    let json = full_json();
    assert_eq!(json["configured"], 6);
    let all: Vec<String> =
        json["configs"][0]["servers"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect();
    assert!(!all.iter().any(|s| s == "retired-srv" || s == "local-srv"), "{all:?}");
}

#[test]
fn mcp_report_states_the_tool_list_it_priced_with() {
    let json = full_json();
    assert_eq!(json["tools_file"]["tools"], 10);
    assert_eq!(json["tools_file"]["servers"], 3);
}

#[test]
fn mcp_report_unions_every_config_given() {
    let mut args = configured_args();
    args.push("--config".into());
    args.push(mcp_fixture("project.mcp.json").display().to_string());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let json = mcp_json(None, &refs);
    assert_eq!(json["configured"], 7);
    assert_eq!(names(&json, "unused"), ["gamma-srv", "delta-srv", "epsilon-srv", "project-srv"]);
}

#[test]
fn mcp_report_defaults_to_the_user_scope_claude_json() {
    let home = TempDir::new().unwrap();
    fs::copy(mcp_fixture("claude.json"), home.path().join(".claude.json")).unwrap();
    let json = mcp_json(Some(home.path()), &["--since", "30d"]);
    assert_eq!(json["configured"], 6);
    assert_eq!(json["configs"][0]["present"], true);
    assert!(json["tools_file"].is_null());
    assert_eq!(names(&json, "unused"), ["delta-srv", "epsilon-srv", "gamma-srv"]);
}

#[test]
fn mcp_report_without_a_default_config_says_so_rather_than_failing() {
    let home = TempDir::new().unwrap();
    let json = mcp_json(Some(home.path()), &["--since", "30d"]);
    assert_eq!(json["configs"][0]["present"], false);
    assert_eq!(json["configured"], 0);
    assert!(json["unused"].as_array().unwrap().is_empty());
}

#[test]
fn mcp_report_fails_on_an_explicit_config_that_is_missing() {
    let out = mcp_report(None, &["--config", "/nonexistent/claude.json"]);
    assert!(!out.status.success());
}

#[test]
fn mcp_report_text_names_the_window_and_the_candidates() {
    let args = configured_args();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = mcp_report(None, &refs);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("since 2026-09-01T00:00:00Z"), "{text}");
    assert!(text.contains("3 malformed"), "{text}");
    assert!(text.contains("configured but unused"), "{text}");
    assert!(text.contains("gamma-srv"), "{text}");
    assert!(text.contains("not configured"), "{text}");
}
