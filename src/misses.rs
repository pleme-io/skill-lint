use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::route::STRONG_PERMILLE;
use crate::usage::{self, LogLine, PromptRecord, Window};
use crate::whynot::LEXICAL_GAP_PERMILLE;

pub const UNEXPLAINED: &str = "unexplained";

#[derive(Debug, Clone, Serialize)]
pub struct MissedPrompt {
    pub id: String,
    pub permille: u32,
    pub rank: usize,
    pub top_permille: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillMisses {
    pub skill: String,
    pub strong_prompts: usize,
    pub misses: usize,
    pub miss_rate_permille: u32,
    pub dominant_class: String,
    pub classes: BTreeMap<String, usize>,
    pub prompts: Vec<MissedPrompt>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissReport {
    pub log: String,
    pub log_present: bool,
    pub window: String,
    pub strong_permille: u32,
    pub prompts_in_window: usize,
    pub prompts_with_strong_candidate: usize,
    pub missed_prompts: usize,
    pub skills: Vec<SkillMisses>,
}

pub struct MissInput<'a> {
    pub log: &'a Path,
    pub now: i64,
    pub window: Window,
    pub window_label: &'a str,
}

struct Turn {
    prompt: PromptRecord,
    ts: i64,
    fired: bool,
}

fn session_key(p: &PromptRecord) -> String { p.session_id.clone().unwrap_or_default() }

/// Join prompt lines to the skill events that follow them in the same session.
///
/// # Errors
///
/// A log that exists but cannot be read.
pub fn misses(input: &MissInput<'_>, structural_class: &mut dyn FnMut(&str) -> Option<String>) -> anyhow::Result<MissReport> {
    let (text, present) = usage::read_log(input.log)?;
    let since = input.window.lower_bound(input.now);
    let mut turns: Vec<Turn> = Vec::new();
    let mut open: BTreeMap<String, usize> = BTreeMap::new();

    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<LogLine>(line) {
            Ok(LogLine::Prompt(p)) => {
                let Some(ts) = usage::parse_rfc3339(&p.ts) else { continue };
                open.insert(session_key(&p), turns.len());
                turns.push(Turn { prompt: p, ts, fired: false });
            }
            Ok(LogLine::Skill(rec)) => {
                if let Some(i) = open.get(&rec.origin.session_id.clone().unwrap_or_default()) {
                    turns[*i].fired = true;
                }
            }
            _ => {}
        }
    }

    let in_window: Vec<&Turn> = turns.iter().filter(|t| !since.is_some_and(|s| t.ts < s) && t.ts <= input.now).collect();
    let mut by_skill: BTreeMap<String, SkillMisses> = BTreeMap::new();
    let mut strong_prompts = 0;
    let mut missed_prompts = 0;
    let mut cache: BTreeMap<String, Option<String>> = BTreeMap::new();

    for t in &in_window {
        let strong: Vec<(usize, &usage::Candidate)> =
            t.prompt.candidates.iter().enumerate().filter(|(_, c)| c.permille >= STRONG_PERMILLE).collect();
        if strong.is_empty() {
            continue;
        }
        strong_prompts += 1;
        if !t.fired {
            missed_prompts += 1;
        }
        let top = t.prompt.candidates.first().map_or(0, |c| c.permille);
        for (idx, c) in strong {
            let e = by_skill.entry(c.skill.clone()).or_insert_with(|| SkillMisses {
                skill: c.skill.clone(),
                strong_prompts: 0,
                misses: 0,
                miss_rate_permille: 0,
                dominant_class: UNEXPLAINED.to_owned(),
                classes: BTreeMap::new(),
                prompts: Vec::new(),
            });
            e.strong_prompts += 1;
            if t.fired {
                continue;
            }
            e.misses += 1;
            let structural = cache.entry(c.skill.clone()).or_insert_with(|| structural_class(&c.skill)).clone();
            let class = structural.unwrap_or_else(|| {
                if idx > 0 && top > c.permille {
                    "outranked".to_owned()
                } else if c.permille < LEXICAL_GAP_PERMILLE {
                    "lexical_gap".to_owned()
                } else {
                    UNEXPLAINED.to_owned()
                }
            });
            *e.classes.entry(class).or_insert(0) += 1;
            e.prompts.push(MissedPrompt { id: t.prompt.id.clone(), permille: c.permille, rank: idx + 1, top_permille: top });
        }
    }

    let mut skills: Vec<SkillMisses> = by_skill.into_values().filter(|s| s.misses > 0).collect();
    for s in &mut skills {
        s.miss_rate_permille = u32::try_from(s.misses * 1000 / s.strong_prompts).unwrap_or(1000);
        s.dominant_class = s
            .classes
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map_or_else(|| UNEXPLAINED.to_owned(), |(k, _)| k.clone());
    }
    skills.sort_by(|a, b| b.misses.cmp(&a.misses).then(b.miss_rate_permille.cmp(&a.miss_rate_permille)).then_with(|| a.skill.cmp(&b.skill)));

    Ok(MissReport {
        log: input.log.display().to_string(),
        log_present: present,
        window: input.window_label.to_owned(),
        strong_permille: STRONG_PERMILLE,
        prompts_in_window: in_window.len(),
        prompts_with_strong_candidate: strong_prompts,
        missed_prompts,
        skills,
    })
}

#[must_use]
pub fn render(r: &MissReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "usage misses ({}): log {}{}", r.window, r.log, if r.log_present { "" } else { " (absent)" });
    let _ = writeln!(
        out,
        "  {} prompts, {} with a strong candidate (>= {}/1000), {} with no skill fired",
        r.prompts_in_window, r.prompts_with_strong_candidate, r.strong_permille, r.missed_prompts
    );
    if r.skills.is_empty() {
        let _ = writeln!(out, "  no skill missed in the window");
    }
    for s in &r.skills {
        let _ = writeln!(
            out,
            "\n  {}: {} of {} strong prompts missed ({}/1000), dominant reason {}",
            s.skill, s.misses, s.strong_prompts, s.miss_rate_permille, s.dominant_class
        );
        let classes: Vec<String> = s.classes.iter().map(|(k, v)| format!("{k} x{v}")).collect();
        let _ = writeln!(out, "    reasons: {}", classes.join(", "));
        for p in &s.prompts {
            let _ = writeln!(out, "    prompt {} score {}/1000 rank {} (top {}/1000)", p.id, p.permille, p.rank, p.top_permille);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn prompt(ts: &str, sess: &str, id: &str, cands: &[(&str, u32)]) -> String {
        let c: Vec<String> = cands.iter().map(|(s, p)| format!(r#"{{"skill":"{s}","permille":{p}}}"#)).collect();
        format!(r#"{{"ts":"{ts}","event":"UserPromptSubmit","id":"{id}","session_id":"{sess}","candidates":[{}]}}"#, c.join(","))
    }

    fn fired(ts: &str, sess: &str, skill: &str) -> String {
        format!(r#"{{"ts":"{ts}","event":"PreToolUse","skill":"{skill}","trigger":"tool","session_id":"{sess}","cwd":null}}"#)
    }

    fn run(lines: &[String], classify: &mut dyn FnMut(&str) -> Option<String>) -> MissReport {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("u.jsonl");
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();
        let now = usage::parse_rfc3339("2026-10-21T00:00:00Z").unwrap();
        misses(&MissInput { log: &log, now, window: Window::Seconds(30 * 86_400), window_label: "30d" }, classify).unwrap()
    }

    #[test]
    fn a_skill_event_after_a_prompt_in_the_same_session_is_a_hit() {
        let r = run(
            &[
                prompt("2026-10-20T10:00:00Z", "s1", "p1", &[("a", 800)]),
                fired("2026-10-20T10:00:05Z", "s1", "a"),
                prompt("2026-10-20T11:00:00Z", "s1", "p2", &[("a", 800)]),
            ],
            &mut |_| None,
        );
        assert_eq!(r.prompts_with_strong_candidate, 2);
        assert_eq!(r.missed_prompts, 1);
        assert_eq!(r.skills[0].misses, 1);
        assert_eq!(r.skills[0].strong_prompts, 2);
        assert_eq!(r.skills[0].miss_rate_permille, 500);
        assert_eq!(r.skills[0].prompts[0].id, "p2");
    }

    #[test]
    fn a_skill_event_in_another_session_does_not_count() {
        let r = run(&[prompt("2026-10-20T10:00:00Z", "s1", "p1", &[("a", 800)]), fired("2026-10-20T10:00:05Z", "s2", "a")], &mut |_| None);
        assert_eq!(r.missed_prompts, 1);
    }

    #[test]
    fn a_skill_event_after_the_next_prompt_belongs_to_the_next_prompt() {
        let r = run(
            &[
                prompt("2026-10-20T10:00:00Z", "s1", "p1", &[("a", 800)]),
                prompt("2026-10-20T10:01:00Z", "s1", "p2", &[("a", 800)]),
                fired("2026-10-20T10:01:05Z", "s1", "a"),
            ],
            &mut |_| None,
        );
        assert_eq!(r.skills[0].prompts.len(), 1);
        assert_eq!(r.skills[0].prompts[0].id, "p1");
    }

    #[test]
    fn a_weak_candidate_is_not_a_miss() {
        let r = run(&[prompt("2026-10-20T10:00:00Z", "s1", "p1", &[("a", STRONG_PERMILLE - 1)])], &mut |_| None);
        assert_eq!(r.prompts_with_strong_candidate, 0);
        assert!(r.skills.is_empty());
    }

    #[test]
    fn a_prompt_outside_the_window_is_ignored() {
        let r = run(&[prompt("2026-08-01T10:00:00Z", "s1", "p1", &[("a", 900)])], &mut |_| None);
        assert_eq!(r.prompts_in_window, 0);
    }

    #[test]
    fn the_class_is_structural_first_then_outranked_then_gap_then_unexplained() {
        let r = run(
            &[
                prompt("2026-10-20T10:00:00Z", "s1", "p1", &[("top", 900), ("second", 700)]),
                prompt("2026-10-20T10:10:00Z", "s2", "p2", &[("weak", 450)]),
                prompt("2026-10-20T10:20:00Z", "s3", "p3", &[("sure", 900)]),
                prompt("2026-10-20T10:30:00Z", "s4", "p4", &[("gone", 900)]),
            ],
            &mut |s| (s == "gone").then(|| "not_deployed".to_owned()),
        );
        let class = |n: &str| r.skills.iter().find(|s| s.skill == n).unwrap().dominant_class.clone();
        assert_eq!(class("second"), "outranked");
        assert_eq!(class("weak"), "lexical_gap");
        assert_eq!(class("sure"), UNEXPLAINED);
        assert_eq!(class("gone"), "not_deployed");
        assert_eq!(class("top"), UNEXPLAINED);
    }

    #[test]
    fn skills_sort_by_misses_and_the_dominant_class_is_the_most_frequent() {
        let r = run(
            &[
                prompt("2026-10-20T10:00:00Z", "s1", "p1", &[("a", 900)]),
                prompt("2026-10-20T10:10:00Z", "s2", "p2", &[("a", 900)]),
                prompt("2026-10-20T10:20:00Z", "s3", "p3", &[("b", 900)]),
            ],
            &mut |s| Some(if s == "a" { "dropped_by_budget" } else { "not_deployed" }.to_owned()),
        );
        assert_eq!(r.skills[0].skill, "a");
        assert_eq!(r.skills[0].dominant_class, "dropped_by_budget");
        assert_eq!(r.skills[0].classes["dropped_by_budget"], 2);
    }

    #[test]
    fn an_absent_log_is_zero_prompts_not_an_error() {
        let dir = TempDir::new().unwrap();
        let r = misses(
            &MissInput { log: &dir.path().join("none"), now: 0, window: Window::All, window_label: "all" },
            &mut |_| None,
        )
        .unwrap();
        assert!(!r.log_present);
        assert_eq!(r.prompts_in_window, 0);
    }

    #[test]
    fn the_report_never_carries_prompt_text_only_ids_and_scores() {
        let r = run(&[prompt("2026-10-20T10:00:00Z", "s1", "p1", &[("a", 800)])], &mut |_| None);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"id\":\"p1\"") && json.contains("\"permille\":800"));
    }
}
