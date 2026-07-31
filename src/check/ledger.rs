//! The tier-honest ledger — is every mitigation's ceiling actually named?
//!
//! Doctrine says a seal is graded at its true tier and never rounded up, and
//! that `only-mitigated` is the tier that must say *why* it cannot be higher.
//! In Rust that constraint already exists and is airtight: `selo`'s
//! `SealTier::OnlyMitigated(Ceiling)` is a TUPLE variant, so an unnamed-ceiling
//! mitigation has no constructor.
//!
//! The ledgers that actually get written are markdown tables in skill bodies,
//! and **a Rust type cannot gate a document nothing typechecks against**. `selo`
//! has zero consumers fleet-wide (no `Cargo.toml` in the org depends on it), so
//! making `SealTier` the "row type" of a hand-written table would be ceremony:
//! the table would still be prose, and the type still unreferenced by it.
//!
//! What closes the gap instead is to consume `selo` as a **VOCABULARY** — the
//! exact label set its `SealTier::label()` and `Ceiling::code()` render — and
//! enforce that vocabulary where the rows really live. The dependency runs one
//! way and costs nothing: this crate does not link `selo`, it agrees with it.
//! [`TIERS`] and [`CEILINGS`] below are that agreement, written out, with a test
//! pinning them to the strings `selo` emits.
//!
//! # A ledger declares itself
//!
//! The table is found by an explicit `<!-- tier-ledger -->` marker, never by
//! guessing from headers. Measured on this corpus: seven skill bodies contain a
//! table with a `tier` column and only one of them is a seal-tier ledger — the
//! rest grade milestones (`M0`/`M1`), abstraction layers, or recon results
//! (`SHIPPED primitive` / `NET-NEW` / `pattern`). A header-shape heuristic would
//! have failed six times out of seven.
//!
//! Declaring the table also makes DELETING it visible: a skill named by
//! `--require-tier-ledger` must carry one, so "drop the table to go green" is a
//! red gate rather than a silent pass.

use std::collections::BTreeSet;

use crate::error::{CheckKind, LintError};

use super::links::body_of;
use super::{CheckContext, Checker};

/// The marker that declares the next table a tier ledger.
pub const LEDGER_MARKER: &str = "<!-- tier-ledger -->";

/// The tier labels, exactly as `selo::SealTier::label()` renders them.
pub const TIERS: &[&str] = &["truly-unrep", "parse-time-rejected", "only-mitigated"];

/// The ceiling codes, exactly as `selo::Ceiling::code()` renders them.
pub const CEILINGS: &[&str] = &["C1", "C2", "C3", "C4", "C5", "C6"];

/// The tier label that is required to carry a ceiling.
const MITIGATED: &str = "only-mitigated";

/// How many lines a table may sit below its marker before the marker is
/// considered to point at nothing. Blank lines and a lead-in sentence are
/// normal; a whole paragraph means the marker drifted off its table.
const MARKER_REACH: usize = 4;

/// One parsed ledger row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    /// The row's first cell — what is being graded.
    pub subject: String,
    /// The row's last cell — the tier claim, verbatim.
    pub tier: String,
}

/// A tier cell's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierVerdict {
    /// A well-formed tier claim.
    Valid,
    /// `only-mitigated` with no `(Cn)` ceiling — the defect this check exists
    /// for. The Rust type makes this unconstructible; prose does not.
    MitigationWithoutCeiling,
    /// Not one of [`TIERS`] at all.
    OutsideVocabulary,
}

/// Grade one tier cell against the `selo` vocabulary.
///
/// Accepts exactly what `SealTier`'s `Display` emits: `truly-unrep`,
/// `parse-time-rejected`, `only-mitigated (C2)`. Surrounding emphasis markers
/// and backticks are stripped first — a table cell is prose, and `**only-
/// mitigated (C4)**` is the same claim.
#[must_use]
pub fn grade_tier(cell: &str) -> TierVerdict {
    let text = cell.trim().trim_matches(['*', '`', '_']).trim();

    let Some(label) = TIERS.iter().find(|t| text.starts_with(**t)) else {
        return TierVerdict::OutsideVocabulary;
    };
    if *label != MITIGATED {
        // A ceiling on a non-mitigation is meaningless but harmless; the
        // remainder is free text either way.
        return TierVerdict::Valid;
    }

    let rest = text[label.len()..].trim();
    let named = CEILINGS.iter().any(|code| {
        rest.split(|c: char| !c.is_ascii_alphanumeric()).any(|token| token == *code)
    });

    if named { TierVerdict::Valid } else { TierVerdict::MitigationWithoutCeiling }
}

/// Split one markdown table row into trimmed cells.
fn cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_owned())
        .collect()
}

/// Is this the `|---|---|` separator under a table header?
fn is_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|')
        && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
        && trimmed.contains('-')
}

/// Every ledger a body declares, as parsed rows.
///
/// An outer `None` means the body declares no ledger at all. An inner empty
/// `Vec` means a marker was found but no table followed it.
#[must_use]
pub fn scan_ledgers(body: &str) -> Vec<Vec<LedgerRow>> {
    let lines: Vec<&str> = body.lines().collect();
    let mut ledgers = Vec::new();
    let mut fenced = false;

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        // The marker DECLARES, so it must be the whole line. Merely containing
        // it is how a skill *writes about* the convention — "add a
        // `<!-- tier-ledger -->` marker above a table" — and reading that
        // sentence as a declaration turns every skill that documents the
        // format into a malformed ledger. All three pipeline skills do exactly
        // that, which is how this was caught.
        if fenced || trimmed.trim_end() != LEDGER_MARKER {
            continue;
        }

        // Find the table header within reach of the marker.
        let header = lines
            .iter()
            .enumerate()
            .skip(index + 1)
            .take(MARKER_REACH)
            .find(|(_, l)| l.trim_start().starts_with('|'))
            .map(|(at, _)| at);

        let Some(header_at) = header else {
            ledgers.push(Vec::new());
            continue;
        };

        let mut rows = Vec::new();
        for line in lines.iter().skip(header_at + 1) {
            if !line.trim_start().starts_with('|') {
                break;
            }
            if is_separator(line) {
                continue;
            }
            let cells = cells(line);
            if cells.len() < 2 {
                continue;
            }
            rows.push(LedgerRow {
                subject: cells.first().cloned().unwrap_or_default(),
                tier: cells.last().cloned().unwrap_or_default(),
            });
        }
        ledgers.push(rows);
    }

    ledgers
}

/// Validates declared tier ledgers, and that skills required to have one do.
///
/// Structural like sync and frontmatter: "this row claims a mitigation and names
/// no ceiling" has an objective right answer a human fixes by naming the
/// ceiling, so it rides with the always-on suite and is safe to gate a build on.
pub struct TierLedgerChecker {
    /// Skills that MUST declare a ledger. Empty means "validate what is
    /// declared, require nothing" — the honest default for a corpus where most
    /// skills have no seal to grade.
    pub required: BTreeSet<String>,
}

impl Checker for TierLedgerChecker {
    fn kind(&self) -> CheckKind { CheckKind::TierLedger }

    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        for name in &ctx.dir_names {
            let Some(content) = ctx.contents.get(name) else { continue };
            let ledgers = scan_ledgers(body_of(content));

            if ledgers.is_empty() {
                if self.required.contains(name) {
                    errors.push(LintError::LedgerMissing {
                        kind: CheckKind::TierLedger,
                        skill: name.clone(),
                    });
                }
                continue;
            }

            for rows in &ledgers {
                if rows.is_empty() {
                    errors.push(LintError::LedgerMalformed {
                        kind: CheckKind::TierLedger,
                        skill: name.clone(),
                    });
                    continue;
                }
                for row in rows {
                    match grade_tier(&row.tier) {
                        TierVerdict::Valid => {}
                        TierVerdict::MitigationWithoutCeiling => {
                            errors.push(LintError::LedgerMitigationUnbounded {
                                kind: CheckKind::TierLedger,
                                skill: name.clone(),
                                subject: row.subject.clone(),
                            });
                        }
                        TierVerdict::OutsideVocabulary => {
                            errors.push(LintError::LedgerTierUnknown {
                                kind: CheckKind::TierLedger,
                                skill: name.clone(),
                                subject: row.subject.clone(),
                                tier: row.tier.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocabulary is `selo`'s, copied deliberately rather than depended on.
    ///
    /// This pins the copy: if `selo::SealTier::label()` or `Ceiling::code()`
    /// ever renders something else, the strings here are what must be
    /// re-derived — and this test is where a reader is told that.
    #[test]
    fn the_vocabulary_matches_selo() {
        assert_eq!(TIERS, ["truly-unrep", "parse-time-rejected", "only-mitigated"]);
        assert_eq!(CEILINGS, ["C1", "C2", "C3", "C4", "C5", "C6"]);
    }

    #[test]
    fn a_named_ceiling_is_valid() {
        for code in CEILINGS {
            let cell = ["only-mitigated (", code, ")"].concat();
            assert_eq!(grade_tier(&cell), TierVerdict::Valid, "{cell}");
        }
    }

    /// RED: the whole point. A mitigation with no ceiling is the one row shape
    /// the Rust type makes unconstructible and prose does not.
    #[test]
    fn a_mitigation_without_a_ceiling_is_caught() {
        assert_eq!(grade_tier("only-mitigated"), TierVerdict::MitigationWithoutCeiling);
        assert_eq!(grade_tier("only-mitigated — for now"), TierVerdict::MitigationWithoutCeiling);
        assert_eq!(grade_tier("**only-mitigated**"), TierVerdict::MitigationWithoutCeiling);
        // C7 is not a ceiling selo can render.
        assert_eq!(grade_tier("only-mitigated (C7)"), TierVerdict::MitigationWithoutCeiling);
    }

    #[test]
    fn the_stronger_tiers_need_no_ceiling() {
        assert_eq!(grade_tier("truly-unrep"), TierVerdict::Valid);
        assert_eq!(grade_tier("parse-time-rejected"), TierVerdict::Valid);
        assert_eq!(grade_tier("`truly-unrep`"), TierVerdict::Valid);
    }

    #[test]
    fn a_tier_outside_the_vocabulary_is_caught() {
        // Exactly the vocabulary the corpus's recon tables use — which is why
        // a ledger declares itself instead of being guessed from its header.
        assert_eq!(grade_tier("SHIPPED primitive"), TierVerdict::OutsideVocabulary);
        assert_eq!(grade_tier("NET-NEW"), TierVerdict::OutsideVocabulary);
        assert_eq!(grade_tier(""), TierVerdict::OutsideVocabulary);
    }

    #[test]
    fn a_declared_ledger_is_parsed() {
        let body = "\
<!-- tier-ledger -->

| invariant | realization | tier |
|---|---|---|
| a bad tag | typed enum | truly-unrep |
| a live write | shadow gate | only-mitigated (C2) |
";
        let ledgers = scan_ledgers(body);
        assert_eq!(ledgers.len(), 1);
        assert_eq!(ledgers[0].len(), 2);
        assert_eq!(ledgers[0][0].subject, "a bad tag");
        assert_eq!(ledgers[0][1].tier, "only-mitigated (C2)");
    }

    #[test]
    fn an_undeclared_table_is_not_a_ledger() {
        // The recon table shape that exists in naturalize today: a `tier`
        // column, milestone vocabulary, no marker. It must stay untouched.
        let body = "\
| zot capability | realization | tier |
|---|---|---|
| blob store | armazem | SHIPPED primitive |
";
        assert!(scan_ledgers(body).is_empty());
    }

    /// Writing ABOUT the marker is not declaring one.
    #[test]
    fn a_prose_mention_of_the_marker_is_not_a_declaration() {
        let body = "Add a `<!-- tier-ledger -->` marker above a table whose last column\n\
                    is the tier. The `<!-- tier-ledger -->` marker is what declares it.\n";
        assert!(scan_ledgers(body).is_empty());
    }

    #[test]
    fn a_marker_with_no_table_is_malformed() {
        let ledgers = scan_ledgers("<!-- tier-ledger -->\n\nprose, and more prose.\n");
        assert_eq!(ledgers, vec![Vec::new()]);
    }

    #[test]
    fn a_fenced_marker_is_an_example_not_a_ledger() {
        let body = "```\n<!-- tier-ledger -->\n| a | b | only-mitigated |\n```\n";
        assert!(scan_ledgers(body).is_empty());
    }
}
