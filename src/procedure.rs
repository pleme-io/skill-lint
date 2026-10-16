use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::markdown::{Fences, LineKind};
use crate::ratchet::{Ratchet, Verdict, parse_lines};

pub const DEFAULT_MAX_SHELL_LINES: usize = 3;
pub const DEFAULT_MAX_STEPS: usize = 7;

const SHELL_TAGS: &[&str] = &["bash", "sh", "shell", "zsh", "console", "fish", "nu", "frostmourne"];
const KIND_SHELL: &str = "shell";
const KIND_STEPS: &str = "steps";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcedureConfig {
    pub max_shell_lines: usize,
    pub max_steps: usize,
}

impl Default for ProcedureConfig {
    fn default() -> Self { Self { max_shell_lines: DEFAULT_MAX_SHELL_LINES, max_steps: DEFAULT_MAX_STEPS } }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellBlock {
    pub line: usize,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillScan {
    pub name: String,
    pub path: PathBuf,
    pub shell_blocks: Vec<ShellBlock>,
    pub longest_steps: usize,
}

impl SkillScan {
    #[must_use]
    pub fn shell_over(&self, config: &ProcedureConfig) -> usize {
        self.shell_blocks.iter().filter(|b| b.lines > config.max_shell_lines).map(|b| b.lines).sum()
    }

    #[must_use]
    pub fn steps_over(&self, config: &ProcedureConfig) -> usize {
        if self.longest_steps > config.max_steps { self.longest_steps } else { 0 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Baseline {
    shell: Ratchet,
    steps: Ratchet,
}

impl Baseline {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading baseline {}: {e}", path.display()))?;
        Ok(Self::parse(&text))
    }

    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for (kind, key, size) in parse_lines(text) {
            match kind.as_str() {
                KIND_SHELL => out.shell.insert(key, size),
                KIND_STEPS => out.steps.insert(key, size),
                _ => {}
            }
        }
        out
    }

    #[must_use]
    pub fn render(scans: &[SkillScan], config: &ProcedureConfig) -> String {
        let mut text = format!(
            "# skill-lint procedure baseline: known debt, may only shrink\n# shell over {} lines per fence, numbered runs over {} steps\n",
            config.max_shell_lines, config.max_steps
        );
        for scan in scans {
            let shell = scan.shell_over(config);
            if shell > 0 {
                let _ = writeln!(text, "{KIND_SHELL}: {} {shell}", scan.name);
            }
            let steps = scan.steps_over(config);
            if steps > 0 {
                let _ = writeln!(text, "{KIND_STEPS}: {} {steps}", scan.name);
            }
        }
        text
    }
}

#[derive(Debug, Clone)]
pub struct ProcedureReport {
    pub homes: Vec<String>,
    pub scans: Vec<SkillScan>,
    pub errors: Vec<String>,
}

impl ProcedureReport {
    #[must_use]
    pub fn is_ok(&self) -> bool { self.errors.is_empty() }
}

fn body(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---") else { return content };
    match rest.find("\n---") {
        Some(end) => rest[end + 4..].split_once('\n').map_or("", |(_, b)| b),
        None => content,
    }
}

fn is_step(line: &str) -> bool {
    let indent = line.len() - line.trim_start().len();
    if indent > 3 {
        return false;
    }
    let t = line.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && matches!(t[digits..].chars().next(), Some('.' | ')')) && t[digits + 1..].starts_with(' ')
}

#[must_use]
pub fn scan_text(content: &str) -> (Vec<ShellBlock>, usize) {
    let mut fences = Fences::new();
    let mut open: Option<(bool, usize, usize)> = None;
    let mut blocks = Vec::new();
    let mut run = 0usize;
    let mut longest = 0usize;
    for (index, line) in body(content).lines().enumerate() {
        match fences.classify(line) {
            LineKind::Fence => match open.take() {
                None => {
                    let tag = line.trim_start().trim_start_matches(['`', '~']).trim();
                    let tag = tag.split(|c: char| c.is_whitespace() || c == '{').next().unwrap_or("");
                    open = Some((SHELL_TAGS.contains(&tag.to_ascii_lowercase().as_str()), index + 1, 0));
                }
                Some((true, start, lines)) => blocks.push(ShellBlock { line: start, lines }),
                Some(_) => {}
            },
            LineKind::Fenced => {
                if let Some((true, _, lines)) = open.as_mut() {
                    let t = line.trim();
                    if !t.is_empty() && !t.starts_with('#') {
                        *lines += 1;
                    }
                }
            }
            LineKind::Prose => {
                if is_step(line) {
                    run += 1;
                    longest = longest.max(run);
                } else if !line.trim().is_empty() && !line.starts_with([' ', '\t']) {
                    run = 0;
                }
            }
        }
    }
    (blocks, longest)
}

pub fn scan_homes(homes: &[PathBuf]) -> anyhow::Result<(Vec<String>, Vec<SkillScan>)> {
    let mut by_name: BTreeMap<String, SkillScan> = BTreeMap::new();
    let mut scanned = Vec::new();
    for home in homes {
        scanned.push(home.display().to_string());
        let read = std::fs::read_dir(home).map_err(|e| anyhow::anyhow!("reading {}: {e}", home.display()))?;
        let mut dirs: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let path = dir.join("SKILL.md");
            if !path.is_file() {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
            let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let (shell_blocks, longest_steps) = scan_text(&content);
            by_name.insert(name.clone(), SkillScan { name, path, shell_blocks, longest_steps });
        }
    }
    Ok((scanned, by_name.into_values().collect()))
}

fn judge(errors: &mut Vec<String>, ratchet: &Ratchet, what: &str, name: &str, measured: usize, hint: &str) {
    if measured == 0 {
        return;
    }
    match ratchet.judge(name, measured) {
        Verdict::Held => {}
        Verdict::Unrecorded => errors.push(format!("{name}: {measured} {what}, not in the baseline; {hint}")),
        Verdict::Grew { recorded, grew } => {
            errors.push(format!("{name}: {what} grew {recorded} -> {measured} (+{grew}); {hint}"));
        }
    }
}

pub fn lint(homes: &[PathBuf], config: &ProcedureConfig, known: &Baseline) -> anyhow::Result<ProcedureReport> {
    let (homes, scans) = scan_homes(homes)?;
    let mut errors = Vec::new();
    if scans.is_empty() {
        errors.push(format!("no SKILL.md found under {} home(s): a vacuous pass is a failure", homes.len()));
    }
    for scan in &scans {
        judge(
            &mut errors,
            &known.shell,
            &format!("lines of shell in fences over {} lines", config.max_shell_lines),
            &scan.name,
            scan.shell_over(config),
            "move the logic into a typed tool and invoke it in one line",
        );
        judge(
            &mut errors,
            &known.steps,
            &format!("numbered steps in one run (max {})", config.max_steps),
            &scan.name,
            scan.steps_over(config),
            "the procedure belongs in a typed program the skill invokes",
        );
    }
    Ok(ProcedureReport { homes, scans, errors })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_shell_fence_counts_and_comments_do_not() {
        let text = "---\nname: x\n---\n```bash\n# why\na\nb\n\nc\nd\n```\n";
        let (blocks, _) = scan_text(text);
        assert_eq!(blocks, [ShellBlock { line: 1, lines: 4 }]);
    }

    #[test]
    fn non_shell_fences_and_frontmatter_are_ignored() {
        let text = "---\nname: x\n1. a\n---\n```rust\n1. no\n```\n```\nls\n```\n";
        let (blocks, steps) = scan_text(text);
        assert!(blocks.is_empty());
        assert_eq!(steps, 0);
    }

    #[test]
    fn a_numbered_run_survives_continuations_and_breaks_on_prose() {
        let text = "1. a\n   more\n\n2. b\n```sh\nx\n```\n3. c\nprose\n1. d\n";
        let (_, steps) = scan_text(text);
        assert_eq!(steps, 3);
    }

    #[test]
    fn baseline_holds_shrinks_and_fails_on_growth_or_new_debt() {
        let config = ProcedureConfig { max_shell_lines: 1, max_steps: 2 };
        let known = Baseline::parse("shell: a 4\nsteps: a 3\n");
        let mut errors = Vec::new();
        judge(&mut errors, &known.shell, "shell", "a", 4, "");
        judge(&mut errors, &known.steps, "steps", "a", 2, "");
        assert!(errors.is_empty());
        judge(&mut errors, &known.shell, "shell", "a", 5, "");
        judge(&mut errors, &known.steps, "steps", "b", 3, "");
        assert_eq!(errors.len(), 2);
        let scan = SkillScan {
            name: "a".into(),
            path: PathBuf::new(),
            shell_blocks: vec![ShellBlock { line: 1, lines: 1 }, ShellBlock { line: 5, lines: 4 }],
            longest_steps: 3,
        };
        assert_eq!(Baseline::render(&[scan], &config).lines().filter(|l| !l.starts_with('#')).count(), 2);
    }

    #[test]
    fn zero_skills_is_a_failure() {
        let home = tempfile::TempDir::new().unwrap();
        let r = lint(&[home.path().to_path_buf()], &ProcedureConfig::default(), &Baseline::default()).unwrap();
        assert!(!r.is_ok());
    }
}
