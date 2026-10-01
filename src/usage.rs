//! Skill usage — which skills are actually invoked, measured locally.
//!
//! [`crate::budget`] says how big the skill listing is and refuses to guess
//! which descriptions the platform drops on overflow, because that order is by
//! invocation frequency and frequency was not on disk. This module puts it on
//! disk. A Claude Code hook calls `skill-lint usage record` on every Skill tool
//! call and every prompt; each skill invocation becomes one JSON line in a
//! local log, and `skill-lint usage report` joins that log against the deployed
//! skills. That turns "which skills to keep, merge or retire" from a feeling
//! into a count.
//!
//! # Two ways a skill is invoked
//!
//! - **tool** — the model calls the `Skill` tool. A `PreToolUse` event whose
//!   `tool_name` is `Skill`; the name is `tool_input.skill` (measured from
//!   recorded transcripts: `{"skill":"bidama"}`, with `args` when given).
//!   `command` and `name` are read too, defensively, and the raw `tool_input`
//!   is kept on the line so a future shape change is visible in the data.
//! - **slash** — the operator types `/<name>`. A `UserPromptSubmit` event whose
//!   prompt starts with `/<name>`; recorded only when `<name>` is a deployed
//!   skill (`<skills-dir>/<name>/SKILL.md`), so `/clear` and `/context` are not
//!   counted as skills.
//!
//! # The recorder never touches the session
//!
//! A hook's exit status and stdout reach the agent: exit 2 blocks a tool call
//! or erases a prompt, and a `UserPromptSubmit` hook's stdout is added to the
//! context. So recording is total — every failure, from malformed input to an
//! unwritable log, ends in "wrote nothing", never in an error. [`record`] and
//! [`append`] return `Option` and `io::Result` for the caller to discard.
//!
//! # The log line
//!
//! ```json
//! {"ts":"2026-10-01T14:33:05Z","event":"PreToolUse","skill":"bidama","trigger":"tool",
//!  "session_id":"…","cwd":"/…","transcript_path":"/…/….jsonl","tool_input":{"skill":"bidama"}}
//! ```
//!
//! `transcript_path` and `tool_input` are omitted when absent; `tool_input` is
//! only ever on a `tool` line. One `write` per line on an `O_APPEND` file, so
//! concurrent sessions interleave whole lines.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::budget;

/// Log location under the state directory.
pub const LOG_RELATIVE: &str = "skill-lint/usage.jsonl";

/// Default report window.
pub const DEFAULT_SINCE: &str = "30d";

/// How a skill was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    /// The model called the `Skill` tool.
    Tool,
    /// The operator typed `/<name>`.
    Slash,
}

/// One line of the usage log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    /// RFC 3339, UTC, second precision.
    pub ts: String,
    /// The hook event name (`PreToolUse` / `UserPromptSubmit`).
    pub event: String,
    pub skill: String,
    pub trigger: Trigger,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// The Skill tool's input exactly as the hook received it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
}

// ═══════════════════════════════════════════════════════════════════
// Paths
// ═══════════════════════════════════════════════════════════════════

/// `$XDG_STATE_HOME/skill-lint/usage.jsonl`, else `$HOME/.local/state/…`.
///
/// A relative `XDG_STATE_HOME` is ignored, as the XDG spec requires.
#[must_use]
pub fn default_log_path(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let state = xdg_state_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join(LOG_RELATIVE))
}

/// `$HOME/.claude/skills` — where Claude Code reads user skills from.
#[must_use]
pub fn default_skills_dir(home: Option<OsString>) -> Option<PathBuf> {
    home.map(|h| PathBuf::from(h).join(".claude/skills"))
}

// ═══════════════════════════════════════════════════════════════════
// Time — RFC 3339 without a date crate
// ═══════════════════════════════════════════════════════════════════

/// Days since 1970-01-01 → (year, month, day). Hinnant's `civil_from_days`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// (year, month, day) → days since 1970-01-01. Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ`.
#[must_use]
pub fn format_rfc3339(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

/// RFC 3339 → Unix seconds. Accepts `Z` or a `±HH:MM` offset and drops any
/// fractional seconds; `None` for anything else.
#[must_use]
pub fn parse_rfc3339(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = ts.get(r)?;
        if s.bytes().all(|c| c.is_ascii_digit()) { s.parse().ok() } else { None }
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &ts[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = &frac[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh: i64 = rest[1..3].parse().ok()?;
            let om: i64 = rest[4..6].parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
        _ => return None,
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let days = days_from_civil(year, month as u32, day as u32);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Seconds since the Unix epoch, now.
#[must_use]
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// A report window: `30d`, `12h`, `2w`, or `all` (no lower bound).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    All,
    Seconds(i64),
}

impl std::str::FromStr for Window {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "all" {
            return Ok(Self::All);
        }
        let bad = || format!("window '{s}' is not <N>h, <N>d, <N>w or 'all'");
        let unit = s.chars().last().ok_or_else(bad)?;
        let n: i64 = s[..s.len() - unit.len_utf8()].parse().map_err(|_| bad())?;
        let per = match unit {
            'h' => 3600,
            'd' => 86_400,
            'w' => 7 * 86_400,
            _ => return Err(bad()),
        };
        n.checked_mul(per).filter(|v| *v >= 0).map(Self::Seconds).ok_or_else(bad)
    }
}

// ═══════════════════════════════════════════════════════════════════
// record
// ═══════════════════════════════════════════════════════════════════

/// Where `record` looks things up and what time it is.
pub struct RecordEnv<'a> {
    /// Deployed skills, for validating a typed `/<name>`.
    pub skills_dir: Option<&'a Path>,
    pub now: i64,
}

/// A skill name as typed or passed: one token, no path separators, never `.`
/// or `..`. Plugin skills carry a `plugin:` prefix, so `:` is allowed.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// The skill a `Skill` tool call names: `skill`, else `command`, else `name`;
/// a leading `/` and anything after the first whitespace dropped.
#[must_use]
pub fn skill_from_tool_input(input: &Value) -> Option<String> {
    let raw = ["skill", "command", "name"].iter().find_map(|k| input.get(*k).and_then(Value::as_str))?;
    let name = raw.trim().trim_start_matches('/').split_whitespace().next()?;
    valid_name(name).then(|| name.to_owned())
}

/// The `<name>` of a prompt that starts with `/<name>`. Also reads the
/// `<command-name>/<name></command-name>` form a slash command expands to.
#[must_use]
pub fn slash_name(prompt: &str) -> Option<&str> {
    let p = prompt.trim_start();
    let rest = match p.strip_prefix("<command-name>") {
        Some(tagged) => tagged.split("</command-name>").next()?.trim(),
        None => p,
    };
    let name = rest.strip_prefix('/')?.split_whitespace().next()?;
    valid_name(name).then_some(name)
}

fn str_field(event: &Value, key: &str) -> Option<String> { event.get(key).and_then(Value::as_str).map(str::to_owned) }

/// The usage line one hook event yields, or `None` when it is not a skill
/// invocation (or is not understood — which is the same answer here).
#[must_use]
pub fn record(input: &[u8], env: &RecordEnv<'_>) -> Option<UsageRecord> {
    let event: Value = serde_json::from_slice(input).ok()?;
    let event_name = str_field(&event, "hook_event_name");

    let (skill, trigger, tool_input) = if let Some(tool) = event.get("tool_name").and_then(Value::as_str) {
        // PreToolUse only: wiring PostToolUse too must not double-count.
        if tool != "Skill" || event_name.as_deref().is_some_and(|e| e != "PreToolUse") {
            return None;
        }
        let input = event.get("tool_input")?;
        (skill_from_tool_input(input)?, Trigger::Tool, Some(input.clone()))
    } else {
        if event_name.as_deref().is_some_and(|e| e != "UserPromptSubmit") {
            return None;
        }
        let name = slash_name(event.get("prompt")?.as_str()?)?;
        if !env.skills_dir?.join(name).join("SKILL.md").is_file() {
            return None;
        }
        (name.to_owned(), Trigger::Slash, None)
    };

    let default_event = match trigger {
        Trigger::Tool => "PreToolUse",
        Trigger::Slash => "UserPromptSubmit",
    };
    Some(UsageRecord {
        ts: format_rfc3339(env.now),
        event: event_name.unwrap_or_else(|| default_event.to_owned()),
        skill,
        trigger,
        session_id: str_field(&event, "session_id"),
        cwd: str_field(&event, "cwd"),
        transcript_path: str_field(&event, "transcript_path"),
        tool_input,
    })
}

/// Append one line to `log`, creating its directory. One `write` call on an
/// `O_APPEND` descriptor, so lines from concurrent sessions never interleave.
///
/// # Errors
///
/// Any I/O failure. The hook caller discards it.
pub fn append(log: &Path, line: &UsageRecord) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(line)?;
    buf.push(b'\n');
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    file.write_all(&buf)
}

// ═══════════════════════════════════════════════════════════════════
// report
// ═══════════════════════════════════════════════════════════════════

/// One skill's use inside the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillUse {
    pub skill: String,
    pub total: usize,
    pub slash: usize,
    pub tool: usize,
    pub last_used: String,
    pub deployed: bool,
}

/// A deployed skill with no use inside the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unused {
    pub skill: String,
    /// Last use anywhere in the log, or `None` when never.
    pub last_seen: Option<String>,
}

/// One entry of the drop order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DropEntry {
    pub skill: String,
    pub uses: usize,
    pub last_seen: Option<String>,
    pub listing_chars: usize,
    /// Listing chars freed by dropping this entry and every one before it.
    pub cumulative_chars: usize,
}

/// The whole report.
#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub log: String,
    pub log_present: bool,
    pub skills_dir: String,
    pub now: String,
    pub window: String,
    /// Lower bound of the window; `None` for `all`.
    pub since: Option<String>,
    pub lines_read: usize,
    /// Lines that are not a usage record or carry an unreadable `ts`.
    pub malformed_lines: usize,
    pub events_in_window: usize,
    pub deployed: usize,
    pub used: Vec<SkillUse>,
    pub unused: Vec<Unused>,
    /// Skills invoked in the window that are not deployed under `skills_dir`
    /// (plugin skills, retired ones).
    pub not_deployed: Vec<String>,
    pub listing_chars: usize,
    pub budget_chars: usize,
    /// Deployed skills, least used first — the order the platform drops
    /// descriptions in on overflow.
    pub drop_order: Vec<DropEntry>,
    /// How many of `drop_order` go before the listing fits; 0 within budget.
    pub drops_needed: usize,
}

/// Inputs of [`report`].
pub struct ReportInput<'a> {
    pub log: &'a Path,
    pub skills_dir: &'a Path,
    pub now: i64,
    pub window: Window,
    pub window_label: &'a str,
    pub budget_chars: usize,
    pub max_desc_chars: usize,
}

#[derive(Default)]
struct Tally {
    total: usize,
    slash: usize,
    tool: usize,
    last_used: i64,
}

/// Aggregate the log against the deployed skills.
///
/// # Errors
///
/// A log that exists but cannot be read, or an unreadable skills directory.
/// An ABSENT log is not an error: it is zero events, reported as such.
pub fn report(input: &ReportInput<'_>) -> anyhow::Result<UsageReport> {
    let since = match input.window {
        Window::All => None,
        Window::Seconds(s) => Some(input.now - s),
    };

    let (text, log_present) = match std::fs::read_to_string(input.log) {
        Ok(text) => (text, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", input.log.display()))),
    };

    let mut lines_read = 0;
    let mut malformed_lines = 0;
    let mut events_in_window = 0;
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut last_seen: BTreeMap<String, i64> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        lines_read += 1;
        let Some((rec, ts)) = serde_json::from_str::<UsageRecord>(line)
            .ok()
            .and_then(|r| parse_rfc3339(&r.ts).map(|ts| (r, ts)))
        else {
            malformed_lines += 1;
            continue;
        };
        let seen = last_seen.entry(rec.skill.clone()).or_insert(ts);
        *seen = (*seen).max(ts);
        if since.is_some_and(|s| ts < s) || ts > input.now {
            continue;
        }
        events_in_window += 1;
        let t = tallies.entry(rec.skill).or_default();
        t.total += 1;
        match rec.trigger {
            Trigger::Slash => t.slash += 1,
            Trigger::Tool => t.tool += 1,
        }
        t.last_used = t.last_used.max(ts);
    }

    let listing = budget::compute(&[input.skills_dir.to_path_buf()], input.budget_chars, input.max_desc_chars)?;
    let deployed: BTreeSet<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();

    let mut used: Vec<SkillUse> = tallies
        .iter()
        .map(|(skill, t)| SkillUse {
            skill: skill.clone(),
            total: t.total,
            slash: t.slash,
            tool: t.tool,
            last_used: format_rfc3339(t.last_used),
            deployed: deployed.contains(skill.as_str()),
        })
        .collect();
    used.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.skill.cmp(&b.skill)));

    let not_deployed = used.iter().filter(|u| !u.deployed).map(|u| u.skill.clone()).collect();

    let seen_of = |name: &str| last_seen.get(name).copied();
    let mut order: Vec<(&budget::Entry, usize, Option<i64>)> = listing
        .entries
        .iter()
        .map(|e| (e, tallies.get(&e.name).map_or(0, |t| t.total), seen_of(&e.name)))
        .collect();
    // Least used first; among equals, never-seen before seen-long-ago before
    // seen-recently; then by name, so the order is total and stable.
    order.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.2.cmp(&b.2)).then_with(|| a.0.name.cmp(&b.0.name)));

    let unused = order
        .iter()
        .filter(|(_, uses, _)| *uses == 0)
        .map(|(e, _, seen)| Unused { skill: e.name.clone(), last_seen: seen.map(format_rfc3339) })
        .collect();

    let mut cumulative = 0;
    let drop_order: Vec<DropEntry> = order
        .iter()
        .map(|(e, uses, seen)| {
            cumulative += e.listing_chars;
            DropEntry {
                skill: e.name.clone(),
                uses: *uses,
                last_seen: seen.map(format_rfc3339),
                listing_chars: e.listing_chars,
                cumulative_chars: cumulative,
            }
        })
        .collect();

    let overage = listing.overage_chars();
    let drops_needed =
        if overage == 0 { 0 } else { drop_order.iter().position(|d| d.cumulative_chars >= overage).map_or(drop_order.len(), |i| i + 1) };

    Ok(UsageReport {
        log: input.log.display().to_string(),
        log_present,
        skills_dir: input.skills_dir.display().to_string(),
        now: format_rfc3339(input.now),
        window: input.window_label.to_owned(),
        since: since.map(format_rfc3339),
        lines_read,
        malformed_lines,
        events_in_window,
        deployed: listing.entries.len(),
        used,
        unused,
        not_deployed,
        listing_chars: listing.total_listing_chars,
        budget_chars: listing.budget_chars,
        drop_order,
        drops_needed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rfc3339_round_trips_across_eras_and_leap_days() {
        for secs in [0, 951_782_400, 1_759_329_185, 4_107_542_400, -86_400] {
            assert_eq!(parse_rfc3339(&format_rfc3339(secs)), Some(secs), "{}", format_rfc3339(secs));
        }
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn rfc3339_accepts_offsets_and_fractions_and_refuses_the_rest() {
        let z = parse_rfc3339("2026-10-01T12:00:00Z").unwrap();
        assert_eq!(parse_rfc3339("2026-10-01T09:00:00-03:00"), Some(z));
        assert_eq!(parse_rfc3339("2026-10-01T12:00:00.123456Z"), Some(z));
        for bad in ["yesterday", "2026-10-01", "2026-13-01T00:00:00Z", "2026-10-01T12:00:00", "2026-10-01T12:00:00.Z"] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn window_parses_the_documented_units() {
        assert_eq!("30d".parse(), Ok(Window::Seconds(30 * 86_400)));
        assert_eq!("12h".parse(), Ok(Window::Seconds(12 * 3600)));
        assert_eq!("2w".parse(), Ok(Window::Seconds(14 * 86_400)));
        assert_eq!("all".parse(), Ok(Window::All));
        for bad in ["", "d", "30", "30m", "-1d", "fortnight"] {
            assert!(bad.parse::<Window>().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_skill_field_is_preferred_and_the_variants_are_read() {
        assert_eq!(skill_from_tool_input(&json!({"skill": "bidama", "args": "x"})), Some("bidama".into()));
        assert_eq!(skill_from_tool_input(&json!({"command": "/beta some args"})), Some("beta".into()));
        assert_eq!(skill_from_tool_input(&json!({"name": "gamma"})), Some("gamma".into()));
        assert_eq!(skill_from_tool_input(&json!({"skill": "plugin:docx"})), Some("plugin:docx".into()));
        assert_eq!(skill_from_tool_input(&json!({"skill": "../etc"})), None);
        assert_eq!(skill_from_tool_input(&json!({"skill": ""})), None);
        assert_eq!(skill_from_tool_input(&json!({"other": "x"})), None);
    }

    #[test]
    fn slash_names_are_the_first_token_and_only_at_the_start() {
        assert_eq!(slash_name("/bidama find a parser"), Some("bidama"));
        assert_eq!(slash_name("  /bidama"), Some("bidama"));
        assert_eq!(slash_name("<command-name>/bidama</command-name>\n<command-args>x</command-args>"), Some("bidama"));
        assert_eq!(slash_name("what does /bidama do"), None);
        assert_eq!(slash_name("/"), None);
        assert_eq!(slash_name("/.."), None);
        assert_eq!(slash_name("/a/b"), None);
    }

    #[test]
    fn a_post_tool_use_skill_event_is_not_counted_twice() {
        let env = RecordEnv { skills_dir: None, now: 0 };
        let pre = json!({"hook_event_name": "PreToolUse", "tool_name": "Skill", "tool_input": {"skill": "a"}});
        let post = json!({"hook_event_name": "PostToolUse", "tool_name": "Skill", "tool_input": {"skill": "a"}});
        assert!(record(pre.to_string().as_bytes(), &env).is_some());
        assert!(record(post.to_string().as_bytes(), &env).is_none());
    }

    #[test]
    fn a_slash_prompt_without_a_skills_dir_records_nothing() {
        let env = RecordEnv { skills_dir: None, now: 0 };
        let ev = json!({"hook_event_name": "UserPromptSubmit", "prompt": "/a"});
        assert!(record(ev.to_string().as_bytes(), &env).is_none());
    }

    #[test]
    fn a_relative_xdg_state_home_is_ignored() {
        let got = default_log_path(Some("rel/state".into()), Some("/h".into()));
        assert_eq!(got, Some(PathBuf::from("/h/.local/state/skill-lint/usage.jsonl")));
        let got = default_log_path(Some("/x".into()), Some("/h".into()));
        assert_eq!(got, Some(PathBuf::from("/x/skill-lint/usage.jsonl")));
        assert_eq!(default_log_path(None, None), None);
    }
}
