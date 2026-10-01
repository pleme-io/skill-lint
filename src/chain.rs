//! The `CLAUDE.md` load chain — what one session actually reads.
//!
//! [`crate::claudemd`] measures one file. Claude Code never loads one file: a
//! session started in directory `D` reads the global `~/.claude/CLAUDE.md`,
//! then every `CLAUDE.md` from the top of the walk down to `D`, then every
//! `@path` any of them imports, recursively. The SUM is the standing tax on
//! every task started in `D`, and nothing measured it — measured 2026-10-01, a
//! session in one repository loaded ~130k chars, and that number had no gate
//! and no report, so it could only grow.
//!
//! There is a second, quieter defect class. A `@path` whose target is not
//! there is skipped by Claude Code without a word: the session simply lacks
//! whatever the import was meant to carry, and an import that loads nothing
//! looks exactly like one that works. Here it is an error.
//!
//! # The load set, in order
//!
//! 1. The global file (`--global`, default `<home>/.claude/CLAUDE.md`).
//! 2. For each directory from the top of the walk down to `D`, root-most first:
//!    `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md`. The walk's top is
//!    `<home>` when `D` is under it, and the directory below `/` otherwise.
//! 3. After each file, depth-first, the files it imports.
//!
//! A file reached twice — `~/.claude/CLAUDE.md` is both the global file and
//! `$HOME`'s own `.claude/CLAUDE.md`; two files may import a third — is loaded
//! and counted once, keyed on its canonical path.
//!
//! # What counts as an import
//!
//! `@path` at the start of a line or after whitespace, in prose. Not inside a
//! fenced block or an inline code span ([`crate::markdown`] decides which is
//! which): there it is an example of an import. A `#fragment` is dropped and
//! `\ ` is an escaped space. The path must start `./`, `~/`, `/` (not `/`
//! alone), or with a letter, digit, `.`, `_` or `-` — so `@(x)`, `@#x` and a
//! bare `@` are prose, and `name@host` (no whitespace before the `@`) is too.
//!
//! `~/` resolves against `<home>`, an absolute path stands as written, and
//! anything else resolves against the IMPORTING file's directory. A target
//! more than [`MAX_IMPORT_HOPS`] hops from the file that started the chain is
//! not followed, and an import of a file already on the current import path is
//! a cycle — reported, not failed, because the second visit costs nothing.
//!
//! # What this does not model, and says so
//!
//! - **Indented code blocks and HTML comments.** An `@path` inside either is
//!   read as an import. The error is loud in the safe direction: a reported
//!   missing import, never a silently uncounted one.
//! - **Directories above `<home>`.** Claude Code walks to `/`; this stops at
//!   `<home>`, which keeps `--home` a hermetic boundary. A `CLAUDE.md` in
//!   `/Users` would be missed.
//! - **Import approval and file-type filters.** Every import is assumed
//!   approved and every target text, so the total is a ceiling on what loads,
//!   never an underestimate.
//! - **`.claude/rules/`.** Rule files load beside `CLAUDE.md` and are not in
//!   this walk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::check::normalize_lexical;
use crate::error::{CheckKind, LintError};
use crate::markdown::{self, Segment};

/// How many hops an import chain may run from the file that started it.
/// Claude Code's documented maximum.
pub const MAX_IMPORT_HOPS: usize = 5;

/// The spellings checked in every directory of the walk, in load order.
pub const CHAIN_FILE_NAMES: [&str; 3] = ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];

// ═══════════════════════════════════════════════════════════════════
// ChainFs — the I/O seam
// ═══════════════════════════════════════════════════════════════════

/// Where file contents come from. Mirrors [`crate::claudemd::DocSource`]:
/// production reads the filesystem, tests hand over strings.
pub trait ChainFs {
    /// The bytes of the regular file at `path`, or `None` when no loadable file
    /// is there. Absence is not an error at this layer: a directory without a
    /// `CLAUDE.md` is the normal case, and only the caller knows whether the
    /// path was an import that had to resolve.
    fn read(&self, path: &Path) -> Option<Vec<u8>>;

    /// The identity a file is loaded once under.
    fn identity(&self, path: &Path) -> PathBuf;
}

/// Filesystem-backed [`ChainFs`].
pub struct RealFs;

impl ChainFs for RealFs {
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        if path.is_file() { std::fs::read(path).ok() } else { None }
    }

    /// The canonical path, so a symlink and its target are one file. Falls
    /// back to the lexical form for a path that does not exist.
    fn identity(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| normalize_lexical(path))
    }
}

// ═══════════════════════════════════════════════════════════════════
// Imports
// ═══════════════════════════════════════════════════════════════════

/// One `@path` import a file makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// One-based line number.
    pub line: usize,
    /// The path as written after `@`, fragment included.
    pub written: String,
    /// The path to resolve: fragment dropped, `\ ` unescaped.
    pub path: String,
}

/// Every import `content` makes, in order.
#[must_use]
pub fn imports(content: &str) -> Vec<Import> {
    let mut out = Vec::new();
    for (index, line) in markdown::prose_lines(content) {
        for segment in markdown::segments(line) {
            if let Segment::Prose { text, at_line_start } = segment {
                collect_imports(text, at_line_start, index + 1, &mut out);
            }
        }
    }
    out
}

/// Pull `@path` imports out of one prose segment.
fn collect_imports(text: &str, at_line_start: bool, line: usize, out: &mut Vec<Import>) {
    let mut cursor = 0;
    while let Some(found) = text[cursor..].find('@') {
        let at = cursor + found;
        let token_start = at + 1;
        let token_end = token_end(text, token_start);
        cursor = token_end.max(token_start);

        // A segment that does not begin the line begins right after a closing
        // backtick, so an `@` at its offset 0 is butted against code.
        let boundary = if at == 0 {
            at_line_start
        } else {
            text[..at].chars().next_back().is_some_and(char::is_whitespace)
        };
        if !boundary {
            continue;
        }

        let written = &text[token_start..token_end];
        let unescaped = written.replace("\\ ", " ");
        let path = unescaped.split('#').next().unwrap_or_default();
        if is_import_path(path) {
            out.push(Import { line, written: written.to_owned(), path: path.to_owned() });
        }
    }
}

/// Byte offset where a token starting at `start` ends: the first whitespace
/// that is not escaped by a preceding backslash.
fn token_end(text: &str, start: usize) -> usize {
    let mut escaped = false;
    for (offset, c) in text[start..].char_indices() {
        if c.is_whitespace() && !escaped {
            return start + offset;
        }
        escaped = c == '\\' && !escaped;
    }
    text.len()
}

/// Is this the shape of a path Claude Code tries to import?
#[must_use]
pub fn is_import_path(path: &str) -> bool {
    path.starts_with("./")
        || path.starts_with("~/")
        || (path.starts_with('/') && path != "/")
        || path
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Resolve an import path written in a file living in `importer_dir`.
#[must_use]
pub fn resolve_import(path: &str, importer_dir: &Path, home: &Path) -> PathBuf {
    let joined = if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        importer_dir.join(path)
    };
    normalize_lexical(&joined)
}

// ═══════════════════════════════════════════════════════════════════
// The walk
// ═══════════════════════════════════════════════════════════════════

/// Configuration for a `chain` run.
#[derive(Debug, Clone)]
pub struct ChainConfig {
    /// `$HOME`: the top of the walk and the base of `~/` imports.
    pub home: PathBuf,
    /// The global file, loaded first in every session.
    pub global: PathBuf,
    /// Ceiling on one session's total, in bytes. `None` reports without gating.
    pub max_bytes: Option<usize>,
    /// Ceiling on any one loaded file, in bytes. `None` reports without gating.
    pub max_file_bytes: Option<usize>,
}

/// Why a file is in the load set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The global file.
    Global,
    /// A `CLAUDE.md` spelling in a directory of the walk.
    Chain,
    /// Imported by another loaded file.
    Import {
        /// The importing file.
        by: PathBuf,
        /// The importing line, one-based.
        line: usize,
    },
}

/// One file a session loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFile {
    /// The path as reached — not canonicalized, so a symlinked `CLAUDE.md`
    /// reports where it sits, not where it points.
    pub path: PathBuf,
    /// Raw bytes on disk: exactly what gets loaded.
    pub bytes: usize,
    /// Import hops from the file that started its chain; 0 for that file.
    pub depth: usize,
    /// Why it is loaded.
    pub origin: Origin,
}

/// An import of a file already on the current import path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportCycle {
    /// The importing file.
    pub file: PathBuf,
    /// The importing line, one-based.
    pub line: usize,
    /// The file it imports, already being loaded above it.
    pub target: PathBuf,
}

/// Everything one session loads, and what is wrong with it.
#[derive(Debug)]
pub struct Session {
    /// The session directory.
    pub dir: PathBuf,
    /// Loaded files in load order.
    pub files: Vec<LoadedFile>,
    /// Import cycles — reported, never failed.
    pub cycles: Vec<ImportCycle>,
    /// Everything that failed.
    pub errors: Vec<LintError>,
}

impl Session {
    /// Total bytes loaded.
    #[must_use]
    pub fn total_bytes(&self) -> usize { self.files.iter().map(|f| f.bytes).sum() }

    /// Returns `true` when nothing failed.
    #[must_use]
    pub fn is_ok(&self) -> bool { self.errors.is_empty() }
}

/// The directories of the walk for a session in `dir`, root-most first.
///
/// From `<home>` down when `dir` is under it; otherwise from the directory
/// below `/`. Both paths are taken as given — the caller canonicalizes, so a
/// symlinked `$HOME` cannot make `dir` look outside it.
#[must_use]
pub fn walk_dirs(dir: &Path, home: &Path) -> Vec<PathBuf> {
    let under_home = dir.starts_with(home);
    let mut dirs: Vec<PathBuf> = Vec::new();
    for ancestor in dir.ancestors() {
        if ancestor.parent().is_none() {
            break;
        }
        dirs.push(ancestor.to_path_buf());
        if under_home && ancestor == home {
            break;
        }
    }
    dirs.reverse();
    dirs
}

/// Resolve the load set of one session and judge it.
#[must_use]
pub fn resolve(dir: &Path, fs: &dyn ChainFs, config: &ChainConfig) -> Session {
    let mut loader = Loader {
        fs,
        home: &config.home,
        files: Vec::new(),
        loaded: BTreeSet::new(),
        stack: Vec::new(),
        cycles: Vec::new(),
        errors: Vec::new(),
    };

    if let Some(bytes) = fs.read(&config.global) {
        loader.load(&config.global, &bytes, 0, Origin::Global);
    }
    let walk = walk_dirs(dir, &config.home);
    for directory in &walk {
        for name in CHAIN_FILE_NAMES {
            let path = directory.join(name);
            if let Some(bytes) = fs.read(&path) {
                loader.load(&path, &bytes, 0, Origin::Chain);
            }
        }
    }

    let mut session = Session {
        dir: dir.to_path_buf(),
        files: loader.files,
        cycles: loader.cycles,
        errors: Vec::new(),
    };

    if session.files.is_empty() {
        session.errors.push(LintError::NoChainFiles {
            kind: CheckKind::Discovery,
            dir: tilde(dir, &config.home),
            searched: format!(
                "{} and {} from {} down",
                tilde(&config.global, &config.home),
                CHAIN_FILE_NAMES.join(" / "),
                walk.first().map_or_else(|| tilde(dir, &config.home), |top| tilde(top, &config.home)),
            ),
        });
    }
    session.errors.extend(loader.errors);
    session.errors.extend(ceiling_errors(&session, config));
    session
}

/// The per-file and per-session ceilings.
fn ceiling_errors(session: &Session, config: &ChainConfig) -> Vec<LintError> {
    let mut errors = Vec::new();
    let dir = tilde(&session.dir, &config.home);
    if let Some(cap) = config.max_file_bytes {
        for file in session.files.iter().filter(|f| f.bytes > cap) {
            errors.push(LintError::ChainFileTooLarge {
                kind: CheckKind::ClaudeMdChain,
                dir: dir.clone(),
                file: tilde(&file.path, &config.home),
                bytes: file.bytes,
                cap,
                over: file.bytes - cap,
            });
        }
    }
    let total = session.total_bytes();
    if let Some(cap) = config.max_bytes.filter(|cap| total > *cap) {
        errors.push(LintError::ChainTooLarge {
            kind: CheckKind::ClaudeMdChain,
            dir,
            bytes: total,
            cap,
            over: total - cap,
        });
    }
    errors
}

/// Depth-first loader. `loaded` is the once-only set across the whole session;
/// `stack` is the current import path, which is what tells a cycle from a
/// file merely reached twice.
struct Loader<'a> {
    fs: &'a dyn ChainFs,
    home: &'a Path,
    files: Vec<LoadedFile>,
    loaded: BTreeSet<PathBuf>,
    stack: Vec<PathBuf>,
    cycles: Vec<ImportCycle>,
    errors: Vec<LintError>,
}

impl Loader<'_> {
    fn load(&mut self, path: &Path, bytes: &[u8], depth: usize, origin: Origin) {
        let id = self.fs.identity(path);
        if !self.loaded.insert(id.clone()) {
            return;
        }
        let content = String::from_utf8_lossy(bytes);
        let importer_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        self.files.push(LoadedFile { path: path.to_path_buf(), bytes: bytes.len(), depth, origin });
        self.stack.push(id);

        for import in imports(&content) {
            let target = resolve_import(&import.path, &importer_dir, self.home);
            let target_id = self.fs.identity(&target);
            if self.stack.contains(&target_id) {
                self.cycles.push(ImportCycle { file: path.to_path_buf(), line: import.line, target });
                continue;
            }
            if self.loaded.contains(&target_id) {
                continue;
            }
            let hop = depth + 1;
            if hop > MAX_IMPORT_HOPS {
                self.errors.push(LintError::ImportTooDeep {
                    kind: CheckKind::ClaudeMdChain,
                    file: tilde(path, self.home),
                    line: import.line,
                    target: import.written,
                    hop,
                    limit: MAX_IMPORT_HOPS,
                });
                continue;
            }
            let Some(target_bytes) = self.fs.read(&target) else {
                self.errors.push(LintError::ImportMissing {
                    kind: CheckKind::ClaudeMdChain,
                    file: tilde(path, self.home),
                    line: import.line,
                    target: import.written,
                    resolved: tilde(&target, self.home),
                });
                continue;
            };
            self.load(&target, &target_bytes, hop, Origin::Import { by: path.to_path_buf(), line: import.line });
        }

        self.stack.pop();
    }
}

/// `path` with a leading `home` shown as `~`, for humans.
#[must_use]
pub fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════
// Report
// ═══════════════════════════════════════════════════════════════════

/// Every session of a run.
#[derive(Debug)]
pub struct ChainReport {
    /// One per `--dir`, in the order given.
    pub sessions: Vec<Session>,
}

impl ChainReport {
    /// Returns `true` when no session failed.
    #[must_use]
    pub fn is_ok(&self) -> bool { self.sessions.iter().all(Session::is_ok) }

    /// Every error across every session.
    pub fn errors(&self) -> impl Iterator<Item = &LintError> {
        self.sessions.iter().flat_map(|s| s.errors.iter())
    }

    /// The `--json` rendering. Paths are absolute; messages are the same text
    /// the human report prints.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self, config: &ChainConfig) -> serde_json::Result<String> {
        let sessions = self
            .sessions
            .iter()
            .map(|s| JsonSession {
                dir: &s.dir,
                total_bytes: s.total_bytes(),
                max_bytes: config.max_bytes,
                max_file_bytes: config.max_file_bytes,
                files: s.files.iter().map(JsonFile::from).collect(),
                cycles: &s.cycles,
                errors: s
                    .errors
                    .iter()
                    .map(|e| JsonError { kind: e.kind().to_string(), message: e.to_string() })
                    .collect(),
            })
            .collect();
        serde_json::to_string_pretty(&JsonReport {
            ok: self.is_ok(),
            home: &config.home,
            global: &config.global,
            sessions,
        })
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    ok: bool,
    home: &'a Path,
    global: &'a Path,
    sessions: Vec<JsonSession<'a>>,
}

#[derive(Serialize)]
struct JsonSession<'a> {
    dir: &'a Path,
    total_bytes: usize,
    max_bytes: Option<usize>,
    max_file_bytes: Option<usize>,
    files: Vec<JsonFile<'a>>,
    cycles: &'a [ImportCycle],
    errors: Vec<JsonError>,
}

#[derive(Serialize)]
struct JsonFile<'a> {
    path: &'a Path,
    bytes: usize,
    depth: usize,
    origin: &'static str,
    imported_by: Option<&'a Path>,
    line: Option<usize>,
}

impl<'a> From<&'a LoadedFile> for JsonFile<'a> {
    fn from(file: &'a LoadedFile) -> Self {
        let (origin, imported_by, line) = match &file.origin {
            Origin::Global => ("global", None, None),
            Origin::Chain => ("chain", None, None),
            Origin::Import { by, line } => ("import", Some(by.as_path()), Some(*line)),
        };
        Self { path: &file.path, bytes: file.bytes, depth: file.depth, origin, imported_by, line }
    }
}

#[derive(Serialize)]
struct JsonError {
    kind: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// In-memory [`ChainFs`]: identity is the lexical path, so the walk and the
    /// import graph are testable without a filesystem.
    struct MockFs(BTreeMap<PathBuf, String>);

    impl MockFs {
        fn new(files: &[(&str, &str)]) -> Self {
            Self(files.iter().map(|(p, c)| (PathBuf::from(p), (*c).to_owned())).collect())
        }
    }

    impl ChainFs for MockFs {
        fn read(&self, path: &Path) -> Option<Vec<u8>> {
            self.0.get(&normalize_lexical(path)).map(|c| c.clone().into_bytes())
        }
        fn identity(&self, path: &Path) -> PathBuf { normalize_lexical(path) }
    }

    fn config() -> ChainConfig {
        ChainConfig {
            home: "/h".into(),
            global: "/h/.claude/CLAUDE.md".into(),
            max_bytes: None,
            max_file_bytes: None,
        }
    }

    fn written(content: &str) -> Vec<String> {
        imports(content).into_iter().map(|i| i.written).collect()
    }

    #[test]
    fn an_import_starts_a_line_or_follows_whitespace() {
        assert_eq!(written("@a.md\nsee @b.md and\t@c.md"), ["a.md", "b.md", "c.md"]);
        assert!(written("mail me@host.com").is_empty());
    }

    #[test]
    fn code_is_shown_not_imported() {
        assert!(written("`@a.md`\n```\n@b.md\n```\n~~~\n@c.md\n~~~\n").is_empty());
        // Butted against a closing backtick: not after whitespace.
        assert!(written("`x`@a.md").is_empty());
    }

    #[test]
    fn non_path_shapes_are_prose() {
        assert!(written("@ alone, @(x), @#frag, @@twice, @/").is_empty());
        assert_eq!(written("@./a @~/b @/abs @../up @_x @-y @9z"), ["./a", "~/b", "/abs", "../up", "_x", "-y", "9z"]);
    }

    #[test]
    fn a_fragment_is_dropped_and_an_escaped_space_kept() {
        let got = imports(r"@docs/a\ b.md#sec tail");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].written, r"docs/a\ b.md#sec");
        assert_eq!(got[0].path, "docs/a b.md");
    }

    #[test]
    fn imports_resolve_against_the_importer_home_or_root() {
        let home = Path::new("/h");
        let dir = Path::new("/h/p/docs");
        assert_eq!(resolve_import("x.md", dir, home), Path::new("/h/p/docs/x.md"));
        assert_eq!(resolve_import("./x.md", dir, home), Path::new("/h/p/docs/x.md"));
        assert_eq!(resolve_import("../x.md", dir, home), Path::new("/h/p/x.md"));
        assert_eq!(resolve_import("~/n/x.md", dir, home), Path::new("/h/n/x.md"));
        assert_eq!(resolve_import("/etc/x.md", dir, home), Path::new("/etc/x.md"));
    }

    #[test]
    fn the_walk_stops_at_home_or_below_root() {
        assert_eq!(walk_dirs(Path::new("/h/a/b"), Path::new("/h")), [Path::new("/h"), Path::new("/h/a"), Path::new("/h/a/b")]);
        assert_eq!(walk_dirs(Path::new("/h"), Path::new("/h")), [Path::new("/h")]);
        assert_eq!(walk_dirs(Path::new("/srv/x"), Path::new("/h")), [Path::new("/srv"), Path::new("/srv/x")]);
    }

    #[test]
    fn a_file_reached_twice_is_loaded_and_counted_once() {
        let fs = MockFs::new(&[
            ("/h/.claude/CLAUDE.md", "@~/shared.md\n"),
            ("/h/CLAUDE.md", "@shared.md\n"),
            ("/h/shared.md", "12345"),
        ]);
        let s = resolve(Path::new("/h"), &fs, &config());
        let paths: Vec<&Path> = s.files.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(paths, [Path::new("/h/.claude/CLAUDE.md"), Path::new("/h/shared.md"), Path::new("/h/CLAUDE.md")]);
        assert_eq!(s.total_bytes(), 13 + 5 + 11);
        assert!(s.is_ok() && s.cycles.is_empty(), "{s:?}");
    }

    /// A self-import is the smallest cycle.
    #[test]
    fn a_self_import_is_a_cycle() {
        let fs = MockFs::new(&[("/h/CLAUDE.md", "@CLAUDE.md\n")]);
        let s = resolve(Path::new("/h"), &fs, &config());
        assert_eq!(s.cycles.len(), 1);
        assert!(s.is_ok());
    }

    #[test]
    fn exactly_five_hops_load() {
        let fs = MockFs::new(&[
            ("/h/CLAUDE.md", "@1\n"),
            ("/h/1", "@2\n"),
            ("/h/2", "@3\n"),
            ("/h/3", "@4\n"),
            ("/h/4", "@5\n"),
            ("/h/5", "@6\n"),
            ("/h/6", "x"),
        ]);
        let s = resolve(Path::new("/h"), &fs, &config());
        assert_eq!(s.files.last().map(|f| f.depth), Some(MAX_IMPORT_HOPS));
        assert!(matches!(s.errors.as_slice(), [LintError::ImportTooDeep { hop: 6, .. }]), "{:?}", s.errors);
    }

    #[test]
    fn zero_files_is_a_discovery_error() {
        let s = resolve(Path::new("/h/p"), &MockFs::new(&[]), &config());
        assert!(matches!(s.errors.as_slice(), [LintError::NoChainFiles { .. }]), "{:?}", s.errors);
    }

    #[test]
    fn tilde_abbreviates_only_under_home() {
        let home = Path::new("/h");
        assert_eq!(tilde(Path::new("/h"), home), "~");
        assert_eq!(tilde(Path::new("/h/a"), home), "~/a");
        assert_eq!(tilde(Path::new("/hx/a"), home), "/hx/a");
    }
}
