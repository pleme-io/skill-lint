use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;

use serde::Serialize;

use crate::error::{CheckKind, LintError};
use crate::ratchet::{Ratchet, Verdict, parse_lines};

pub const DEFAULT_BANK_HEADROOM: usize = 2048;

const WHY: &str = "why:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Session,
    File,
}

impl Scope {
    pub const ALL: [Self; 2] = [Self::Session, Self::File];

    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::File => "file",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.kind()) }
}

#[derive(Debug, Clone, Default)]
pub struct Ledger {
    sessions: Ratchet,
    files: Ratchet,
    whys: BTreeMap<String, String>,
}

impl Ledger {
    #[must_use]
    pub fn parse(text: &str) -> (Self, Vec<LintError>) {
        let mut ledger = Self::default();
        for (kind, key, size) in parse_lines(text) {
            match kind.as_str() {
                k if k == Scope::Session.kind() => ledger.sessions.insert(key, size),
                k if k == Scope::File.kind() => ledger.files.insert(key, size),
                _ => {}
            }
        }
        let mut errors = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let Some(rest) = raw.trim().strip_prefix(WHY) else { continue };
            let rest = rest.trim();
            let (key, reason) = rest.split_once(" =").map_or((rest, ""), |(k, r)| (k.trim(), r.trim()));
            let line = index + 1;
            if reason.is_empty() {
                errors.push(LintError::ChainWhyEmpty { kind: CheckKind::ClaudeMdChain, line, key: key.to_owned() });
            } else if ledger.row_scope(key).is_none() {
                errors.push(LintError::ChainWhyDangling { kind: CheckKind::ClaudeMdChain, line, key: key.to_owned() });
            } else {
                ledger.whys.insert(key.to_owned(), reason.to_owned());
            }
        }
        (ledger, errors)
    }

    #[must_use]
    pub const fn ratchet(&self, scope: Scope) -> &Ratchet {
        match scope {
            Scope::Session => &self.sessions,
            Scope::File => &self.files,
        }
    }

    const fn ratchet_mut(&mut self, scope: Scope) -> &mut Ratchet {
        match scope {
            Scope::Session => &mut self.sessions,
            Scope::File => &mut self.files,
        }
    }

    #[must_use]
    pub fn recorded(&self, scope: Scope, key: &str) -> Option<usize> { self.ratchet(scope).recorded(key) }

    #[must_use]
    pub fn why(&self, key: &str) -> Option<&str> { self.whys.get(key).map(String::as_str) }

    #[must_use]
    pub fn len(&self) -> usize { self.sessions.len() + self.files.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    fn row_scope(&self, key: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|scope| self.recorded(*scope, key).is_some())
    }

    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::from(
            "# skill-lint chain ledger: the bytes each session loads, and each file over the per-file cap.\n\
             # A row only goes down, by --write-baseline. A raise is a hand edit with `why: <key> = <receipt>`.\n",
        );
        for scope in Scope::ALL {
            for key in self.ratchet(scope).keys() {
                let size = self.recorded(scope, key).unwrap_or_default();
                let _ = writeln!(text, "{scope}: {key} {size}");
            }
        }
        for (key, reason) in &self.whys {
            let _ = writeln!(text, "{WHY} {key} = {reason}");
        }
        text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Judgement {
    Unrecorded,
    Held,
    Shrunk { recorded: usize, freed: usize },
    Unbanked { recorded: usize, freed: usize },
    Grew { recorded: usize, grew: usize },
}

impl Judgement {
    #[must_use]
    pub fn of(ratchet: &Ratchet, key: &str, measured: usize, headroom: usize) -> Self {
        match ratchet.judge(key, measured) {
            Verdict::Unrecorded => Self::Unrecorded,
            Verdict::Grew { recorded, grew } => Self::Grew { recorded, grew },
            Verdict::Held => {
                let recorded = ratchet.recorded(key).unwrap_or(measured);
                match recorded - measured {
                    0 => Self::Held,
                    freed if freed <= headroom => Self::Shrunk { recorded, freed },
                    freed => Self::Unbanked { recorded, freed },
                }
            }
        }
    }

    #[must_use]
    pub const fn is_red(self) -> bool { matches!(self, Self::Unrecorded | Self::Unbanked { .. } | Self::Grew { .. }) }

    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Unrecorded => "UNRECORDED",
            Self::Held => "held",
            Self::Shrunk { .. } => "shrunk",
            Self::Unbanked { .. } => "UNBANKED",
            Self::Grew { .. } => "GREW",
        }
    }

    fn error(self, scope: Scope, key: &str, bytes: usize, headroom: usize) -> Option<LintError> {
        let kind = CheckKind::ClaudeMdChain;
        let scope = scope.to_string();
        let key = key.to_owned();
        match self {
            Self::Held | Self::Shrunk { .. } => None,
            Self::Unrecorded => Some(LintError::ChainUnrecorded { kind, scope, key, bytes }),
            Self::Grew { recorded, grew } => Some(LintError::ChainGrew { kind, scope, key, bytes, recorded, grew }),
            Self::Unbanked { recorded, freed } => {
                Some(LintError::ChainUnbanked { kind, scope, key, bytes, recorded, freed, headroom })
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Measured {
    pub sessions: BTreeMap<String, usize>,
    pub files: BTreeMap<String, usize>,
}

impl Measured {
    #[must_use]
    pub const fn of(&self, scope: Scope) -> &BTreeMap<String, usize> {
        match scope {
            Scope::Session => &self.sessions,
            Scope::File => &self.files,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub scope: Scope,
    pub key: String,
    pub bytes: usize,
    #[serde(flatten)]
    pub judgement: Judgement,
}

#[derive(Debug, Clone, Serialize)]
pub struct Orphan {
    pub scope: Scope,
    pub key: String,
    pub recorded: usize,
}

#[derive(Debug, Default)]
pub struct LedgerReport {
    pub rows: Vec<Row>,
    pub orphans: Vec<Orphan>,
    pub errors: Vec<LintError>,
}

impl LedgerReport {
    #[must_use]
    pub fn is_ok(&self) -> bool { self.errors.is_empty() }
}

#[must_use]
pub fn judge(ledger: &Ledger, measured: &Measured, headroom: usize) -> LedgerReport {
    let mut report = LedgerReport::default();
    for scope in Scope::ALL {
        let ratchet = ledger.ratchet(scope);
        for (key, &bytes) in measured.of(scope) {
            let judgement = Judgement::of(ratchet, key, bytes, headroom);
            report.errors.extend(judgement.error(scope, key, bytes, headroom));
            report.rows.push(Row { scope, key: key.clone(), bytes, judgement });
        }
        let live: BTreeSet<String> = measured.of(scope).keys().cloned().collect();
        for key in ratchet.orphans(&live) {
            report.orphans.push(Orphan { scope, key: key.to_owned(), recorded: ratchet.recorded(key).unwrap_or_default() });
        }
    }
    report
}

#[must_use]
pub fn raised_without_why(previous: &Ledger, current: &Ledger) -> Vec<LintError> {
    let mut errors = Vec::new();
    for scope in Scope::ALL {
        for key in current.ratchet(scope).keys() {
            let (Some(before), Some(now)) = (previous.recorded(scope, key), current.recorded(scope, key)) else { continue };
            if now > before && current.why(key).is_none() {
                errors.push(LintError::ChainRaisedWithoutWhy {
                    kind: CheckKind::ClaudeMdChain,
                    scope: scope.to_string(),
                    key: key.to_owned(),
                    previous: before,
                    recorded: now,
                });
            }
        }
    }
    errors
}

#[must_use]
pub fn tighten(ledger: &Ledger, measured: &Measured, file_cap: Option<usize>) -> Ledger {
    let mut out = Ledger::default();
    for scope in Scope::ALL {
        for (key, &bytes) in measured.of(scope) {
            let recorded = ledger.recorded(scope, key);
            if scope == Scope::File && recorded.is_some() && file_cap.is_some_and(|cap| bytes <= cap) {
                continue;
            }
            let value = recorded.map_or(bytes, |r| r.min(bytes));
            out.ratchet_mut(scope).insert(key.clone(), value);
            if recorded == Some(value)
                && let Some(why) = ledger.why(key)
            {
                out.whys.insert(key.clone(), why.to_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(sessions: &[(&str, usize)], files: &[(&str, usize)]) -> Measured {
        Measured {
            sessions: sessions.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            files: files.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
        }
    }

    fn ledger(text: &str) -> Ledger {
        let (ledger, errors) = Ledger::parse(text);
        assert!(errors.is_empty(), "{errors:?}");
        ledger
    }

    fn verdict(text: &str, key: &str, bytes: usize) -> Judgement {
        let report = judge(&ledger(text), &measured(&[(key, bytes)], &[]), DEFAULT_BANK_HEADROOM);
        report.rows[0].judgement
    }

    #[test]
    fn a_scope_with_no_row_is_red() {
        assert_eq!(verdict("", "~/r", 10), Judgement::Unrecorded);
        assert!(Judgement::Unrecorded.is_red());
    }

    #[test]
    fn growth_by_one_byte_is_red() {
        let j = verdict("session: ~/r 100\n", "~/r", 101);
        assert_eq!(j, Judgement::Grew { recorded: 100, grew: 1 });
        assert!(j.is_red());
    }

    #[test]
    fn an_equal_measurement_holds() {
        assert_eq!(verdict("session: ~/r 100\n", "~/r", 100), Judgement::Held);
    }

    #[test]
    fn a_shrink_within_the_headroom_passes_and_is_reported() {
        let j = verdict("session: ~/r 5000\n", "~/r", 5000 - DEFAULT_BANK_HEADROOM);
        assert_eq!(j, Judgement::Shrunk { recorded: 5000, freed: DEFAULT_BANK_HEADROOM });
        assert!(!j.is_red());
    }

    #[test]
    fn a_shrink_past_the_headroom_left_unbanked_is_red() {
        let j = verdict("session: ~/r 5000\n", "~/r", 5000 - DEFAULT_BANK_HEADROOM - 1);
        assert_eq!(j, Judgement::Unbanked { recorded: 5000, freed: DEFAULT_BANK_HEADROOM + 1 });
        assert!(j.is_red());
    }

    #[test]
    fn a_row_whose_scope_is_gone_is_an_advisory_orphan() {
        let report = judge(&ledger("session: ~/gone 9\nfile: ~/gone/CLAUDE.md 30000\n"), &measured(&[], &[]), 0);
        assert_eq!(report.orphans.len(), 2);
        assert!(report.is_ok());
    }

    #[test]
    fn a_why_without_a_receipt_or_a_row_is_refused() {
        let (_, errors) = Ledger::parse("session: ~/r 1\nwhy: ~/r =\nwhy: ~/r\nwhy: ~/nope = platform budget\n");
        assert!(matches!(
            errors.as_slice(),
            [LintError::ChainWhyEmpty { line: 2, .. }, LintError::ChainWhyEmpty { line: 3, .. }, LintError::ChainWhyDangling { line: 4, .. }]
        ));
    }

    #[test]
    fn a_raise_needs_a_why() {
        let before = ledger("session: ~/r 100\n");
        assert_eq!(raised_without_why(&before, &ledger("session: ~/r 101\n")).len(), 1);
        assert!(raised_without_why(&before, &ledger("session: ~/r 101\nwhy: ~/r = merlin A/B 2026-10-22\n")).is_empty());
        assert!(raised_without_why(&before, &ledger("session: ~/r 90\n")).is_empty());
    }

    #[test]
    fn tighten_only_lowers_records_new_and_drops_paid_debt() {
        let old = ledger("session: ~/a 100\nsession: ~/b 100\nsession: ~/gone 5\nfile: ~/f 30000\nwhy: ~/b = raised for the 1M listing\n");
        let now = measured(&[("~/a", 80), ("~/b", 120), ("~/new", 7)], &[("~/f", 20000)]);
        let out = tighten(&old, &now, Some(25000));
        assert_eq!(out.recorded(Scope::Session, "~/a"), Some(80));
        assert_eq!(out.recorded(Scope::Session, "~/b"), Some(100));
        assert_eq!(out.recorded(Scope::Session, "~/new"), Some(7));
        assert_eq!(out.recorded(Scope::Session, "~/gone"), None);
        assert_eq!(out.recorded(Scope::File, "~/f"), None);
        assert_eq!(out.why("~/b"), Some("raised for the 1M listing"));
        let (reparsed, errors) = Ledger::parse(&out.render());
        assert!(errors.is_empty());
        assert_eq!(reparsed.render(), out.render());
    }
}
