//! `usage record` and `usage report` end to end.
//!
//! `record` runs as a Claude Code hook on every Skill call and every prompt, so
//! the contract under test is mostly about what it must NOT do: print to stdout
//! (a `UserPromptSubmit` hook's stdout is added to the session), exit non-zero
//! (exit 2 blocks the tool call or erases the prompt), or write a line for an
//! event that is not a skill invocation.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixtures() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/usage") }

fn event(name: &str) -> Vec<u8> { fs::read(fixtures().join("events").join(name)).unwrap() }

/// Run `usage record` with `stdin`, `$HOME` at the fixture home and
/// `$XDG_STATE_HOME` at `state`. Asserts the two invariants every run shares —
/// exit 0, nothing on stdout — and returns the log path.
fn record(state: &Path, stdin: Vec<u8>, extra: &[&str]) -> PathBuf {
    let mut args = vec!["usage", "record"];
    args.extend_from_slice(extra);
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(&args)
        .env("HOME", fixtures().join("home"))
        .env("XDG_STATE_HOME", state)
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "record must always exit 0: {out:?}");
    assert!(out.stdout.is_empty(), "record must never print to stdout: {:?}", String::from_utf8_lossy(&out.stdout));
    state.join("skill-lint/usage.jsonl")
}

fn lines(log: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}

fn is_rfc3339_utc(ts: &str) -> bool {
    let b = ts.as_bytes();
    b.len() == 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z'
        && ts.chars().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19) || c.is_ascii_digit())
}

// ═══════════════════════════════════════════════════════════════════
// record — the two hook shapes
// ═══════════════════════════════════════════════════════════════════

#[test]
fn a_skill_tool_call_is_recorded_with_its_raw_tool_input() {
    let state = TempDir::new().unwrap();
    let log = record(state.path(), event("pre-tool-use-skill.json"), &[]);
    let got = lines(&log);
    assert_eq!(got.len(), 1);
    let line = &got[0];
    assert!(is_rfc3339_utc(line["ts"].as_str().unwrap()), "{line}");
    assert_eq!(line["event"], "PreToolUse");
    assert_eq!(line["skill"], "alpha");
    assert_eq!(line["trigger"], "tool");
    assert_eq!(line["session_id"], "sess-1");
    assert_eq!(line["cwd"], "/home/u/repo");
    assert_eq!(line["transcript_path"], "/home/u/.claude/projects/-home-u-repo/sess-1.jsonl");
    assert_eq!(line["tool_input"]["skill"], "alpha");
    assert_eq!(line["tool_input"]["args"], "find a json parser");
}

#[test]
fn the_command_spelling_of_the_skill_field_is_accepted_and_its_slash_and_args_dropped() {
    let state = TempDir::new().unwrap();
    let log = record(state.path(), event("pre-tool-use-command-variant.json"), &[]);
    let got = lines(&log);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["skill"], "beta");
    assert_eq!(got[0]["trigger"], "tool");
    assert!(got[0].get("transcript_path").is_none(), "absent in the event, so absent in the line: {}", got[0]);
}

#[test]
fn a_typed_slash_command_naming_a_deployed_skill_is_recorded() {
    let state = TempDir::new().unwrap();
    let log = record(state.path(), event("user-prompt-slash.json"), &[]);
    let got = lines(&log);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["event"], "UserPromptSubmit");
    assert_eq!(got[0]["skill"], "gamma");
    assert_eq!(got[0]["trigger"], "slash");
    assert_eq!(got[0]["session_id"], "sess-3");
    assert!(got[0].get("tool_input").is_none(), "{}", got[0]);
}

#[test]
fn records_append_rather_than_overwrite() {
    let state = TempDir::new().unwrap();
    record(state.path(), event("pre-tool-use-skill.json"), &[]);
    let log = record(state.path(), event("user-prompt-slash.json"), &[]);
    let skills: Vec<String> = lines(&log).iter().map(|l| l["skill"].as_str().unwrap().to_owned()).collect();
    assert_eq!(skills, ["alpha", "gamma"]);
}

#[test]
fn without_xdg_state_home_the_log_lands_under_home_local_state() {
    let home = TempDir::new().unwrap();
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["usage", "record"])
        .env("HOME", home.path())
        .env_remove("XDG_STATE_HOME")
        .write_stdin(event("pre-tool-use-skill.json"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert_eq!(lines(&home.path().join(".local/state/skill-lint/usage.jsonl")).len(), 1);
}

// ═══════════════════════════════════════════════════════════════════
// record — everything that must write NOTHING and still exit 0
// ═══════════════════════════════════════════════════════════════════

fn records_nothing(stdin: Vec<u8>, extra: &[&str]) {
    let state = TempDir::new().unwrap();
    let log = record(state.path(), stdin, extra);
    assert!(!log.exists(), "expected no write, found {:?}", fs::read_to_string(&log));
}

#[test]
fn an_unknown_slash_name_is_not_a_skill_invocation() { records_nothing(event("user-prompt-unknown-slash.json"), &[]); }

#[test]
fn a_prompt_that_merely_mentions_a_skill_is_not_an_invocation() { records_nothing(event("user-prompt-plain.json"), &[]); }

#[test]
fn a_slash_name_that_walks_out_of_the_skills_dir_is_refused() { records_nothing(event("user-prompt-traversal.json"), &[]); }

#[test]
fn a_non_skill_tool_call_is_ignored_even_when_its_input_looks_like_one() {
    records_nothing(event("pre-tool-use-bash.json"), &[]);
}

#[test]
fn malformed_json_writes_nothing() { records_nothing(event("malformed.json"), &[]); }

#[test]
fn empty_stdin_writes_nothing() { records_nothing(Vec::new(), &[]); }

#[test]
fn an_unknown_flag_exits_0_not_clap_s_2() {
    // Exit 2 from a PreToolUse hook BLOCKS the tool call and from a
    // UserPromptSubmit hook erases the prompt. clap exits 2 on a bad flag, so a
    // module that passes a flag an older binary lacks would otherwise block
    // every Skill call in every session.
    records_nothing(event("pre-tool-use-skill.json"), &["--no-such-flag"]);
}

#[test]
fn an_unwritable_log_location_fails_silently() {
    let state = TempDir::new().unwrap();
    // A FILE where the `skill-lint` directory must go: create_dir_all fails.
    fs::write(state.path().join("skill-lint"), "in the way").unwrap();
    let log = record(state.path(), event("pre-tool-use-skill.json"), &[]);
    assert!(!log.exists());
}

#[test]
fn no_home_and_no_xdg_state_home_fails_silently() {
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["usage", "record"])
        .env_remove("HOME")
        .env_remove("XDG_STATE_HOME")
        .write_stdin(event("pre-tool-use-skill.json"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

// ═══════════════════════════════════════════════════════════════════
// report
// ═══════════════════════════════════════════════════════════════════

fn report(extra: &[&str]) -> std::process::Output {
    let skills = fixtures().join("home/.claude/skills");
    let log = fixtures().join("usage.jsonl");
    let mut args: Vec<String> = [
        "usage",
        "report",
        "--log",
        log.to_str().unwrap(),
        "--skills-dir",
        skills.to_str().unwrap(),
        "--now",
        "2026-10-01T00:00:00Z",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    let out = Command::cargo_bin("skill-lint").unwrap().args(&args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out
}

fn report_json(extra: &[&str]) -> serde_json::Value {
    let mut extra = extra.to_vec();
    extra.push("--json");
    serde_json::from_slice(&report(&extra).stdout).unwrap()
}

#[test]
fn report_counts_per_skill_inside_the_window() {
    let json = report_json(&["--since", "30d"]);
    assert_eq!(json["since"], "2026-09-01T00:00:00Z");
    assert_eq!(json["events_in_window"], 5);
    assert_eq!(json["malformed_lines"], 2);

    let used = json["used"].as_array().unwrap();
    let names: Vec<&str> = used.iter().map(|u| u["skill"].as_str().unwrap()).collect();
    assert_eq!(names, ["alpha", "beta", "plugin:docx"], "most used first, then by name");

    assert_eq!(used[0]["total"], 3);
    assert_eq!(used[0]["slash"], 1);
    assert_eq!(used[0]["tool"], 2);
    assert_eq!(used[0]["last_used"], "2026-09-30T10:00:00Z");
    assert_eq!(used[0]["deployed"], true);
    assert_eq!(used[2]["deployed"], false);
}

#[test]
fn report_names_deployed_skills_unused_in_the_window() {
    let json = report_json(&["--since", "30d"]);
    let never: Vec<&str> = json["unused"].as_array().unwrap().iter().map(|u| u["skill"].as_str().unwrap()).collect();
    // gamma WAS used — in July, outside the window — and keeps that date.
    assert_eq!(never, ["delta", "gamma"]);
    assert_eq!(json["unused"][1]["last_seen"], "2026-07-01T00:00:00Z");
    assert!(json["unused"][0]["last_seen"].is_null());
    let undeployed: Vec<&str> =
        json["not_deployed"].as_array().unwrap().iter().map(|u| u.as_str().unwrap()).collect();
    assert_eq!(undeployed, ["plugin:docx"]);
}

#[test]
fn report_drop_order_is_least_used_first_never_seen_before_seen_long_ago() {
    let json = report_json(&["--since", "30d"]);
    let order: Vec<&str> =
        json["drop_order"].as_array().unwrap().iter().map(|d| d["skill"].as_str().unwrap()).collect();
    assert_eq!(order, ["delta", "gamma", "beta", "alpha"]);
    let cumulative: Vec<u64> =
        json["drop_order"].as_array().unwrap().iter().map(|d| d["cumulative_chars"].as_u64().unwrap()).collect();
    assert!(cumulative.windows(2).all(|w| w[0] < w[1]), "{cumulative:?}");
    assert_eq!(*cumulative.last().unwrap(), json["listing_chars"].as_u64().unwrap());
}

#[test]
fn report_marks_how_far_down_the_drop_order_an_overage_reaches() {
    // A budget smaller than the whole listing: the platform drops the least
    // used first, and `drops_needed` says how many entries that takes.
    let json = report_json(&["--since", "30d", "--budget-chars", "100"]);
    let listing = json["listing_chars"].as_u64().unwrap();
    assert!(listing > 100);
    let needed = usize::try_from(json["drops_needed"].as_u64().unwrap()).unwrap();
    let order = json["drop_order"].as_array().unwrap();
    assert!(order[needed - 1]["cumulative_chars"].as_u64().unwrap() >= listing - 100);
    if needed > 1 {
        assert!(order[needed - 2]["cumulative_chars"].as_u64().unwrap() < listing - 100);
    }
}

#[test]
fn report_since_all_counts_every_well_formed_line() {
    let json = report_json(&["--since", "all"]);
    assert_eq!(json["events_in_window"], 6);
    assert!(json["since"].is_null());
    assert_eq!(json["unused"].as_array().unwrap().len(), 1);
}

#[test]
fn report_text_states_the_window_and_the_candidates() {
    let out = report(&["--since", "30d"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("since 2026-09-01T00:00:00Z"), "{text}");
    assert!(text.contains("2 malformed"), "{text}");
    assert!(text.contains("delta"), "{text}");
    assert!(text.contains("retire/merge candidates"), "{text}");
}

#[test]
fn report_on_a_missing_log_reports_zero_events_rather_than_failing() {
    let state = TempDir::new().unwrap();
    let skills = fixtures().join("home/.claude/skills");
    let out = Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["usage", "report", "--json", "--log"])
        .arg(state.path().join("absent.jsonl"))
        .arg("--skills-dir")
        .arg(&skills)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["events_in_window"], 0);
    assert_eq!(json["log_present"], false);
    assert_eq!(json["unused"].as_array().unwrap().len(), 4);
}

#[test]
fn report_rejects_a_window_it_cannot_parse() {
    Command::cargo_bin("skill-lint")
        .unwrap()
        .args(["usage", "report", "--since", "fortnight", "--log", "/nonexistent"])
        .assert()
        .failure();
}
