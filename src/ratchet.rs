//! The baseline ratchet — one mechanism, shared by every gate that ships onto a
//! corpus already in violation.
//!
//! A gate that goes red on every run is a gate that gets skipped, so a gate has
//! to be adoptable on day one. The shape that makes it adoptable is not an
//! allowlist: known debt is recorded WITH THE MEASUREMENT IT HAD when recorded,
//! and exactly two things fail —
//!
//! - a violation the baseline does not name;
//! - a baselined item whose measurement has GROWN past the recorded value.
//!
//! That second rule is the whole seal. A pure allowlist lets a baselined 1,400 B
//! entry drift to 8,000 B in silence, which is precisely the failure mode being
//! sealed. Debt may shrink or hold; it may never grow.
//!
//! This module exists because that mechanism now has two consumers
//! ([`crate::claudemd`] and [`crate::workflows`]) and a ratchet re-implemented
//! per gate is a ratchet that will drift per gate — one of them would quietly
//! grow a `>=` where the other has `>`, and the looser gate would be the one
//! nobody noticed.

use std::collections::BTreeMap;

/// Known debt for one kind of measured thing: key → measurement when recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ratchet {
    recorded: BTreeMap<String, usize>,
}

/// What the ratchet says about one measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The baseline does not name this key — a NEW violation.
    Unrecorded,
    /// Recorded, and the measurement has not exceeded the recorded value.
    /// Known debt, holding or shrinking.
    Held,
    /// Recorded, and the measurement has grown past the recorded value.
    Grew {
        /// The measurement when the baseline was written.
        recorded: usize,
        /// How far past it the current measurement sits.
        grew: usize,
    },
}

impl Ratchet {
    /// The recorded measurement for `key`, if the baseline names it.
    #[must_use]
    pub fn recorded(&self, key: &str) -> Option<usize> { self.recorded.get(key).copied() }

    /// Record `key` at `size`.
    pub fn insert(&mut self, key: impl Into<String>, size: usize) {
        self.recorded.insert(key.into(), size);
    }

    /// How many items the baseline names.
    #[must_use]
    pub fn len(&self) -> usize { self.recorded.len() }

    /// Does the baseline name nothing?
    #[must_use]
    pub fn is_empty(&self) -> bool { self.recorded.is_empty() }

    /// Judge one measurement against the recorded debt.
    #[must_use]
    pub fn judge(&self, key: &str, measured: usize) -> Verdict {
        match self.recorded.get(key) {
            None => Verdict::Unrecorded,
            Some(&recorded) if measured > recorded => {
                Verdict::Grew { recorded, grew: measured - recorded }
            }
            Some(_) => Verdict::Held,
        }
    }
}

/// Parse baseline lines into `(kind, key, size)` triples.
///
/// The grammar is `<kind>: <key> <size>`. The size is the LAST
/// whitespace-separated token because a key contains spaces — parsing from the
/// left would truncate every key at its first space and silently cover nothing.
///
/// Blank lines and `#` comments are ignored, and so is anything else
/// unparseable: a malformed baseline must not be able to WEAKEN a gate by
/// accident. An unrecognized line covers nothing, which fails loudly on the
/// next run rather than passing quietly forever.
#[must_use]
pub fn parse_lines(text: &str) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((kind, rest)) = line.split_once(':') else { continue };
        let Some((key, size)) = rest.trim().rsplit_once(char::is_whitespace) else { continue };
        let Ok(size) = size.trim().parse::<usize>() else { continue };
        out.push((kind.trim().to_owned(), key.trim().to_owned(), size));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecorded_key_is_a_new_violation() {
        let r = Ratchet::default();
        assert_eq!(r.judge("k", 10), Verdict::Unrecorded);
    }

    #[test]
    fn recorded_debt_may_hold_or_shrink() {
        let mut r = Ratchet::default();
        r.insert("k", 10);
        assert_eq!(r.judge("k", 10), Verdict::Held);
        assert_eq!(r.judge("k", 3), Verdict::Held);
    }

    /// The seal. Without this, a baseline is an amnesty.
    #[test]
    fn recorded_debt_that_grows_by_one_fails() {
        let mut r = Ratchet::default();
        r.insert("k", 10);
        assert_eq!(r.judge("k", 11), Verdict::Grew { recorded: 10, grew: 1 });
    }

    #[test]
    fn parse_keeps_keys_containing_spaces() {
        let rows = parse_lines(
            "# comment\n\nentry: docs/CLAUDE.md::The Urdume Method 4139\n\
             run: wf/image-push.yml::push/Build and push 60\nnonsense\nbad: key notanumber\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("entry".to_owned(), "docs/CLAUDE.md::The Urdume Method".to_owned(), 4139));
        assert_eq!(rows[1], ("run".to_owned(), "wf/image-push.yml::push/Build and push".to_owned(), 60));
    }
}
