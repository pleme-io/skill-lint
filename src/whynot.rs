use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Serialize;

use crate::check::links::{body_of, scan_body};
use crate::check::{FsSource, SkillSource as _};
use crate::error::PathForm;
use crate::route::{self, Corpus, Scored, SkillDoc};
use crate::usage::{self, ReportInput, Window};

pub const DESCRIPTION_CAP: usize = 60;
pub const BODY_CAP_BYTES: usize = 12_000;
pub const LEXICAL_GAP_PERMILLE: u32 = 500;
pub const KEYWORD_NEEDLES: &[&str] = &["domain_keywords", "domainKeywords", "domain-keywords"];
const ROUTER_EXTENSIONS: &[&str] = &["nix", "rs", "b", "tlisp", "lisp", "py", "sh", "json", "ts", "go", "rb"];
const SKIPPED_DIRS: &[&str] = &[".git", "target", "node_modules", "result", "skills", "skill-map.d", "skill-lint", "docs"];
const PRONOUNS: &[&str] = &["i", "me", "my", "we", "our", "us", "you", "your"];
const ROUTER_WALK_DEPTH: usize = 10;
const PLUGIN_WALK_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Likely,
    Advisory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    NotDeployed,
    FrontmatterInvalid,
    UserOnly,
    Shadowed,
    StaleDeploy,
    DroppedByBudget,
    DescriptionTruncated,
    Outranked,
    LexicalGap,
    MapIntegrity,
    BodyOverCap,
}

impl Class {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotDeployed => "not_deployed",
            Self::FrontmatterInvalid => "frontmatter_invalid",
            Self::UserOnly => "user_only",
            Self::Shadowed => "shadowed",
            Self::StaleDeploy => "stale_deploy",
            Self::DroppedByBudget => "dropped_by_budget",
            Self::DescriptionTruncated => "description_truncated",
            Self::Outranked => "outranked",
            Self::LexicalGap => "lexical_gap",
            Self::MapIntegrity => "map_integrity",
            Self::BodyOverCap => "body_over_cap",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reason {
    pub class: Class,
    pub severity: Severity,
    pub evidence: String,
    pub fix: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outranker {
    pub skill: String,
    pub permille: u32,
    pub terms_it_has_that_this_lacks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Lexical {
    pub prompt_terms: usize,
    pub known_terms: usize,
    pub permille: u32,
    pub rank: Option<usize>,
    pub of: usize,
    pub matched: Vec<String>,
    pub outranked_by: Vec<Outranker>,
    pub candidate_terms: Vec<String>,
    pub keyword_only_hits: Vec<String>,
    pub description_chars: usize,
    pub description_headroom: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Visibility {
    pub model_visible: Vec<&'static str>,
    pub keyword_readers: Vec<String>,
    pub files_scanned: usize,
    pub roots: Vec<String>,
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnosis {
    pub skill: String,
    pub deployed_path: Option<String>,
    pub source_paths: Vec<String>,
    pub reasons: Vec<Reason>,
    pub lexical: Option<Lexical>,
    pub visibility: Visibility,
}

pub struct Input<'a> {
    pub skill: &'a str,
    pub deployed: &'a Path,
    pub workspace_root: &'a Path,
    pub source_homes: &'a [PathBuf],
    pub map_dirs: &'a [PathBuf],
    pub project_skills: Option<&'a Path>,
    pub plugin_roots: &'a [PathBuf],
    pub router_roots: &'a [PathBuf],
    pub log: &'a Path,
    pub now: i64,
    pub budget_chars: usize,
    pub max_desc_chars: usize,
    pub prompt: Option<&'a str>,
}

fn repo_of(source_skill_md: &Path) -> String {
    source_skill_md
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map_or_else(|| "<repo>".to_owned(), |n| n.to_string_lossy().into_owned())
}

fn rebuild_fix(input: &Input<'_>, source: Option<&Path>) -> String {
    let repo = source.map_or_else(|| "<repo>".to_owned(), repo_of);
    let nix = input.workspace_root.join("nix");
    format!("cd {} && nix flake update {repo} && nix run .#rebuild", nix.display())
}

fn first_diff_line(a: &str, b: &str) -> usize {
    a.lines().zip(b.lines()).position(|(x, y)| x != y).map_or_else(|| a.lines().count().min(b.lines().count()) + 1, |i| i + 1)
}


fn find_named(root: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(read) = std::fs::read_dir(root) else { return };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let dir = entry.file_name().to_string_lossy().into_owned();
        if matches!(dir.as_str(), ".git" | "node_modules" | "target") {
            continue;
        }
        if dir == name && path.join("SKILL.md").is_file() && root.file_name().is_some_and(|n| n == "skills") {
            out.push(path.join("SKILL.md"));
        }
        find_named(&path, name, depth - 1, out);
    }
}

#[must_use]
pub fn probe_keyword_readers(roots: &[PathBuf]) -> Visibility {
    let mut hits = BTreeSet::new();
    let mut scanned = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                if depth < ROUTER_WALK_DEPTH && !SKIPPED_DIRS.contains(&name.as_str()) {
                    stack.push((path, depth + 1));
                }
            } else if ft.is_file() && path.extension().is_some_and(|e| ROUTER_EXTENSIONS.iter().any(|x| e == *x)) {
                scanned += 1;
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if KEYWORD_NEEDLES.iter().any(|n| text.contains(n)) {
                        hits.insert(path.display().to_string());
                    }
                }
            }
        }
    }
    let readers: Vec<String> = hits.into_iter().collect();
    let verdict = if scanned == 0 {
        "unknown: no router or hook source was scanned, so nothing can be said about metadata.domain_keywords".to_owned()
    } else if readers.is_empty() {
        format!(
            "metadata.domain_keywords is read by no hook or router: 0 of {scanned} source files under the scanned roots mention it, so it never reaches the model; the listing carries name plus description"
        )
    } else {
        format!("metadata.domain_keywords is mentioned by {} source file(s); read them to see whether any routes on it", readers.len())
    };
    Visibility {
        model_visible: vec!["name", "description"],
        keyword_readers: readers,
        files_scanned: scanned,
        roots: roots.iter().map(|r| r.display().to_string()).collect(),
        verdict,
    }
}

fn desc_chars(doc: &SkillDoc) -> usize { doc.description.as_deref().map_or(0, |d| d.chars().count()) }

fn frontmatter_reasons(doc: &SkillDoc, dir_name: &str, source: Option<&Path>, out: &mut Vec<Reason>) {
    let edit = source.map_or_else(|| doc.path.display().to_string(), |s| s.display().to_string());
    if let Some(err) = &doc.parse_error {
        out.push(Reason {
            class: Class::FrontmatterInvalid,
            severity: Severity::Blocking,
            evidence: format!("frontmatter does not parse: {err}"),
            fix: format!("repair the YAML block at the top of {edit}, then run: skill-lint budget --strict --skills-dir {}", doc.path.parent().and_then(Path::parent).map_or_else(|| ".".into(), |p| p.display().to_string())),
        });
        return;
    }
    match doc.description.as_deref() {
        None | Some("") => out.push(Reason {
            class: Class::FrontmatterInvalid,
            severity: Severity::Blocking,
            evidence: "no description: the listing carries a bare name nothing can route to".into(),
            fix: format!("add a description of at most {DESCRIPTION_CAP} characters to {edit}"),
        }),
        Some(d) => {
            let n = d.chars().count();
            if n > DESCRIPTION_CAP {
                out.push(Reason {
                    class: Class::FrontmatterInvalid,
                    severity: Severity::Advisory,
                    evidence: format!("description is {n} chars, {} over the {DESCRIPTION_CAP}-char fleet cap", n - DESCRIPTION_CAP),
                    fix: format!("shorten the description in {edit} to {DESCRIPTION_CAP} chars or fewer"),
                });
            }
            let lower = d.to_ascii_lowercase();
            let hit: Vec<&str> = lower
                .split(|c: char| !c.is_ascii_alphabetic())
                .filter(|w| PRONOUNS.contains(w))
                .collect();
            if !hit.is_empty() {
                out.push(Reason {
                    class: Class::FrontmatterInvalid,
                    severity: Severity::Advisory,
                    evidence: format!("description is not third person: it uses {}", hit.join(", ")),
                    fix: format!("rewrite the description in {edit} as a verb phrase about the skill, with no first or second person"),
                });
            }
        }
    }
    if doc.name != dir_name {
        out.push(Reason {
            class: Class::FrontmatterInvalid,
            severity: Severity::Likely,
            evidence: format!("frontmatter name '{}' differs from its directory '{dir_name}'", doc.name),
            fix: format!("set name: {dir_name} in {edit}"),
        });
    }
}

fn shadow_reasons(input: &Input<'_>, sources: &[PathBuf], out: &mut Vec<Reason>) {
    let mut scoped = Vec::new();
    if let Some(p) = input.project_skills {
        let f = p.join(input.skill).join("SKILL.md");
        if f.is_file() {
            scoped.push(f);
        }
    }
    if sources.len() > 1 {
        out.push(Reason {
            class: Class::Shadowed,
            severity: Severity::Likely,
            evidence: format!(
                "{} skill source homes define '{}': {}",
                sources.len(),
                input.skill,
                sources.iter().map(|s| s.display().to_string()).collect::<Vec<_>>().join(", ")
            ),
            fix: "keep one definition and delete or rename the others, then rebuild".into(),
        });
    }
    if !scoped.is_empty() {
        out.push(Reason {
            class: Class::Shadowed,
            severity: Severity::Likely,
            evidence: format!(
                "the project scope also defines '{}' ({}) beside the user scope ({}): two definitions, the platform lists one",
                input.skill,
                scoped[0].display(),
                input.deployed.join(input.skill).join("SKILL.md").display()
            ),
            fix: format!("delete or rename {}", scoped[0].display()),
        });
    }
    let mut plugin = Vec::new();
    for root in input.plugin_roots {
        find_named(root, input.skill, PLUGIN_WALK_DEPTH, &mut plugin);
    }
    if !plugin.is_empty() {
        out.push(Reason {
            class: Class::Shadowed,
            severity: Severity::Advisory,
            evidence: format!(
                "a plugin ships a skill with the same bare name ({}); plugin skills are namespaced plugin:name, so confirm which one the prompt reached",
                plugin[0].display()
            ),
            fix: "rename one of the two, or disable the plugin that ships the duplicate".into(),
        });
    }
}

fn map_reasons(input: &Input<'_>, doc: &SkillDoc, source: Option<&Path>, out: &mut Vec<Reason>) {
    if input.map_dirs.is_empty() {
        return;
    }
    let mut known: BTreeSet<String> = BTreeSet::new();
    let mut entry = None;
    let mut domain_of = None;
    for dir in input.map_dirs {
        let src = FsSource { skills_dir: input.deployed, map_dir_override: Some(dir) };
        let Ok(map) = src.skill_map() else { continue };
        known.extend(map.skills.keys().cloned());
        if let Some(e) = map.skills.get(input.skill) {
            domain_of = map.domains.iter().find(|(_, members)| members.iter().any(|m| m == input.skill)).map(|(d, _)| d.clone());
            entry = Some((dir.clone(), e.clone()));
        }
    }
    let Some((dir, e)) = entry else {
        out.push(Reason {
            class: Class::MapIntegrity,
            severity: Severity::Likely,
            evidence: format!("no skill-map entry for '{}' in {} map dir(s)", input.skill, input.map_dirs.len()),
            fix: format!("add an entry to skill-map.d/<domain>.yaml in the owning repo, then run: skill-lint check --skills-dir {} --map-dir {}", input.deployed.display(), input.map_dirs[0].display()),
        });
        return;
    };
    if domain_of.as_deref().is_some_and(|d| d != e.domain) {
        out.push(Reason {
            class: Class::MapIntegrity,
            severity: Severity::Likely,
            evidence: format!("map entry says domain '{}' but lives in {}.yaml", e.domain, domain_of.unwrap_or_default()),
            fix: format!("set domain to match the file name in {}", dir.display()),
        });
    }
    let dead: Vec<&String> = e.references.iter().filter(|r| !known.contains(*r)).collect();
    if !dead.is_empty() {
        out.push(Reason {
            class: Class::MapIntegrity,
            severity: Severity::Likely,
            evidence: format!("map references name skills absent from the map: {}", dead.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
            fix: format!("remove or correct those references in {}", dir.display()),
        });
    }
    let base = source.and_then(Path::parent).or_else(|| doc.path.parent());
    if let Some(base) = base {
        let scan = scan_body(body_of(&doc.text));
        let dead_links: Vec<&str> = scan
            .refs
            .iter()
            .filter(|r| r.form == PathForm::RelativeLink && !scan.waived.contains(&r.path) && !base.join(&r.path).exists())
            .map(|r| r.path.as_str())
            .collect();
        if !dead_links.is_empty() {
            out.push(Reason {
                class: Class::MapIntegrity,
                severity: Severity::Likely,
                evidence: format!("body links point at files that do not exist: {}", dead_links.join(", ")),
                fix: format!("create those files under {} or fix the links", base.display()),
            });
        }
    }
}

/// Every reason that does not need a prompt.
///
/// # Errors
///
/// An unreadable deployed home or usage log.
pub fn structural(input: &Input<'_>) -> anyhow::Result<(Vec<Reason>, Option<SkillDoc>, Vec<PathBuf>)> {
    let mut reasons = Vec::new();
    let sources: Vec<PathBuf> = input
        .source_homes
        .iter()
        .map(|h| h.join(input.skill).join("SKILL.md"))
        .filter(|p| p.is_file())
        .collect();
    let deployed_md = input.deployed.join(input.skill).join("SKILL.md");
    let deployed_doc = route::load_doc(&input.deployed.join(input.skill));
    let dangling = deployed_doc.is_none() && std::fs::symlink_metadata(&deployed_md).is_ok();

    if deployed_doc.is_none() {
        if sources.is_empty() {
            reasons.push(Reason {
                class: Class::NotDeployed,
                severity: Severity::Blocking,
                evidence: format!(
                    "{} is absent and none of {} source homes defines '{}': the skill does not exist",
                    deployed_md.display(),
                    input.source_homes.len(),
                    input.skill
                ),
                fix: "create it with the claude-skills skill (skills/<name>/SKILL.md plus a skill-map.d entry), then rebuild".into(),
            });
        } else {
            let why = if dangling { "a dangling symlink" } else { "absent" };
            reasons.push(Reason {
                class: Class::NotDeployed,
                severity: Severity::Blocking,
                evidence: format!("{} is {why}; the source exists at {}", deployed_md.display(), sources[0].display()),
                fix: rebuild_fix(input, sources.first().map(PathBuf::as_path)),
            });
        }
    }

    let source_doc = sources.first().and_then(|s| s.parent()).and_then(route::load_doc);
    let doc = deployed_doc.clone().or_else(|| source_doc.clone());

    if let Some(deployed) = &deployed_doc {
        if let Some(src) = &source_doc {
            if deployed.text != src.text {
                reasons.push(Reason {
                    class: Class::StaleDeploy,
                    severity: Severity::Likely,
                    evidence: format!(
                        "deployed copy differs from {} (first difference at line {}): a push without a rebuild",
                        sources[0].display(),
                        first_diff_line(&deployed.text, &src.text)
                    ),
                    fix: rebuild_fix(input, sources.first().map(PathBuf::as_path)),
                });
            }
        }
    }

    if let Some(d) = &doc {
        frontmatter_reasons(d, input.skill, sources.first().map(PathBuf::as_path), &mut reasons);
        if d.flags.disable_model_invocation {
            reasons.push(Reason {
                class: Class::UserOnly,
                severity: Severity::Blocking,
                evidence: "frontmatter sets disable-model-invocation: the model never auto-invokes this skill; only a typed /name runs it".into(),
                fix: format!("remove disable-model-invocation from {}", sources.first().map_or_else(|| d.path.display().to_string(), |s| s.display().to_string())),
            });
        }
        if d.body_bytes > BODY_CAP_BYTES {
            reasons.push(Reason {
                class: Class::BodyOverCap,
                severity: Severity::Advisory,
                evidence: format!("SKILL.md is {} bytes, {} over the {BODY_CAP_BYTES}-byte cap", d.body_bytes, d.body_bytes - BODY_CAP_BYTES),
                fix: format!("move detail into references/ files; check with: skill-lint budget --strict --max-body-bytes {BODY_CAP_BYTES} --skills-dir {}", input.deployed.display()),
            });
        }
        if d.description.as_deref().is_some_and(|s| s.chars().count() > input.max_desc_chars) {
            reasons.push(Reason {
                class: Class::DescriptionTruncated,
                severity: Severity::Likely,
                evidence: format!("description is {} chars; the platform keeps {} and discards the rest", desc_chars(d), input.max_desc_chars),
                fix: "move trigger terms to the front of the description".into(),
            });
        }
        map_reasons(input, d, sources.first().map(PathBuf::as_path), &mut reasons);
    }

    shadow_reasons(input, &sources, &mut reasons);

    if deployed_doc.is_some() {
        let report = usage::report(&ReportInput {
            log: input.log,
            skills_dir: input.deployed,
            now: input.now,
            window: Window::Seconds(30 * 86_400),
            window_label: "30d",
            budget_chars: input.budget_chars,
            max_desc_chars: input.max_desc_chars,
        })?;
        if let Some(pos) = report.drop_order.iter().take(report.drops_needed).position(|e| e.skill == input.skill) {
            let e = &report.drop_order[pos];
            reasons.push(Reason {
                class: Class::DroppedByBudget,
                severity: Severity::Likely,
                evidence: format!(
                    "listing is {} chars against a {} budget; {} description(s) are dropped, least used first, and this skill is number {} with {} use(s) in 30d",
                    report.listing_chars, report.budget_chars, report.drops_needed, pos + 1, e.uses
                ),
                fix: format!(
                    "free {} chars: retire or shorten skills earlier in the drop order (skill-lint usage report --skills-dir {}), or invoke this skill so it ranks higher",
                    report.listing_chars - report.budget_chars,
                    input.deployed.display()
                ),
            });
        }
    }

    reasons.sort_by_key(|r| (r.severity, r.class));
    Ok((reasons, doc, sources))
}

fn lexical(input: &Input<'_>, prompt: &str, doc: &SkillDoc, source: Option<&Path>, reasons: &mut Vec<Reason>) -> Lexical {
    let mut docs = route::load_home(input.deployed);
    if !docs.iter().any(|d| d.name == doc.name) {
        docs.push(doc.clone());
    }
    let corpus = Corpus::build(&docs);
    let surface = route::surface_forms(prompt);
    let prompt_terms: BTreeSet<String> = surface.keys().cloned().collect();
    let show = |v: Vec<String>| -> Vec<String> { v.into_iter().map(|t| surface.get(&t).cloned().unwrap_or(t)).collect() };
    let ranked = corpus.rank(&prompt_terms);
    let me = corpus.score_of(&ranked, &doc.name);
    let known = prompt_terms.iter().filter(|t| corpus.knows(t)).count();
    let (rank, mine) = match me {
        Some((r, s)) => (Some(r), s),
        None => (
            None,
            Scored { skill: doc.name.clone(), permille: 0, matched: Vec::new(), keyword_only: Vec::new(), visible_matched_count: 0 },
        ),
    };
    let outranked_by: Vec<Outranker> = ranked
        .iter()
        .filter(|s| s.skill != doc.name && s.permille > mine.permille)
        .take(3)
        .map(|s| Outranker {
            skill: s.skill.clone(),
            permille: s.permille,
            terms_it_has_that_this_lacks: show(s.matched.iter().filter(|t| !mine.matched.contains(t)).cloned().collect()),
        })
        .collect();
    let missing: Vec<String> = show(corpus.missing_terms(&prompt_terms, doc).into_iter().take(6).collect());
    let invisible = show(corpus.invisible_keyword_terms(&prompt_terms, doc));
    let matched_shown = show(mine.matched.clone());
    let chars = desc_chars(doc);
    let headroom = DESCRIPTION_CAP.saturating_sub(chars);
    let edit = source.map_or_else(|| doc.path.display().to_string(), |s| s.display().to_string());

    if !outranked_by.is_empty() {
        let top = &outranked_by[0];
        reasons.push(Reason {
            class: Class::Outranked,
            severity: Severity::Likely,
            evidence: format!(
                "{} scores {}/1000 against this skill's {}/1000; it has terms this skill lacks: {}",
                top.skill,
                top.permille,
                mine.permille,
                if top.terms_it_has_that_this_lacks.is_empty() { "(none: it matches more weight with the same terms)".to_owned() } else { top.terms_it_has_that_this_lacks.join(", ") }
            ),
            fix: lexical_fix(&edit, &missing, headroom, &doc.keywords),
        });
    }
    if mine.permille < LEXICAL_GAP_PERMILLE && !missing.is_empty() {
        reasons.push(Reason {
            class: Class::LexicalGap,
            severity: Severity::Likely,
            evidence: format!(
                "name and description cover {}/1000 of the prompt's term weight; missing: {}",
                mine.permille,
                missing.join(", ")
            ),
            fix: lexical_fix(&edit, &missing, headroom, &doc.keywords),
        });
    }
    reasons.sort_by_key(|r| (r.severity, r.class));

    Lexical {
        prompt_terms: prompt_terms.len(),
        known_terms: known,
        permille: mine.permille,
        rank,
        of: ranked.len(),
        matched: matched_shown,
        outranked_by,
        candidate_terms: missing,
        keyword_only_hits: invisible,
        description_chars: chars,
        description_headroom: headroom,
    }
}

fn lexical_fix(edit: &str, missing: &[String], headroom: usize, keywords: &[String]) -> String {
    if missing.is_empty() {
        return format!("no unmatched prompt term to add; reword the description in {edit}");
    }
    let fits: Vec<&String> = {
        let mut room = headroom;
        missing
            .iter()
            .filter(|t| {
                let need = t.chars().count() + 1;
                if need <= room {
                    room -= need;
                    true
                } else {
                    false
                }
            })
            .collect()
    };
    let new_kw: Vec<&String> = missing.iter().filter(|t| !keywords.iter().any(|k| k.to_ascii_lowercase().contains(t.as_str()))).collect();
    let kw_note = if new_kw.is_empty() {
        String::new()
    } else {
        format!("; also list {} under metadata.domain_keywords (documentation only: no hook reads it)", new_kw.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
    };
    if fits.is_empty() {
        format!("the description is full ({}/{DESCRIPTION_CAP} chars): in {edit} swap a low-value word for {}{kw_note}", DESCRIPTION_CAP - headroom, missing[..missing.len().min(2)].join(" or "))
    } else {
        format!("in {edit} add to the description: {} ({headroom} chars of room){kw_note}", fits.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
    }
}

/// The full diagnosis of one skill.
///
/// # Errors
///
/// An unreadable usage log or skills home.
pub fn diagnose(input: &Input<'_>) -> anyhow::Result<Diagnosis> {
    let (mut reasons, doc, sources) = structural(input).with_context(|| format!("diagnosing {}", input.skill))?;
    let mut lex = None;
    if let (Some(prompt), Some(doc)) = (input.prompt, doc.as_ref()) {
        lex = Some(lexical(input, prompt, doc, sources.first().map(PathBuf::as_path), &mut reasons));
    }
    let deployed_md = input.deployed.join(input.skill).join("SKILL.md");
    Ok(Diagnosis {
        skill: input.skill.to_owned(),
        deployed_path: deployed_md.is_file().then(|| deployed_md.display().to_string()),
        source_paths: sources.iter().map(|s| s.display().to_string()).collect(),
        reasons,
        lexical: lex,
        visibility: probe_keyword_readers(input.router_roots),
    })
}

#[must_use]
pub fn render(d: &Diagnosis) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "why-not {}", d.skill);
    let _ = writeln!(out, "  deployed: {}", d.deployed_path.as_deref().unwrap_or("no"));
    for s in &d.source_paths {
        let _ = writeln!(out, "  source:   {s}");
    }
    if d.reasons.is_empty() {
        let _ = writeln!(out, "\nno reason found: the skill is deployed, valid, listed and (if a prompt was given) the best lexical match");
    }
    for (i, r) in d.reasons.iter().enumerate() {
        let sev = match r.severity {
            Severity::Blocking => "BLOCKING",
            Severity::Likely => "likely",
            Severity::Advisory => "advisory",
        };
        let _ = writeln!(out, "\n{}. [{sev}] {}", i + 1, r.class.as_str());
        let _ = writeln!(out, "   evidence: {}", r.evidence);
        let _ = writeln!(out, "   fix:      {}", r.fix);
    }
    match &d.lexical {
        Some(l) => {
            let rank = l.rank.map_or_else(|| "unranked".to_owned(), |r| format!("rank {r} of {}", l.of));
            let _ = writeln!(out, "\nprompt match (name + description; {} prompt terms, {} known to some skill)", l.prompt_terms, l.known_terms);
            let _ = writeln!(out, "  score {}/1000, {rank}; matched: {}", l.permille, join_or_none(&l.matched));
            for o in &l.outranked_by {
                let _ = writeln!(out, "  outranked by {} ({}/1000) via {}", o.skill, o.permille, join_or_none(&o.terms_it_has_that_this_lacks));
            }
            let _ = writeln!(out, "  candidate trigger terms: {}", join_or_none(&l.candidate_terms));
            let _ = writeln!(out, "  description: {}/{DESCRIPTION_CAP} chars, {} free", l.description_chars, l.description_headroom);
            if !l.keyword_only_hits.is_empty() {
                let _ = writeln!(out, "  matched only in domain_keywords (invisible to the model): {}", l.keyword_only_hits.join(", "));
            }
        }
        None => {
            let _ = writeln!(out, "\nprompt match: not run (no prompt given; pass --prompt-file or pipe stdin)");
        }
    }
    let _ = writeln!(out, "\nrouting surface: {}", d.visibility.verdict);
    let _ = writeln!(out, "  scanned {} files under {}", d.visibility.files_scanned, d.visibility.roots.join(", "));
    out
}

fn join_or_none(v: &[String]) -> String { if v.is_empty() { "(none)".to_owned() } else { v.join(", ") } }

#[derive(Debug, Clone)]
pub struct Paths {
    pub deployed: PathBuf,
    pub workspace_root: PathBuf,
    pub source_homes: Vec<PathBuf>,
    pub map_dirs: Vec<PathBuf>,
    pub plugin_roots: Vec<PathBuf>,
    pub router_roots: Vec<PathBuf>,
    pub project_skills: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub deployed: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub source_homes: Vec<PathBuf>,
    pub map_dirs: Vec<PathBuf>,
    pub plugin_roots: Vec<PathBuf>,
    pub router_roots: Vec<PathBuf>,
    pub project_skills: Option<PathBuf>,
}

impl Paths {
    /// Defaults: deployed `~/.claude/skills`, workspace `~/code/github/pleme-io`,
    /// source homes `<workspace>/*/skills`, map dirs `<workspace>/*/skill-map.d`,
    /// plugins `~/.claude/plugins`, router roots the claude, pleme and nix repos,
    /// project skills `<cwd>/.claude/skills`.
    ///
    /// # Errors
    ///
    /// No deployed home and `HOME` unset.
    pub fn resolve(o: Overrides, home: Option<&Path>, cwd: Option<&Path>) -> anyhow::Result<Self> {
        let deployed = match (o.deployed, home) {
            (Some(d), _) => d,
            (None, Some(h)) => h.join(".claude/skills"),
            (None, None) => anyhow::bail!("HOME is unset; pass --skills-dir"),
        };
        let workspace_root = match (o.workspace_root, home) {
            (Some(w), _) => w,
            (None, Some(h)) => h.join("code/github/pleme-io"),
            (None, None) => PathBuf::from("."),
        };
        let source_homes = if o.source_homes.is_empty() { crate::budget::discover_homes(&workspace_root) } else { o.source_homes };
        let map_dirs = if o.map_dirs.is_empty() {
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(&workspace_root)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|e| e.path().join("skill-map.d"))
                .filter(|p| p.is_dir())
                .collect();
            dirs.sort();
            dirs
        } else {
            o.map_dirs
        };
        let plugin_roots = if o.plugin_roots.is_empty() { home.map(|h| vec![h.join(".claude/plugins")]).unwrap_or_default() } else { o.plugin_roots };
        let router_roots = if o.router_roots.is_empty() {
            ["blackmatter-claude", "blackmatter-pleme", "nix"].iter().map(|r| workspace_root.join(r)).filter(|p| p.is_dir()).collect()
        } else {
            o.router_roots
        };
        let project_skills = o.project_skills.or_else(|| cwd.map(|c| c.join(".claude/skills")));
        Ok(Self { deployed, workspace_root, source_homes, map_dirs, plugin_roots, router_roots, project_skills })
    }

    #[must_use]
    pub fn input<'a>(&'a self, skill: &'a str, log: &'a Path, now: i64, budget_chars: usize, max_desc_chars: usize, prompt: Option<&'a str>) -> Input<'a> {
        Input {
            skill,
            deployed: &self.deployed,
            workspace_root: &self.workspace_root,
            source_homes: &self.source_homes,
            map_dirs: &self.map_dirs,
            project_skills: self.project_skills.as_deref(),
            plugin_roots: &self.plugin_roots,
            router_roots: &self.router_roots,
            log,
            now,
            budget_chars,
            max_desc_chars,
            prompt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Fx {
        root: TempDir,
    }

    impl Fx {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            for d in ["deployed", "ws/repo-a/skills", "ws/repo-a/skill-map.d", "router", "project"] {
                std::fs::create_dir_all(root.path().join(d)).unwrap();
            }
            Self { root }
        }

        fn p(&self, rel: &str) -> PathBuf { self.root.path().join(rel) }

        fn skill_text(name: &str, desc: &str, extra: &str, body: &str) -> String {
            format!("---\nname: {name}\ndescription: \"{desc}\"\n{extra}---\n{body}\n")
        }

        fn put(&self, home: &str, name: &str, text: &str) {
            let dir = self.p(home).join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), text).unwrap();
        }

        fn both(&self, name: &str, desc: &str) {
            let t = Self::skill_text(name, desc, "", "body");
            self.put("deployed", name, &t);
            self.put("ws/repo-a/skills", name, &t);
        }

        fn map(&self, entries: &[(&str, &[&str])]) {
            let mut y = String::new();
            for (n, refs) in entries {
                y.push_str(&format!("{n}:\n  description: d\n  domain: meta\n  repo: repo-a\n  references: [{}]\n", refs.join(", ")));
            }
            std::fs::write(self.p("ws/repo-a/skill-map.d/meta.yaml"), y).unwrap();
        }

        fn run(&self, skill: &str, prompt: Option<&str>, budget: usize) -> Diagnosis {
            let paths = Paths::resolve(
                Overrides {
                    deployed: Some(self.p("deployed")),
                    workspace_root: Some(self.p("ws")),
                    source_homes: vec![self.p("ws/repo-a/skills")],
                    map_dirs: if self.p("ws/repo-a/skill-map.d/meta.yaml").is_file() { vec![self.p("ws/repo-a/skill-map.d")] } else { vec![] },
                    plugin_roots: vec![self.p("plugins")],
                    router_roots: vec![self.p("router")],
                    project_skills: Some(self.p("project")),
                },
                None,
                None,
            )
            .unwrap();
            let log = self.p("usage.jsonl");
            let input = paths.input(skill, &log, usage::parse_rfc3339("2026-10-21T00:00:00Z").unwrap(), budget, 1536, prompt);
            diagnose(&input).unwrap()
        }
    }

    fn classes(d: &Diagnosis) -> Vec<Class> { d.reasons.iter().map(|r| r.class).collect() }

    #[test]
    fn a_source_that_was_never_deployed_is_blocking_and_names_the_rebuild() {
        let fx = Fx::new();
        fx.put("ws/repo-a/skills", "ghost", &Fx::skill_text("ghost", "Haunt things", "", "b"));
        let d = fx.run("ghost", None, 1_000_000);
        assert_eq!(d.reasons[0].class, Class::NotDeployed);
        assert_eq!(d.reasons[0].severity, Severity::Blocking);
        assert!(d.reasons[0].fix.contains("nix flake update repo-a && nix run .#rebuild"), "{}", d.reasons[0].fix);
    }

    #[test]
    fn a_skill_that_exists_nowhere_says_so() {
        let fx = Fx::new();
        let d = fx.run("nope", None, 1_000_000);
        assert_eq!(d.reasons[0].class, Class::NotDeployed);
        assert!(d.reasons[0].evidence.contains("does not exist"));
    }

    #[test]
    fn a_deployed_copy_that_differs_from_its_source_is_a_stale_deploy() {
        let fx = Fx::new();
        fx.both("old", "Tend old things");
        fx.put("ws/repo-a/skills", "old", &Fx::skill_text("old", "Tend old and new things", "", "changed"));
        let d = fx.run("old", None, 1_000_000);
        assert!(classes(&d).contains(&Class::StaleDeploy), "{:?}", d.reasons);
    }

    #[test]
    fn frontmatter_faults_are_found() {
        let fx = Fx::new();
        fx.put("deployed", "nodesc", "---\nname: nodesc\n---\nb\n");
        fx.put("deployed", "long", &Fx::skill_text("long", &"x".repeat(61), "", "b"));
        fx.put("deployed", "person", &Fx::skill_text("person", "Help you with things", "", "b"));
        fx.put("deployed", "wrongname", &Fx::skill_text("other", "Fine words", "", "b"));
        let d = fx.run("nodesc", None, 1_000_000);
        assert_eq!(d.reasons[0].class, Class::FrontmatterInvalid);
        assert_eq!(d.reasons[0].severity, Severity::Blocking);
        let d = fx.run("long", None, 1_000_000);
        assert!(d.reasons.iter().any(|r| r.class == Class::FrontmatterInvalid && r.evidence.contains("1 over the 60")));
        let d = fx.run("person", None, 1_000_000);
        assert!(d.reasons.iter().any(|r| r.evidence.contains("not third person")));
        let d = fx.run("wrongname", None, 1_000_000);
        assert!(d.reasons.iter().any(|r| r.evidence.contains("differs from its directory")));
    }

    #[test]
    fn a_user_only_flag_blocks_auto_invocation() {
        let fx = Fx::new();
        fx.put("deployed", "manual", &Fx::skill_text("manual", "Run by hand", "disable-model-invocation: true\n", "b"));
        let d = fx.run("manual", None, 1_000_000);
        assert_eq!(d.reasons[0].class, Class::UserOnly);
        assert_eq!(d.reasons[0].severity, Severity::Blocking);
    }

    #[test]
    fn allowed_tools_alone_never_stops_invocation() {
        let fx = Fx::new();
        fx.put("deployed", "tools", &Fx::skill_text("tools", "Use tools", "allowed-tools: Read\n", "b"));
        let d = fx.run("tools", None, 1_000_000);
        assert!(!classes(&d).contains(&Class::UserOnly));
    }

    #[test]
    fn the_least_used_skill_sits_in_the_drop_order_when_the_listing_overflows() {
        let fx = Fx::new();
        fx.both("busy", "Does busy work all day");
        fx.both("idle", "Does nothing at all");
        let usage = r#"{"ts":"2026-10-20T00:00:00Z","event":"PreToolUse","skill":"busy","trigger":"tool","session_id":"s","cwd":null}"#;
        std::fs::write(fx.p("usage.jsonl"), format!("{usage}\n")).unwrap();
        let d = fx.run("idle", None, 40);
        let r = d.reasons.iter().find(|r| r.class == Class::DroppedByBudget).expect("dropped");
        assert!(r.evidence.contains("number 1 with 0 use(s)"), "{}", r.evidence);
        let d = fx.run("busy", None, 40);
        assert!(!classes(&d).contains(&Class::DroppedByBudget));
        let d = fx.run("idle", None, 1_000_000);
        assert!(!classes(&d).contains(&Class::DroppedByBudget));
    }

    #[test]
    fn a_description_past_the_platform_cap_is_truncated_text() {
        let fx = Fx::new();
        fx.both("wordy", &"word ".repeat(40));
        let paths = Paths::resolve(
            Overrides { deployed: Some(fx.p("deployed")), workspace_root: Some(fx.p("ws")), source_homes: vec![fx.p("ws/repo-a/skills")], router_roots: vec![fx.p("router")], ..Overrides::default() },
            None,
            None,
        )
        .unwrap();
        let log = fx.p("usage.jsonl");
        let input = paths.input("wordy", &log, 0, 1_000_000, 50, None);
        assert!(diagnose(&input).unwrap().reasons.iter().any(|r| r.class == Class::DescriptionTruncated));
    }

    #[test]
    fn two_definitions_in_two_scopes_are_a_collision() {
        let fx = Fx::new();
        fx.both("twin", "Exists twice");
        fx.put("project", "twin", &Fx::skill_text("twin", "Exists twice", "", "b"));
        let d = fx.run("twin", None, 1_000_000);
        assert!(classes(&d).contains(&Class::Shadowed));
    }

    #[test]
    fn a_same_named_plugin_skill_is_reported_as_advisory() {
        let fx = Fx::new();
        fx.both("dual", "Exists in a plugin too");
        let plugin = fx.p("plugins/vendor/skills/dual");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(plugin.join("SKILL.md"), "---\nname: dual\n---\n").unwrap();
        let d = fx.run("dual", None, 1_000_000);
        let r = d.reasons.iter().find(|r| r.class == Class::Shadowed).expect("plugin");
        assert_eq!(r.severity, Severity::Advisory);
    }

    #[test]
    fn map_entry_faults_are_found() {
        let fx = Fx::new();
        fx.both("mapped", "Has a map entry");
        fx.both("unmapped", "Has none");
        fx.map(&[("mapped", &["ghost-ref"])]);
        let d = fx.run("unmapped", None, 1_000_000);
        assert!(d.reasons.iter().any(|r| r.class == Class::MapIntegrity && r.evidence.contains("no skill-map entry")));
        let d = fx.run("mapped", None, 1_000_000);
        assert!(d.reasons.iter().any(|r| r.class == Class::MapIntegrity && r.evidence.contains("ghost-ref")));
    }

    #[test]
    fn a_dead_body_link_is_a_reference_fault() {
        let fx = Fx::new();
        let t = Fx::skill_text("linky", "Links around", "", "see [x](./references/missing.md)");
        fx.put("deployed", "linky", &t);
        fx.put("ws/repo-a/skills", "linky", &t);
        fx.map(&[("linky", &[])]);
        let d = fx.run("linky", None, 1_000_000);
        assert!(d.reasons.iter().any(|r| r.evidence.contains("references/missing.md")));
    }

    #[test]
    fn a_body_past_twelve_thousand_bytes_is_flagged() {
        let fx = Fx::new();
        let t = Fx::skill_text("fat", "Very large body", "", &"x".repeat(12_100));
        fx.put("deployed", "fat", &t);
        let d = fx.run("fat", None, 1_000_000);
        assert!(classes(&d).contains(&Class::BodyOverCap));
    }

    #[test]
    fn a_prompt_the_description_does_not_cover_is_a_lexical_gap_with_candidate_terms() {
        let fx = Fx::new();
        fx.both("flowers", "Tend the garden");
        fx.both("helm", "Create Helm charts and releases for kubernetes");
        let d = fx.run("flowers", Some("create a helm chart release for kubernetes"), 1_000_000);
        assert!(classes(&d).contains(&Class::LexicalGap), "{:?}", d.reasons);
        let l = d.lexical.unwrap();
        assert!(l.candidate_terms.contains(&"helm".to_owned()) && l.candidate_terms.contains(&"kubernetes".to_owned()), "{:?}", l.candidate_terms);
    }

    #[test]
    fn the_competitor_that_outranks_is_named_with_its_terms() {
        let fx = Fx::new();
        fx.both("weak", "Create things");
        fx.both("strong", "Create Helm charts and releases");
        let d = fx.run("weak", Some("create helm charts releases"), 1_000_000);
        let r = d.reasons.iter().find(|r| r.class == Class::Outranked).expect("outranked");
        assert!(r.evidence.starts_with("strong scores"), "{}", r.evidence);
        let l = d.lexical.unwrap();
        assert_eq!(l.rank, Some(2));
        assert!(l.outranked_by[0].terms_it_has_that_this_lacks.contains(&"helm".to_owned()));
    }

    #[test]
    fn a_full_description_gets_a_swap_suggestion_and_a_keyword_that_is_marked_inert() {
        let fx = Fx::new();
        let full = "Create Helm charts via pleme-lib OCI releases and values.";
        fx.both("fullone", full);
        fx.both("other", "Rust kubernetes ingress tool");
        let d = fx.run("fullone", Some("kubernetes ingress"), 1_000_000);
        let fix = &d.reasons.iter().find(|r| r.class == Class::LexicalGap).expect("gap").fix;
        assert!(fix.contains("swap a low-value word") || fix.contains("add to the description"), "{fix}");
        assert!(fix.contains("domain_keywords (documentation only"), "{fix}");
    }

    #[test]
    fn keyword_matches_never_reach_the_visible_score() {
        let fx = Fx::new();
        let t = Fx::skill_text("kw", "Plain words only", "metadata:\n  domain_keywords:\n    - kubernetes ingress\n", "b");
        fx.put("deployed", "kw", &t);
        fx.both("filler", "Filler skill");
        let d = fx.run("kw", Some("kubernetes ingress"), 1_000_000);
        let l = d.lexical.unwrap();
        assert_eq!(l.permille, 0);
        assert_eq!(l.keyword_only_hits.len(), 2);
    }

    #[test]
    fn the_keyword_probe_reports_readers_and_the_denominator() {
        let fx = Fx::new();
        std::fs::write(fx.p("router/hook.nix"), "{ x = 1; }").unwrap();
        let v = probe_keyword_readers(&[fx.p("router")]);
        assert_eq!(v.files_scanned, 1);
        assert!(v.keyword_readers.is_empty());
        assert!(v.verdict.contains("never reaches the model"));
        std::fs::write(fx.p("router/route.nix"), "builtins.readFile domain_keywords").unwrap();
        let v = probe_keyword_readers(&[fx.p("router")]);
        assert_eq!(v.keyword_readers.len(), 1);
        assert!(!v.verdict.contains("never reaches"));
        assert!(probe_keyword_readers(&[fx.p("absent")]).verdict.starts_with("unknown"));
    }

    #[test]
    fn a_healthy_best_match_reports_no_reason() {
        let fx = Fx::new();
        fx.both("solo", "Create Helm charts");
        fx.both("filler", "Something else");
        fx.map(&[("solo", &[]), ("filler", &[])]);
        let d = fx.run("solo", Some("create helm charts"), 1_000_000);
        assert!(d.reasons.is_empty(), "{:?}", d.reasons);
        assert_eq!(d.lexical.unwrap().rank, Some(1));
    }

    #[test]
    fn reasons_are_ranked_blocking_first() {
        let fx = Fx::new();
        fx.put("deployed", "mix", &Fx::skill_text("mix", "Run by hand", "disable-model-invocation: true\n", &"x".repeat(12_100)));
        let d = fx.run("mix", None, 1_000_000);
        let sev: Vec<Severity> = d.reasons.iter().map(|r| r.severity).collect();
        let mut sorted = sev.clone();
        sorted.sort();
        assert_eq!(sev, sorted);
        assert_eq!(d.reasons[0].severity, Severity::Blocking);
    }
}
