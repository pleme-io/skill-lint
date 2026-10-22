use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::chain::CHAIN_FILE_NAMES;
use crate::error::{CheckKind, LintError};

pub const WAIVER_PREFIX: &str = "skip-context-budget:";

const SKIPPED_DIRS: [&str; 1] = ["node_modules"];
const SKIPPED_PREFIXES: [&str; 3] = [".", "target", "result"];

#[must_use]
pub fn has_chain_file(dir: &Path) -> bool {
    CHAIN_FILE_NAMES.iter().any(|name| dir.join(name).is_file())
}

#[must_use]
pub fn sessions_under(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.to_path_buf()];
    let mut seen = BTreeSet::new();
    seen.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));
    descend(root, &mut seen, &mut found);
    found.sort();
    found.dedup();
    found
}

fn skipped(name: &str) -> bool {
    SKIPPED_DIRS.contains(&name) || SKIPPED_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn descend(dir: &Path, seen: &mut BTreeSet<PathBuf>, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut children: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| !skipped(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    for child in children {
        let canonical = std::fs::canonicalize(&child).unwrap_or_else(|_| child.clone());
        if !seen.insert(canonical) {
            continue;
        }
        if has_chain_file(&child) {
            found.push(child.clone());
        }
        descend(&child, seen, found);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiver {
    pub key: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct Waivers {
    pub entries: Vec<Waiver>,
}

impl Waivers {
    #[must_use]
    pub fn parse(text: &str) -> (Self, Vec<LintError>) {
        let mut entries = Vec::new();
        let mut errors = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parsed = line
                .strip_prefix(WAIVER_PREFIX)
                .map(str::trim)
                .and_then(|rest| rest.split_once(char::is_whitespace))
                .map(|(key, reason)| (key.trim(), reason.trim()))
                .filter(|(key, reason)| !key.is_empty() && !reason.is_empty());
            match parsed {
                Some((key, reason)) => {
                    entries.push(Waiver { key: key.trim_end_matches('/').to_owned(), reason: reason.to_owned() });
                }
                None => errors.push(LintError::ChainWaiverMalformed {
                    kind: CheckKind::ClaudeMdChain,
                    line: index + 1,
                    text: line.to_owned(),
                }),
            }
        }
        (Self { entries }, errors)
    }

    #[must_use]
    pub fn covering(&self, key: &str) -> Option<&Waiver> {
        self.entries.iter().find(|w| Path::new(key).starts_with(&w.key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiver_covers_its_scope_and_everything_under_it() {
        let (w, errors) = Waivers::parse("# x\nskip-context-budget: ~/code/v vendored upstream, not ours\n");
        assert!(errors.is_empty());
        assert!(w.covering("~/code/v").is_some());
        assert!(w.covering("~/code/v/sub").is_some());
        assert!(w.covering("~/code/vx").is_none());
    }

    #[test]
    fn a_waiver_without_a_reason_is_refused() {
        let (w, errors) = Waivers::parse("skip-context-budget: ~/code/v\nskip-other: ~/a b\n");
        assert!(w.entries.is_empty());
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn discovery_finds_the_root_and_every_dir_with_a_chain_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        for dir in ["a", "a/deep", "b", "c/.claude", "d/target", "e/.git"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        for file in ["a/CLAUDE.md", "a/deep/CLAUDE.local.md", "c/.claude/CLAUDE.md", "d/target/CLAUDE.md", "e/.git/CLAUDE.md"] {
            std::fs::write(root.join(file), "x").expect("write");
        }
        let got: Vec<PathBuf> =
            sessions_under(root).iter().map(|p| p.strip_prefix(root).expect("under root").to_path_buf()).collect();
        assert_eq!(got, [PathBuf::new(), PathBuf::from("a"), PathBuf::from("a/deep"), PathBuf::from("c")]);
    }
}
