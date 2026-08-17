//! Slash-command pointers — does the `/skill-name` a body names still exist?
//!
//! A skill body steers an agent by naming other skills: "run `/vocabularify`
//! first", "its sibling practice is **/naturalize**". That pointer is a routing
//! instruction, and when the skill it names has been renamed or folded away the
//! agent goes looking for a slash command that no longer resolves — the same
//! wasted session a dead file path costs, arriving through a different door.
//!
//! # Why this is not [`super::links`]
//!
//! [`super::links::PathResolutionChecker`] resolves two forms — `[text](./path)`
//! and a backticked `` `<repo>/<path>` `` — against the FILESYSTEM. A bare
//! `` `/vocabularify` `` is neither: it has no second segment, so the repo-path
//! matcher rejects it outright, and it is not a markdown link. Measured on this
//! corpus 2026-07-31: with four dead `/vocabulary-bridging` pointers present,
//! `skill-lint check` passed green BOTH with and without
//! `--skip-path-resolution`. The class was not under-gated, it was **invisible**.
//!
//! Two consequences shape this checker:
//!
//! 1. **It resolves against the SKILL LISTING, not the filesystem** — the same
//!    oracle [`super::MapIntegrityChecker`] already uses for the map's own
//!    `references:` edges. The listing is in [`super::CheckContext`]
//!    unconditionally, so this check needs no sibling repositories and no
//!    writable root.
//! 2. **It has no disable FLAG** — living with a dead pointer is what the
//!    scoped `pending-skill-pointer:` waiver below is for, not a switch.
//!
//!    This point previously read "it is therefore not disableable", on the
//!    reasoning that the knowably-unavailable case "cannot arise here — the
//!    skills and the map travel together". **Corrected 2026-07-31: it does
//!    arise.** A caller may lint a GATED SUBSET with no map at all
//!    (`blackmatter-claude`, whose fleet map lives in `blackmatter-pleme`),
//!    which strips the federating half of the oracle and made four live
//!    cross-repo pointers report as dead — blocking a whole system closure.
//!    So the checker now self-suppresses on an EMPTY map, the same way an
//!    absent [`super::CheckContext::oracle`] already silences path resolution.
//!    See [`SkillPointerChecker`] for the measurement. That is a property of
//!    the input, not an operator-facing off-switch: where a map exists, the
//!    check is still mandatory and still catches dead pointers.
//!
//! # The matcher is calibrated, not guessed
//!
//! `/name` is also how a URL route, an absolute path and an alternation are
//! written. Measured over the 151-skill corpus, a naive matcher yields ~50
//! findings that are not pointers at all: `/code` (76), `/etc` (21), `/metrics`
//! (19), `/tmp` (16), `/nix`, `/var`, `/usr`, `/api`, `/bin`, `/readyz`… Three
//! rules cut that to zero without losing the real pointers:
//!
//! * **A hyphen is required.** Every one of the single-word noise tokens above
//!   is one segment; the pointers that matter — `/vocabulary-bridging`,
//!   `/big-bang-pleme`, `/algorithmic-prowess-seal` — are compounds. This does
//!   mean a dead one-word pointer (`/viggy`) is MISSED, and that is the
//!   deliberate direction: a miss leaves the status quo, a false positive
//!   discredits every true finding and gets the gate switched off.
//! * **An inline code span must be EXACTLY the pointer.** `` `/vocabularify` ``
//!   is a pointer; `` `gh api repos/<org>/<repo>/commits/<sha>/check-suites` ``
//!   is a command that happens to end in one.
//! * **In prose, the `/` must follow a real boundary** — whitespace, or one of
//!   `([*,;·—→`. This is what rejects the alternation form `` `X`/y-z `` ("X or
//!   y-z", written 4× in this corpus as `` `ReplicaBand`/lifecycle-breath ``,
//!   `` `builder-pool`/runtime-managed ``, `` `db_pass`/service-shared-key ``,
//!   `[[carve]]/enjulho-composed`) and `http://host:8090/org-root`. Two of the
//!   four are shown with neutral identifiers — what was measured, and what the
//!   fixtures reproduce byte-for-byte, is the SHAPE, not the words.
//!
//! Fenced blocks are out of scope for the same reason they are in `links`: a
//! fence is where a skill *demonstrates* a command, not where it issues one.

use std::collections::BTreeSet;

use crate::error::{CheckKind, LintError};

use super::links::body_of;
use super::{CheckContext, Checker};

/// Marker declaring a `/name` token that is legitimately not a skill.
///
/// Scoped exactly like `pending-path:` — it names the token it excuses, so it
/// cannot silence the next dead pointer too:
///
/// ```text
/// pending-skill-pointer: /org-root — an HTTP route of this service, not a slash command
/// ```
///
/// The residual class this exists for is real and irreducible: an HTTP route and
/// a slash command are the same string. Nothing structural tells them apart, so
/// the author says which.
const WAIVER_MARKER: &str = "pending-skill-pointer:";

/// Characters after which a `/` opens a slash-command pointer.
///
/// A whitelist, never a blacklist: the alternation form `` `X`/y-z `` and the
/// URL form `:8090/org-root` are both "some character, then a slash", and only
/// an enumerated set of genuine boundaries keeps them out. An unanticipated
/// shape stays silent rather than becoming a finding.
fn is_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | '[' | '*' | ',' | ';' | '·' | '—' | '→' | '⊕')
}

/// Everything one pass over a body yields.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PointerScan {
    /// Skill names pointed at, without the leading `/`, in document order.
    pub refs: Vec<String>,
    /// Tokens declared not-a-skill-pointer by a `pending-skill-pointer:` line.
    pub waived: BTreeSet<String>,
}

/// Extract every `/skill-name` pointer and every waiver from a body.
///
/// Pure: no oracle, no filesystem. Resolution is the checker's job so that
/// extraction — where all the false-positive risk lives — is testable alone.
#[must_use]
pub fn scan_pointers(body: &str) -> PointerScan {
    let mut scan = PointerScan::default();
    let mut fenced = false;

    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }

        if let Some(token) = waived_token(line) {
            scan.waived.insert(token);
        }

        // Odd segments of a backtick split are inline code spans, even segments
        // are prose. The two forms are matched by different rules because they
        // fail in different directions: a span carries its own delimiters, so
        // "is the whole span the pointer?" is answerable exactly, while prose
        // has to infer the left boundary.
        for (index, segment) in line.split('`').enumerate() {
            if index % 2 == 1 {
                if let Some(name) = whole_span_pointer(segment) {
                    scan.refs.push(name);
                }
            } else {
                // A prose segment at index 0 begins the line; any later even
                // segment begins immediately after a CLOSING backtick, so a
                // match at its offset 0 butted straight against `X` — the
                // alternation form, not a pointer.
                collect_prose_pointers(segment, index == 0, &mut scan.refs);
            }
        }
    }

    scan
}

/// The skill name when an inline code span is EXACTLY one pointer.
fn whole_span_pointer(segment: &str) -> Option<String> {
    let raw = segment.trim();
    let rest = raw.strip_prefix('/')?;
    skill_shaped(rest).then(|| rest.to_owned())
}

/// Pull `/skill-name` pointers out of one prose segment.
fn collect_prose_pointers(segment: &str, at_line_start: bool, refs: &mut Vec<String>) {
    let chars: Vec<char> = segment.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '/' {
            index += 1;
            continue;
        }

        let boundary =
            if index == 0 { at_line_start } else { is_boundary(chars[index - 1]) };

        let start = index + 1;
        let mut end = start;
        while end < chars.len()
            && (chars[end].is_ascii_lowercase() || chars[end].is_ascii_digit() || chars[end] == '-')
        {
            end += 1;
        }

        // Whatever follows must not continue a path or an identifier —
        // `/code/github/x` and `/v1alpha1beta` are not pointers.
        //
        // A `.` is the one ambiguous tail: it ends a sentence ("run
        // /vocabularify.") and it introduces an extension ("/some-file.md").
        // Alphanumeric AFTER the dot tells them apart, so a pointer closing a
        // sentence is read while a filename stays out.
        let clean_tail = match chars.get(end) {
            None => true,
            Some('.') => chars.get(end + 1).is_none_or(|c| !c.is_ascii_alphanumeric()),
            Some(&c) => !(c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-')),
        };

        if boundary && clean_tail {
            let name: String = chars[start..end].iter().collect();
            if skill_shaped(&name) {
                refs.push(name);
            }
        }

        index = end.max(start);
    }
}

/// Is this token shaped like a compound skill name?
///
/// Lowercase, digits and inner hyphens only, with AT LEAST ONE hyphen. The
/// hyphen requirement is the whole noise filter — see the module docs for the
/// measurement that chose it.
fn skill_shaped(name: &str) -> bool {
    let mut segments = name.split('-');
    let first = segments.next().unwrap_or_default();

    if !first.starts_with(|c: char| c.is_ascii_lowercase()) {
        return false;
    }
    let mut compound = false;
    for segment in segments {
        compound = true;
        if segment.is_empty() || !segment.chars().all(|c| c.is_ascii_alphanumeric()) {
            return false;
        }
    }
    compound && first.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The token a `pending-skill-pointer:` line excuses, if the line is one.
fn waived_token(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(WAIVER_MARKER)?;
    let token = rest
        .split_whitespace()
        .next()?
        .trim_matches(['`', '"', '\'', '*'])
        .trim_end_matches(['.', ',', ';']);
    let name = token.strip_prefix('/').unwrap_or(token);
    (!name.is_empty()).then(|| name.to_owned())
}

/// Reports `/skill-name` pointers in skill bodies that name no known skill.
///
/// The oracle is the union of the skill DIRECTORIES on disk and the skill MAP's
/// entries. The union matters: the map federates skills owned by sibling repos
/// (`repo: blackmatter-claude`), which have no local directory but are perfectly
/// live routing targets — resolving against directories alone would report every
/// cross-repo pointer as dead.
///
/// Findings are reported as [`LintError::BrokenReference`], the same variant the
/// map's own `references:` edges use, because it is the same defect: a skill
/// names a skill that does not exist. Only the [`CheckKind`] differs, which is
/// what tells the operator whether to fix a YAML edge or a line of prose.
///
/// # An EMPTY map means the oracle is knowably partial — report nothing
///
/// The module docs above claim this check "is therefore not disableable",
/// reasoning that "the skills and the map travel together, in the working tree
/// and inside the build sandbox alike — so that case cannot arise here."
///
/// **Measured 2026-07-31: it does arise, and the claim was false.**
/// `blackmatter-claude`'s `skill-map-check` runs
///
/// ```text
/// skill-lint check --skills-dir <GATED SUBSET> --map-dir <EMPTY> \
///   --skip-sync --skip-map-integrity --skip-version
/// ```
///
/// because that repo has **no local `skill-map.d`** — the fleet map lives in
/// `blackmatter-pleme` — and it deliberately lints a *gated subset* (9 of 22
/// skills) rather than the whole corpus. Both halves of the oracle are thus
/// incomplete at once, and the federation this checker's own docs rely on is
/// simply absent. The result was four findings against pointers that are all
/// perfectly live — `/rust-tool`, `/rust-service`, `/helm-k8s-charts`,
/// `/claude-skills`, every one of them a top-level key in the fleet map
/// (`rust.yaml:8`, `rust.yaml:22`, `infrastructure.yaml:33`, `meta.yaml:1`) —
/// which blocked the whole darwin system closure from building.
///
/// So an empty map gets the same treatment [`super::CheckContext::oracle`]
/// already gives an absent path oracle: when the answer is *knowably
/// unavailable*, report nothing rather than report a guess. This is not a way
/// to live with dead pointers — that is what `pending-skill-pointer:` is for —
/// and it costs no coverage where the check can actually work: the fleet-wide
/// gate passes the real map, so `map_names` is populated there and every
/// pointer is still resolved.
///
/// The alternative — "fix" the four skills by deleting correct
/// cross-references to green a gate — is the vacuous fix this corpus exists to
/// prevent.
pub struct SkillPointerChecker;

impl Checker for SkillPointerChecker {
    fn kind(&self) -> CheckKind { CheckKind::SkillPointer }

    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        // No map at all => the federating half of the oracle is absent and the
        // listing is knowably partial, so every cross-repo and out-of-subset
        // pointer would report as dead. Silence beats a confident wrong answer.
        // See the type docs for the measured case that added this.
        if ctx.map_names.is_empty() {
            return;
        }

        let known: BTreeSet<&str> = ctx
            .dir_names
            .iter()
            .chain(ctx.map_names.iter())
            .map(String::as_str)
            .collect();

        for name in &ctx.dir_names {
            let Some(content) = ctx.contents.get(name) else { continue };
            let scan = scan_pointers(body_of(content));

            // One pointer repeated N times is one defect with one fix.
            let mut reported: BTreeSet<&str> = BTreeSet::new();

            for target in &scan.refs {
                if known.contains(target.as_str()) || scan.waived.contains(target) {
                    continue;
                }
                if reported.insert(target.as_str()) {
                    errors.push(LintError::BrokenReference {
                        kind: CheckKind::SkillPointer,
                        skill: name.clone(),
                        target: target.clone(),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(body: &str) -> Vec<String> { scan_pointers(body).refs }

    #[test]
    fn a_bare_pointer_in_prose_is_extracted() {
        assert_eq!(refs("its sibling practice is /vocabulary-bridging."), ["vocabulary-bridging"]);
    }

    #[test]
    fn the_two_forms_the_corpus_actually_used_are_extracted() {
        // Both dead-pointer shapes found in algorithmic-prowess-seal/SKILL.md.
        assert_eq!(
            refs("`(def…)` bridging) · **/vocabulary-bridging** (its default-on sibling) ·"),
            ["vocabulary-bridging"]
        );
        assert_eq!(refs("reference. Sibling practice: `/vocabulary-bridging`."), ["vocabulary-bridging"]);
    }

    #[test]
    fn a_single_word_pointer_is_deliberately_out_of_scope() {
        // The hyphen requirement is what makes `/etc`, `/tmp`, `/metrics`,
        // `/code`, `/readyz` silent. `/viggy` pays that price knowingly.
        assert!(refs("run /viggy and /etc and /tmp and /metrics").is_empty());
    }

    /// THE noise class, measured: `X/y-z` means "X or y-z", not a pointer.
    ///
    /// Four live instances in this corpus, every one of them a false positive
    /// under a naive matcher.
    ///
    /// Two carry neutral identifiers rather than the corpus's own words. The
    /// measured thing is the SHAPE — a backtick or a bare letter immediately
    /// before the `/`, then a hyphenated compound — and every shape below is
    /// reproduced exactly, including the letter-before-slash case (`KEK/`) that
    /// the backtick case would otherwise hide. Do not expect to grep the corpus
    /// for these literals and find them.
    #[test]
    fn the_alternation_form_is_not_a_pointer() {
        assert!(refs("breathe's `ReplicaBand`/lifecycle-breath").is_empty());
        assert!(refs("a vendor's own `builder-pool`/runtime-managed scale set").is_empty());
        assert!(refs("`Secret` for KEK/`db_pass`/service-shared-key, the MySQL").is_empty());
        assert!(refs("Helm-deployed (or [[carve]]/enjulho-composed, whichever").is_empty());
    }

    #[test]
    fn a_url_route_is_not_a_pointer() {
        assert!(refs("curl http://localhost:8090/org-root").is_empty());
        assert!(refs("see ~/code/github/pleme-io/theory for the-storm").is_empty());
    }

    /// The ambiguous tail, both ways: sentence punctuation reads, an extension
    /// does not.
    #[test]
    fn a_closing_period_reads_but_a_file_extension_does_not() {
        assert_eq!(refs("finish with /run-trifecta."), ["run-trifecta"]);
        assert!(refs("open /some-file.md for the detail").is_empty());
    }

    /// A span has to be the WHOLE pointer — a command ending in one is a
    /// command.
    #[test]
    fn a_pointer_inside_a_longer_code_span_is_not_a_pointer() {
        assert!(
            refs("(`gh api repos/<org>/<repo>/commits/<sha>/check-suites`) — a commit").is_empty()
        );
    }

    #[test]
    fn fenced_blocks_are_skipped() {
        let body = "\
`/live-one`
```
`/fenced-dead`
pending-skill-pointer: /live-one
```
**/live-two**
";
        let scan = scan_pointers(body);
        assert_eq!(scan.refs, ["live-one", "live-two"]);
        // A waiver shown inside a fence is an example of a waiver, not one.
        assert!(scan.waived.is_empty(), "fenced waiver leaked: {:?}", scan.waived);
    }

    #[test]
    fn waivers_are_scoped_to_the_token_they_name() {
        let scan = scan_pointers(
            "pending-skill-pointer: /org-root — an HTTP route, not a slash command\n\
             pending-skill-pointer: `other-thing` — also not one\n",
        );
        assert!(scan.waived.contains("org-root"));
        assert!(scan.waived.contains("other-thing"));
        assert_eq!(scan.waived.len(), 2);
    }

    #[test]
    fn a_waiver_naming_nothing_waives_nothing() {
        assert!(scan_pointers("pending-skill-pointer:\n").waived.is_empty());
    }

    #[test]
    fn pointers_are_found_after_every_accepted_boundary() {
        assert_eq!(refs("(/fan-out, then"), ["fan-out"]);
        assert_eq!(refs("first · /big-bang-pleme · then"), ["big-bang-pleme"]);
        assert_eq!(refs("/at-line-start counts"), ["at-line-start"]);
        assert_eq!(refs("the-storm ⊕ /the-chaos"), ["the-chaos"]);
    }
}
