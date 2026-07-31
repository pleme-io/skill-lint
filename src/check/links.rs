//! Link resolution — do the paths a skill body points at actually exist?
//!
//! A skill is read by an agent that will *follow* what it names. A pointer at a
//! file that does not exist costs a whole session: the agent reads the skill,
//! goes looking, and finds nothing. Measured across the fleet corpus before this
//! check existed: 3 dead relative links and 19 dead repo-relative paths across
//! 144 skills — every one of them a session waiting to be wasted.
//!
//! # What is in scope, and why
//!
//! Two forms, both deliberately narrow:
//!
//! 1. **Relative markdown links** — `[text](./path)` / `[text](../path)`,
//!    resolved from the skill's own directory. Bare-relative targets
//!    (`[text](path)`) are OUT of scope: measured against the real corpus they
//!    produced 0 true hits and 6 pieces of noise (`~/`-prefixed paths, one
//!    `checkpoint(C` fragment of prose). A form that only ever produces noise
//!    does not earn a place in the matcher.
//! 2. **Backticked repo-relative paths** — `` `<repo>/<path>` ``, resolved from
//!    the root that holds sibling repositories, and checked ONLY when `<repo>`
//!    is a repository present on this machine. An absent clone yields silence,
//!    never a finding: a check that fires because the reader has not cloned
//!    something is untrustworthy, and an untrustworthy check gets disabled,
//!    which costs more than the dead pointers it would have caught.
//!
//! # Fenced code blocks are OUT of scope — deliberately
//!
//! A fence is where a skill *demonstrates* — example trees, sample commands,
//! illustrative paths that were never meant to exist. Matching inside one makes
//! the check fire on the documentation OF a path rather than on a path, which
//! is precisely the false-positive class that gets a gate switched off. The
//! exclusion is symmetric: a `pending-path:` waiver inside a fence is not a
//! waiver either, because a fence is an example of a waiver, not a waiver.
//!
//! Inline code spans are the opposite case — they are exactly where the
//! repo-relative form lives — so they are scanned, and markdown links are
//! scanned only OUTSIDE them for the same reason a fence is skipped.

use std::collections::BTreeSet;

use crate::error::{CheckKind, LintError, PathForm};

use super::{CheckContext, Checker};

/// Root segments that name a local repository AND are conventional directory
/// names inside repositories.
///
/// `docs/arch/landmarks.md` in a skill about `mathscape` means *mathscape's*
/// docs — but a repository named `docs` also exists, so resolving it against
/// the repo root would report a dead path that is not dead, merely
/// misattributed. Measured on the real corpus, `docs` alone produced 22 such
/// misattributions; `infrastructure` produced 2; `actions` produced 1
/// (`actions/setup-node`, written beside `dtolnay/rust-toolchain` — the upstream
/// GitHub-action namespace, which collides with the `actions` repo permanently
/// and for every workflow anyone ever cites).
///
/// The rule here is precision over recall: where repo-qualified and repo-local
/// cannot be told apart, say nothing. A miss leaves a dead pointer uncaught,
/// which is the status quo; a false positive discredits every other finding.
/// Segments are listed prophylactically — a name need not collide today to be
/// excluded, because the collision appears the day someone creates the repo.
const AMBIGUOUS_ROOT_SEGMENTS: &[&str] = &[
    ".claude", ".github", "actions", "assets", "benches", "bin", "charts",
    "config", "crates", "docs", "examples", "infrastructure", "lib", "modules",
    "packages", "programs", "scripts", "skills", "spec", "specs", "src",
    "target", "templates", "test", "tests", "tools", "vendor",
];

/// Marker declaring a path that is legitimately absent.
///
/// The waiver is SCOPED — it names the exact path it excuses:
///
/// ```text
/// pending-path: sui/sui-store/src/postgres.rs — ships on the tiered-backend branch, unmerged
/// ```
///
/// Naming the path is what keeps the waiver honest. A bare "disable this check
/// here" marker would silence the next dead pointer too, including one nobody
/// decided to accept. The trailing reason is for the human reader; the linter
/// keys on the path.
const WAIVER_MARKER: &str = "pending-path:";

/// A path a skill body points at.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BodyRef {
    /// The path as written, cleaned of trailing punctuation and locators.
    pub path: String,
    /// How it was written — decides what it resolves against.
    pub form: PathForm,
}

/// Everything one pass over a body yields.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BodyScan {
    /// Paths pointed at, in document order.
    pub refs: Vec<BodyRef>,
    /// Paths declared legitimately-absent by a `pending-path:` line.
    pub waived: BTreeSet<String>,
}

/// The body of a `SKILL.md` — everything after the frontmatter block.
///
/// Frontmatter is excluded because it is the routing surface (name,
/// description, metadata), not prose that points anywhere.
#[must_use]
pub fn body_of(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let rest = &trimmed[3..];
    let Some(end) = rest.find("\n---") else { return content };
    let after = &rest[end + 4..];
    after.find('\n').map_or("", |nl| &after[nl + 1..])
}

/// Extract every in-scope path reference and every waiver from a body.
///
/// Pure: no filesystem access, no oracle. Resolution is the checker's job, so
/// extraction can be tested on its own.
#[must_use]
pub fn scan_body(body: &str) -> BodyScan {
    let mut scan = BodyScan::default();
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

        if let Some(path) = waived_path(line) {
            scan.waived.insert(path);
        }

        // Odd segments of a backtick split are inline code spans; even segments
        // are prose. Repo paths live in code, links live in prose — scanning
        // each form only where it belongs keeps a link written INSIDE a code
        // span (i.e. a link being shown, not made) out of the results.
        for (index, segment) in line.split('`').enumerate() {
            if index % 2 == 1 {
                if let Some(path) = code_span_path(segment) {
                    scan.refs.push(BodyRef { path, form: PathForm::RepoPath });
                }
            } else {
                collect_relative_links(segment, &mut scan.refs);
            }
        }
    }

    scan
}

/// Pull `[text](./path)` / `[text](../path)` targets out of a prose segment.
fn collect_relative_links(segment: &str, refs: &mut Vec<BodyRef>) {
    let mut cursor = 0;
    while let Some(found) = segment[cursor..].find("](") {
        let start = cursor + found + 2;
        let Some(offset) = segment[start..].find(')') else { break };
        let target = &segment[start..start + offset];
        cursor = start + offset + 1;

        if !(target.starts_with("./") || target.starts_with("../")) {
            continue;
        }
        // An anchor addresses a heading inside the target, not a different
        // file; the file is what resolves.
        let target = target.split('#').next().unwrap_or(target);
        if target.is_empty() {
            continue;
        }
        refs.push(BodyRef {
            path: target.trim_end_matches('/').to_owned(),
            form: PathForm::RelativeLink,
        });
    }
}

/// Interpret one inline code span as a repo-relative path, or reject it.
///
/// Rejection is a whitelist, not a blacklist: a path is alphanumerics plus
/// `-_./` and nothing else. Anything with a glob, a placeholder (`<cluster>`,
/// `{name}`, `$VAR`), a space, or a scheme is a template or a sentence, not a
/// path — and a blacklist would let the next unanticipated shape through.
fn code_span_path(segment: &str) -> Option<String> {
    let raw = segment.trim().trim_end_matches(['.', ',', ';', ':']);
    let raw = strip_locator(raw).trim_end_matches('/');

    if raw.is_empty() || !raw.contains('/') || raw.contains("://") {
        return None;
    }
    if raw.starts_with('/') || raw.starts_with('.') || raw.starts_with('-') {
        return None;
    }
    if !raw.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')) {
        return None;
    }
    Some(raw.to_owned())
}

/// Strip a source locator: `path.rs:120`, `path.rs:120-140`, `path.rs::test_name`.
///
/// A locator addresses a position INSIDE a file; the file is the thing that
/// either exists or does not. Without this, 24 of the corpus's live pointers
/// read as dead — the check would have opened by being wrong about more of its
/// findings than it got right.
fn strip_locator(raw: &str) -> &str {
    let raw = match raw.find("::") {
        Some(pos) if raw[..pos].contains('/') => &raw[..pos],
        _ => raw,
    };
    match raw.rfind(':') {
        Some(pos)
            if raw[..pos].contains('/')
                && !raw[pos + 1..].is_empty()
                && raw[pos + 1..].chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            &raw[..pos]
        }
        _ => raw,
    }
}

/// The path a `pending-path:` line excuses, if the line is one.
fn waived_path(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(WAIVER_MARKER)?;
    let token = rest
        .split_whitespace()
        .next()?
        .trim_matches(['`', '"', '\'', '*'])
        .trim_end_matches(['.', ',', ';']);
    let token = strip_locator(token).trim_end_matches('/');
    (!token.is_empty()).then(|| token.to_owned())
}

/// Reports paths a skill body points at that do not exist.
///
/// Structural, not freshness: "the file is not there" has an objective right
/// answer a human fixes without manufacturing a claim, so this is always-on and
/// safe to gate CI on — unlike staleness, where a bot bumping a date to go
/// green produces a false claim worse than the stale one.
///
/// When the source cannot resolve paths at all (no oracle), the checker reports
/// NOTHING. Unknown is not the same as absent, and a check that treats it as
/// absent reports every path in the corpus as broken.
pub struct PathResolutionChecker;

impl Checker for PathResolutionChecker {
    fn kind(&self) -> CheckKind { CheckKind::PathResolution }

    fn check(&self, ctx: &CheckContext, errors: &mut Vec<LintError>) {
        let Some(oracle) = ctx.oracle.as_deref() else { return };

        for name in &ctx.dir_names {
            let Some(content) = ctx.contents.get(name) else { continue };
            let scan = scan_body(body_of(content));

            // One pointer repeated N times is one defect with one fix.
            let mut reported: BTreeSet<&BodyRef> = BTreeSet::new();

            for reference in &scan.refs {
                if scan.waived.contains(&reference.path) {
                    continue;
                }
                let resolves = match reference.form {
                    PathForm::RelativeLink => oracle.exists_near_skill(name, &reference.path),
                    PathForm::RepoPath => {
                        // Two legitimate authored readings of the same string:
                        // bare `<repo>/<path>` and org-qualified
                        // `<org>/<repo>/<path>`. A pointer that resolves under
                        // EITHER is not dead, so try both and only report when
                        // every knowable reading says absent.
                        let org_stripped = oracle.org_segment().and_then(|org| {
                            reference
                                .path
                                .strip_prefix(org.as_str())
                                .and_then(|rest| rest.strip_prefix('/'))
                                .map(str::to_owned)
                        });

                        // `None` = unknowable, so it contributes no verdict.
                        //
                        // `root_may_be_ambiguous` is false for an org-qualified
                        // reading, and that is the whole reason the org form is
                        // worth resolving: AMBIGUOUS_ROOT_SEGMENTS exists because a
                        // bare `actions/x` might mean the `actions` repo or some
                        // repo's own `actions/` directory. Writing `pleme-io/actions`
                        // settles that — the author named the org — so the
                        // ambiguity exclusion must not fire on it.
                        let judge = |candidate: &str, root_may_be_ambiguous: bool| {
                            let mut segments = candidate.split('/');
                            let root = segments.next().unwrap_or_default();
                            // A lone segment carrying an EXTENSION is a FILE at the
                            // org root — `pleme-io/CLAUDE.md` is the org's own
                            // CLAUDE.md, not a repo named `CLAUDE.md`. Fully
                            // knowable, so the repo gate (which exists only to
                            // excuse uncloned repos) must not swallow it.
                            //
                            // The dot must not be leading: `.github` is a
                            // repository (the org's special repo), not a file, so a
                            // dotfile-shaped name stays behind the repo gate and an
                            // uncloned `.github` reads as unknowable rather than
                            // dead.
                            let has_extension = root.rfind('.').is_some_and(|at| at > 0);
                            if segments.next().is_none() && has_extension {
                                return Some(oracle.exists_under_repo_root(candidate));
                            }
                            if (root_may_be_ambiguous
                                && AMBIGUOUS_ROOT_SEGMENTS.contains(&root))
                                || !oracle.has_repo(root)
                            {
                                return None;
                            }
                            Some(oracle.exists_under_repo_root(candidate))
                        };

                        // The org-qualified reading is PRIMARY when the path is
                        // org-qualified — it is what the author wrote and meant. The
                        // as-written reading is the fallback for the one case that
                        // makes it meaningful: a repository whose name equals the
                        // org's, which `tend` really does clone
                        // (github.com/pleme-io/pleme-io).
                        let verdict = match org_stripped.as_deref() {
                            Some(stripped) => judge(stripped, false).or_else(|| {
                                // ONE-WAY fallback. The as-written reading may only
                                // rescue a pointer to "alive", never condemn it to
                                // "dead": that `<org>/pleme-io/.github` is absent
                                // says nothing about the org's `.github` repo, which
                                // is merely uncloned. `filter` drops a negative back
                                // to unknowable so an unresolvable primary stays
                                // silent instead of borrowing a verdict from a
                                // reading nobody meant.
                                judge(&reference.path, true).filter(|&present| present)
                            }),
                            None => judge(&reference.path, true),
                        };
                        match verdict {
                            Some(resolved) => resolved,
                            None => continue,
                        }
                    }
                };
                if !resolves && reported.insert(reference) {
                    errors.push(LintError::UnresolvedPath {
                        kind: CheckKind::PathResolution,
                        skill: name.clone(),
                        path: reference.path.clone(),
                        form: reference.form,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(body: &str, form: PathForm) -> Vec<String> {
        scan_body(body)
            .refs
            .into_iter()
            .filter(|r| r.form == form)
            .map(|r| r.path)
            .collect()
    }

    #[test]
    fn body_of_strips_frontmatter() {
        let content = "---\nname: a\ndescription: X\n---\n\n# Body\ntext\n";
        assert_eq!(body_of(content), "\n# Body\ntext\n");
    }

    #[test]
    fn body_of_passes_through_content_without_frontmatter() {
        assert_eq!(body_of("# Body\n"), "# Body\n");
        assert_eq!(body_of("---\nunterminated\n"), "---\nunterminated\n");
    }

    #[test]
    fn relative_links_are_extracted() {
        let body = "see [a](./refs/a.md) and [b](../sib/b.md) and [c](../c.md#anchor)";
        assert_eq!(
            paths(body, PathForm::RelativeLink),
            ["./refs/a.md", "../sib/b.md", "../c.md"]
        );
    }

    #[test]
    fn absolute_and_bare_link_targets_are_out_of_scope() {
        // Bare-relative produced 0 true hits and 6 pieces of noise on the real
        // corpus; URLs and anchors are not filesystem paths at all.
        let body = "[a](https://example.com/x) [b](refs/b.md) [c](#anchor) [d](/etc/passwd)";
        assert!(paths(body, PathForm::RelativeLink).is_empty());
    }

    #[test]
    fn repo_paths_are_extracted_from_code_spans() {
        let body = "read `theory/THEORY.md` then `mado/docs/MACRO-VOCABULARY.md`";
        assert_eq!(
            paths(body, PathForm::RepoPath),
            ["theory/THEORY.md", "mado/docs/MACRO-VOCABULARY.md"]
        );
    }

    #[test]
    fn templates_globs_and_prose_are_rejected() {
        let body = "`k8s/clusters/<cluster>/RUNBOOK.md` `charts/*` `pangea/{parent}/{child}` \
                    `$HOME/x` `a word/here` `https://x.dev/y` `./relative/x` `noslash`";
        assert!(paths(body, PathForm::RepoPath).is_empty());
    }

    #[test]
    fn source_locators_are_stripped() {
        let body = "`samba/src/config.rs:190-205` `tend/src/x.rs:155` \
                    `tabeliao/tests/e2e.rs::provable_statement`";
        assert_eq!(
            paths(body, PathForm::RepoPath),
            ["samba/src/config.rs", "tend/src/x.rs", "tabeliao/tests/e2e.rs"]
        );
    }

    #[test]
    fn fenced_blocks_are_skipped_in_both_directions() {
        let body = "\
`live/one.md`
```
`fenced/dead.md`
[x](./fenced-dead.md)
pending-path: live/one.md
```
~~~sh
`tilde-fenced/dead.md`
~~~
[y](./live-two.md)
";
        let scan = scan_body(body);
        assert_eq!(
            scan.refs,
            vec![
                BodyRef { path: "live/one.md".into(), form: PathForm::RepoPath },
                BodyRef { path: "./live-two.md".into(), form: PathForm::RelativeLink },
            ]
        );
        // A waiver shown inside a fence is an example of a waiver, not one.
        assert!(scan.waived.is_empty(), "fenced waiver leaked: {:?}", scan.waived);
    }

    #[test]
    fn a_link_inside_a_code_span_is_not_a_link() {
        // The documentation OF the form must not match the form.
        assert!(paths("write it as `[text](./path.md)`", PathForm::RelativeLink).is_empty());
    }

    #[test]
    fn waivers_are_scoped_to_the_path_they_name() {
        let scan = scan_body(
            "pending-path: sui/sui-store/src/postgres.rs — unmerged branch\n\
             pending-path: `theory/GHOST.md` — planned\n",
        );
        assert!(scan.waived.contains("sui/sui-store/src/postgres.rs"));
        assert!(scan.waived.contains("theory/GHOST.md"));
        assert_eq!(scan.waived.len(), 2);
    }

    #[test]
    fn a_waiver_naming_nothing_waives_nothing() {
        assert!(scan_body("pending-path:\n").waived.is_empty());
    }
}
