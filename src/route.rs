use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::budget;
use crate::model;

pub const STRONG_PERMILLE: u32 = 400;
pub const MIN_MATCHED_TERMS: usize = 2;
pub const TOP_CANDIDATES: usize = 3;
pub const MAX_PROMPT_BYTES: usize = 1 << 20;

const MIN_TERM_LEN: usize = 3;
const MAX_TERM_LEN: usize = 24;
const SECRET_RAW_LEN: usize = 20;

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "onto", "are", "was", "were", "has", "have", "had", "not", "but", "you",
    "your", "can", "will", "would", "should", "could", "its", "our", "out", "all", "any", "use", "using", "used", "when", "then",
    "than", "them", "they", "their", "there", "what", "which", "who", "how", "why", "does", "did", "get", "got", "let", "make", "need",
    "want", "like", "just", "now", "also", "via", "per", "one", "two", "too", "very", "some", "more", "most", "each", "every", "about",
    "over", "under", "after", "before", "please", "thanks", "okay", "yes", "run", "see", "show", "look",
];

const SECRET_PREFIXES: &[&str] = &["sk-", "sk_", "ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "xox", "akia", "asia", "eyj", "glpat-", "npm_", "pk_", "rk_"];

#[must_use]
pub fn looks_secret(raw: &str) -> bool {
    let t = raw.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let lower = t.to_ascii_lowercase();
    if SECRET_PREFIXES.iter().any(|p| lower.starts_with(p)) && t.len() >= 8 {
        return true;
    }
    if t.len() < SECRET_RAW_LEN {
        return false;
    }
    let upper = t.chars().any(|c| c.is_ascii_uppercase());
    let lowerc = t.chars().any(|c| c.is_ascii_lowercase());
    let digit = t.chars().any(|c| c.is_ascii_digit());
    let classes = usize::from(upper) + usize::from(lowerc) + usize::from(digit);
    let hexish = t.chars().all(|c| c.is_ascii_hexdigit());
    (classes >= 2 && !t.contains(' ')) || hexish
}

fn stem(word: &str) -> String {
    let w = word;
    if w.len() > 4 && w.ends_with("ies") {
        return format!("{}y", &w[..w.len() - 3]);
    }
    if w.len() > 4 && (w.ends_with("sses") || w.ends_with("xes") || w.ends_with("ches") || w.ends_with("shes")) {
        return w[..w.len() - 2].to_owned();
    }
    if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") {
        return w[..w.len() - 1].to_owned();
    }
    w.to_owned()
}

#[must_use]
pub fn terms(text: &str) -> BTreeSet<String> { surface_forms(text).into_keys().collect() }

#[must_use]
pub fn surface_forms(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for raw in text.split_whitespace() {
        if looks_secret(raw) {
            continue;
        }
        for piece in raw.split(|c: char| !c.is_ascii_alphanumeric()) {
            if piece.len() < MIN_TERM_LEN || piece.len() > MAX_TERM_LEN || piece.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let lower = piece.to_ascii_lowercase();
            if STOPWORDS.contains(&lower.as_str()) {
                continue;
            }
            let has_digit = lower.bytes().any(|b| b.is_ascii_digit());
            let has_alpha = lower.bytes().any(|b| b.is_ascii_alphabetic());
            if has_digit && has_alpha && lower.len() >= 12 {
                continue;
            }
            let s = stem(&lower);
            if s.len() >= MIN_TERM_LEN && !STOPWORDS.contains(&s.as_str()) {
                out.entry(s).or_insert(lower);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct Flags {
    pub disable_model_invocation: bool,
    pub user_invocable_false: bool,
    pub allowed_tools: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SkillDoc {
    pub name: String,
    pub dir_name: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    pub flags: Flags,
    pub body_bytes: usize,
    pub parse_error: Option<String>,
    pub text: String,
}

impl SkillDoc {
    #[must_use]
    pub fn visible_terms(&self) -> BTreeSet<String> {
        let mut t = terms(&self.name.replace(['-', '_', ':'], " "));
        if let Some(d) = &self.description {
            t.extend(terms(d));
        }
        t
    }

    #[must_use]
    pub fn keyword_terms(&self) -> BTreeSet<String> {
        let mut t = BTreeSet::new();
        for k in &self.keywords {
            t.extend(terms(k));
        }
        t
    }

    #[must_use]
    pub fn model_invocable(&self) -> bool { !self.flags.disable_model_invocation }
}

fn flags_of(content: &str) -> Flags {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else { return Flags::default() };
    let Some(end) = rest.find("\n---") else { return Flags::default() };
    let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&rest[..end]) else {
        return Flags::default();
    };
    let truthy = |keys: &[&str]| keys.iter().any(|k| map.get(*k).is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true")));
    let falsy = |keys: &[&str]| keys.iter().any(|k| map.get(*k).is_some_and(|v| v.as_bool() == Some(false) || v.as_str() == Some("false")));
    let allowed = ["allowed-tools", "allowed_tools"].iter().find_map(|k| map.get(*k)).map(|v| match v {
        serde_yaml_ng::Value::String(s) => s.clone(),
        other => serde_yaml_ng::to_string(other).unwrap_or_default().trim().to_owned(),
    });
    Flags {
        disable_model_invocation: truthy(&["disable-model-invocation", "disable_model_invocation"]),
        user_invocable_false: falsy(&["user-invocable", "user_invocable"]),
        allowed_tools: allowed,
    }
}

#[must_use]
pub fn load_doc(skill_dir: &Path) -> Option<SkillDoc> {
    let path = skill_dir.join("SKILL.md");
    let text = std::fs::read_to_string(&path).ok()?;
    let dir_name = skill_dir.file_name()?.to_string_lossy().into_owned();
    let (fm, parse_error) = match model::parse_frontmatter(&text) {
        Ok(fm) => (fm, None),
        Err(e) => (model::SkillFrontmatter::default(), Some(e.to_string())),
    };
    let keywords = fm.metadata.as_ref().map(|m| m.domain_keywords.clone()).unwrap_or_default();
    Some(SkillDoc {
        name: fm.name.clone().unwrap_or_else(|| dir_name.clone()),
        dir_name,
        path,
        description: fm.description.as_deref().map(budget::fold),
        keywords,
        flags: flags_of(&text),
        body_bytes: text.len(),
        parse_error,
        text,
    })
}

#[must_use]
pub fn load_home(home: &Path) -> Vec<SkillDoc> {
    let Ok(read) = std::fs::read_dir(home) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.iter().filter_map(|d| load_doc(d)).collect()
}

pub struct Corpus {
    docs: Vec<Indexed>,
    df: BTreeMap<String, usize>,
}

struct Indexed {
    name: String,
    invocable: bool,
    visible: BTreeSet<String>,
    keywords: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scored {
    pub skill: String,
    pub permille: u32,
    pub matched: Vec<String>,
    pub keyword_only: Vec<String>,
    pub visible_matched_count: usize,
}

impl Corpus {
    #[must_use]
    pub fn build(docs: &[SkillDoc]) -> Self {
        let indexed: Vec<Indexed> = docs
            .iter()
            .map(|d| Indexed {
                name: d.name.clone(),
                invocable: d.model_invocable(),
                visible: d.visible_terms(),
                keywords: d.keyword_terms(),
            })
            .collect();
        let mut df: BTreeMap<String, usize> = BTreeMap::new();
        for d in &indexed {
            for t in d.visible.union(&d.keywords) {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
        }
        Self { docs: indexed, df }
    }

    #[must_use]
    pub fn len(&self) -> usize { self.docs.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.docs.is_empty() }

    #[must_use]
    pub fn knows(&self, term: &str) -> bool { self.df.contains_key(term) }

    #[allow(clippy::cast_precision_loss)]
    fn idf(&self, term: &str) -> f64 {
        let n = self.docs.len() as f64;
        let df = self.df.get(term).copied().unwrap_or(0).max(1) as f64;
        (1.0 + n / df).ln()
    }

    #[must_use]
    pub fn rank(&self, prompt_terms: &BTreeSet<String>) -> Vec<Scored> {
        let known: Vec<&String> = prompt_terms.iter().filter(|t| self.knows(t)).collect();
        let total: f64 = known.iter().map(|t| self.idf(t)).sum();
        let mut out: Vec<Scored> = self
            .docs
            .iter()
            .filter(|d| d.invocable)
            .map(|d| {
                let mut got = 0.0;
                let mut matched = Vec::new();
                let mut keyword_only = Vec::new();
                for t in &known {
                    if d.visible.contains(*t) {
                        got += self.idf(t);
                        matched.push((*t).clone());
                    } else if d.keywords.contains(*t) {
                        keyword_only.push((*t).clone());
                    }
                }
                Scored {
                    skill: d.name.clone(),
                    permille: permille(got, total),
                    visible_matched_count: matched.len(),
                    matched,
                    keyword_only,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.permille
                .cmp(&a.permille)
                .then_with(|| b.visible_matched_count.cmp(&a.visible_matched_count))
                .then_with(|| a.skill.cmp(&b.skill))
        });
        out
    }

    #[must_use]
    pub fn score_of(&self, ranked: &[Scored], skill: &str) -> Option<(usize, Scored)> {
        ranked.iter().position(|s| s.skill == skill).map(|i| (i + 1, ranked[i].clone()))
    }

    #[must_use]
    pub fn missing_terms(&self, prompt_terms: &BTreeSet<String>, doc: &SkillDoc) -> Vec<String> {
        let vis = doc.visible_terms();
        let kw = doc.keyword_terms();
        let mut missing: Vec<(f64, String)> = prompt_terms
            .iter()
            .filter(|t| self.knows(t) && !vis.contains(*t) && !kw.contains(*t))
            .map(|t| (self.idf(t), t.clone()))
            .collect();
        missing.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        missing.into_iter().map(|(_, t)| t).collect()
    }

    #[must_use]
    pub fn invisible_keyword_terms(&self, prompt_terms: &BTreeSet<String>, doc: &SkillDoc) -> Vec<String> {
        let vis = doc.visible_terms();
        let kw = doc.keyword_terms();
        prompt_terms.iter().filter(|t| kw.contains(*t) && !vis.contains(*t)).cloned().collect()
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn permille(got: f64, total: f64) -> u32 {
    if total <= 0.0 {
        return 0;
    }
    ((got / total) * 1000.0).round().clamp(0.0, 1000.0) as u32
}

#[must_use]
pub fn candidates(corpus: &Corpus, prompt: &str) -> Vec<Scored> {
    let pt = terms(prompt);
    corpus
        .rank(&pt)
        .into_iter()
        .filter(|s| s.permille > 0 && s.visible_matched_count >= MIN_MATCHED_TERMS)
        .take(TOP_CANDIDATES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, desc: &str, kws: &[&str]) -> SkillDoc {
        SkillDoc {
            name: name.into(),
            dir_name: name.into(),
            path: PathBuf::new(),
            description: Some(desc.into()),
            keywords: kws.iter().map(|s| (*s).to_owned()).collect(),
            flags: Flags::default(),
            body_bytes: 0,
            parse_error: None,
            text: String::new(),
        }
    }

    #[test]
    fn terms_drop_stopwords_short_words_and_stem_plurals() {
        let t = terms("Please fix the flaky tests and charts");
        assert!(t.contains("flaky") && t.contains("test") && t.contains("chart") && t.contains("fix"));
        assert!(!t.contains("the") && !t.contains("and") && !t.contains("please"));
    }

    #[test]
    fn secret_shaped_tokens_never_become_terms() {
        let fake = concat!("ghp", "_FAKEFAKEFAKEFAKEFAKEFAKEFAKE0123456789");
        let aws = concat!("AKI", "AFAKEFAKEFAKE1234");
        let hex = "0123456789abcdef0123456789abcdef";
        let t = terms(&format!("deploy {fake} {aws} {hex} sk-ant-api03-ZZZZZZZZZZZZZZZZ chart"));
        assert_eq!(t.iter().cloned().collect::<Vec<_>>(), ["chart", "deploy"]);
    }

    #[test]
    fn the_scorer_ranks_the_covering_skill_first() {
        let docs = vec![
            doc("helm-charts", "Create Helm charts and releases", &[]),
            doc("rust-tool", "Rust binary or library", &[]),
        ];
        let c = Corpus::build(&docs);
        let r = c.rank(&terms("create a helm chart release"));
        assert_eq!(r[0].skill, "helm-charts");
        assert!(r[0].permille > r[1].permille);
    }

    #[test]
    fn keywords_are_reported_apart_and_never_lift_the_visible_score() {
        let docs = vec![doc("a", "Alpha thing", &["kubernetes cluster"]), doc("b", "Other thing", &[])];
        let c = Corpus::build(&docs);
        let r = c.rank(&terms("kubernetes cluster"));
        let a = r.iter().find(|s| s.skill == "a").unwrap();
        assert_eq!(a.permille, 0);
        assert_eq!(a.keyword_only, ["cluster".to_owned(), "kubernete".to_owned()]);
    }
}
