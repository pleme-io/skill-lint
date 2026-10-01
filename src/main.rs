use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use skill_lint::check::{self, CheckConfig};
use skill_lint::claudemd::ClaudeMdConfig;

#[derive(Parser)]
#[command(name = "skill-lint", about = "Validate Claude Code skill maps")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report the skill-listing budget — the agent-reachable `/context`.
    ///
    /// `/context`'s Skills row is authoritative but interactive: it has no
    /// print-mode rendering (`claude -p "/context"` returns "Execution
    /// error"), so no agent, script or CI job can read it. This recomputes the
    /// same accounting from the frontmatter the platform reads.
    ///
    /// It does NOT guess which descriptions get dropped on overflow — that
    /// ordering is by invocation frequency, which is not on disk.
    Budget {
        /// Skill home(s). Repeat for each. The live listing is the UNION of
        /// every deployed home, so a single repo is a partial picture.
        #[arg(long)]
        skills_dir: Vec<PathBuf>,

        /// Discover `*/skills` homes under this workspace root instead.
        #[arg(long)]
        discover_under: Option<PathBuf>,

        /// Context window in tokens; the budget is a fraction of it.
        #[arg(long, default_value_t = 1_000_000)]
        window_tokens: usize,

        /// Budget as a fraction of the window (`skillListingBudgetFraction`).
        #[arg(long, default_value_t = skill_lint::budget::DEFAULT_BUDGET_FRACTION)]
        budget_fraction: f64,

        /// Exact character budget, overriding the window/fraction derivation
        /// (`SLASH_COMMAND_TOOL_CHAR_BUDGET`).
        #[arg(long)]
        budget_chars: Option<usize>,

        /// Per-entry cap (`skillListingMaxDescChars`).
        #[arg(long, default_value_t = skill_lint::budget::DEFAULT_MAX_DESC_CHARS)]
        max_desc_chars: usize,

        /// Ceiling on a whole `SKILL.md` in bytes, frontmatter included: an
        /// invoked skill loads the entire file. Off when absent.
        #[arg(long)]
        max_body_bytes: Option<usize>,

        /// Show this many largest entries.
        #[arg(long, default_value_t = 20)]
        top: usize,

        /// Exit non-zero when the listing is over budget, any entry is past
        /// `--max-desc-chars`, any `SKILL.md` is past `--max-body-bytes`, or
        /// any `SKILL.md` is unreadable, unparseable or description-less, for
        /// use as a gate. Without it the run is a report and exits 0 either way.
        #[arg(long)]
        strict: bool,
    },

    /// Run checks: sync, frontmatter, map integrity, version.
    Check {
        /// Path to the skills directory (contains skill subdirs).
        #[arg(long, default_value = ".")]
        skills_dir: PathBuf,

        /// Path to skill-map.d/ directory. Defaults to {skills_dir}/skill-map.d,
        /// then {skills_dir}/../skill-map.d, then falls back to skill-map.yaml.
        #[arg(long)]
        map_dir: Option<PathBuf>,

        /// Skip version check.
        #[arg(long)]
        skip_version: bool,

        /// Skip sync check.
        #[arg(long)]
        skip_sync: bool,

        /// Skip frontmatter check.
        #[arg(long)]
        skip_frontmatter: bool,

        /// Skip map integrity check.
        #[arg(long)]
        skip_map_integrity: bool,

        /// Skip link/path resolution — do the paths skill bodies point at exist?
        ///
        /// For the case where the answer is knowably unavailable (linting a
        /// corpus away from the repositories it points into), NOT for living
        /// with dead pointers: a single legitimately-absent target is declared
        /// with a `pending-path: <path> — <reason>` line in the skill body.
        #[arg(long)]
        skip_path_resolution: bool,

        /// A skill that MUST declare a `<!-- tier-ledger -->`. Repeat for each.
        ///
        /// Every declared ledger is validated unconditionally; this additionally
        /// makes its ABSENCE a failure, so a skill whose doctrine promises a
        /// tier-honest ledger cannot go green by deleting the table.
        ///
        /// There is no `--skip-skill-pointers` or `--skip-tier-ledger`: both
        /// resolve against data that travels with the corpus, so neither can be
        /// knowably-unavailable the way a sibling repo can. Living with a single
        /// finding is a scoped `pending-skill-pointer:` line, not a flag.
        #[arg(long = "require-tier-ledger")]
        require_tier_ledger: Vec<String>,

        /// Flag skills not verified within this many days as stale.
        #[arg(long)]
        max_age_days: Option<u32>,

        /// Per-entry skill-listing character cap. Descriptions longer than this
        /// are truncated by the platform and the remainder — including any
        /// trigger phrases in it — is silently discarded. Defaults to the
        /// platform default for `skillListingMaxDescChars`.
        #[arg(long, default_value_t = 1536)]
        max_desc_chars: usize,
    },

    /// Lint CLAUDE.md files — the anti-regrowth seal.
    ///
    /// A CLAUDE.md is loaded whole into every session, so its size taxes every
    /// task in the repository. Its index section states its own contract —
    /// "each line: rule + skill + long-form doc" — and then grows past it,
    /// because nothing was checking. This checks it.
    ///
    /// Baseline-debt shaped: known violations are recorded with the size they
    /// had when recorded, and only a NEW violation or a baselined item that
    /// GREW fails the run.
    Claudemd {
        /// A CLAUDE.md to lint. Repeat for each file.
        ///
        /// Which files were scanned is an OUTPUT of every run: a linter wired
        /// into one repository of seven is green because it never looked at the
        /// other six, and a run over zero files exits non-zero rather than
        /// reporting a vacuous pass.
        #[arg(long = "file")]
        files: Vec<PathBuf>,

        /// Folded-byte ceiling for one index entry.
        #[arg(long, default_value_t = skill_lint::claudemd::DEFAULT_MAX_ENTRY_BYTES)]
        max_entry_bytes: usize,

        /// Raw-byte ceiling for a whole file.
        #[arg(long, default_value_t = skill_lint::claudemd::DEFAULT_MAX_FILE_BYTES)]
        max_file_bytes: usize,

        /// Heading substring identifying the index section whose entries are
        /// measured. Bullets elsewhere in the file are prose, not entries.
        #[arg(long, default_value = skill_lint::claudemd::DEFAULT_INDEX_HEADING)]
        index_heading: String,

        /// Baseline of known debt. Without one, every existing violation fails.
        #[arg(long)]
        baseline: Option<PathBuf>,

        /// Write the current violations to this path as a baseline, then exit
        /// 0. This is what makes the gate adoptable on a file that is already
        /// in violation; it is not a way to clear a finding you just caused.
        #[arg(long)]
        write_baseline: Option<PathBuf>,

        /// Show this many largest entries.
        #[arg(long, default_value_t = 10)]
        top: usize,
    },

    /// Lint GitHub Actions workflows for inline shell — the no-shell seal.
    ///
    /// The fleet PRIME DIRECTIVE allows ~3 lines of inline glue and no more. The
    /// shape that evades it is a block-form `run:`, because that does not look
    /// like a shell script — it looks like YAML with a long string in it.
    ///
    /// Flow form (`run: cargo build`) is never a violation: a one-line
    /// invocation of a typed tool is the target shape. Comments are not counted:
    /// explaining WHY at the call site is house style and must not be taxed.
    ///
    /// Baseline-debt shaped, exactly like `claudemd`: known shell is recorded at
    /// its measured line count, and only a NEW block or a baselined block that
    /// GREW fails the run.
    Workflows {
        /// A workflow YAML to lint. Repeat for each file.
        ///
        /// Which files were scanned is an OUTPUT of every run, and a run over
        /// zero files exits non-zero rather than reporting a vacuous pass.
        #[arg(long = "file")]
        files: Vec<PathBuf>,

        /// Non-blank, non-comment lines a block-form `run:` may hold.
        #[arg(long, default_value_t = skill_lint::workflows::DEFAULT_MAX_RUN_LINES)]
        max_run_lines: usize,

        /// Baseline of known debt. Without one, every existing violation fails.
        #[arg(long)]
        baseline: Option<PathBuf>,

        /// Write the current violations to this path as a baseline, then exit 0.
        /// This is what makes the gate adoptable on a corpus already in
        /// violation; it is not a way to clear a finding you just caused.
        #[arg(long)]
        write_baseline: Option<PathBuf>,

        /// Show this many longest steps.
        #[arg(long, default_value_t = 10)]
        top: usize,
    },

    /// Measure the CLAUDE.md load chain of a session — every file Claude Code
    /// reads before the first token of work, imports included.
    ///
    /// `claudemd` measures one file; nothing measured the sum, so it regrew in
    /// silence. For each `--dir` this resolves the load set in order — the
    /// global file, then `CLAUDE.md`, `.claude/CLAUDE.md` and `CLAUDE.local.md`
    /// from `--home` down to the directory, then every `@path` they import —
    /// and reports each file and the total.
    ///
    /// A dangling `@path` fails the run: Claude Code skips it without a word.
    /// So does an import past the 5-hop limit, and a session that loads zero
    /// files (a vacuous pass). Cycles are reported, not failed.
    Chain {
        /// A session directory. Repeat for each; each is its own session with
        /// its own total.
        #[arg(long = "dir", required = true)]
        dirs: Vec<PathBuf>,

        /// `$HOME`: the top of the walk and the base of `~/` imports.
        /// Defaults to the `HOME` environment variable.
        #[arg(long)]
        home: Option<PathBuf>,

        /// The global file loaded first in every session. Defaults to
        /// `<home>/.claude/CLAUDE.md`.
        #[arg(long)]
        global: Option<PathBuf>,

        /// Fail when one session's total exceeds this many bytes. Without it the
        /// total is reported, not gated.
        #[arg(long)]
        max_bytes: Option<usize>,

        /// Fail when any one loaded file exceeds this many bytes.
        #[arg(long)]
        max_file_bytes: Option<usize>,

        /// Print the report as JSON on stdout instead of text.
        #[arg(long)]
        json: bool,
    },

    /// Skill usage: record each invocation from a Claude Code hook, then report
    /// which skills are used, which are not, and the order the listing budget
    /// drops them in.
    ///
    /// `budget` cannot say which descriptions the platform drops on overflow,
    /// because that order is by invocation frequency and frequency was not on
    /// disk. `record` puts it on disk; `report` reads it back.
    Usage {
        #[command(subcommand)]
        action: UsageAction,
    },
}

#[derive(Subcommand)]
enum UsageAction {
    /// Read ONE hook event (JSON) on stdin and append a line to the usage log
    /// when it is a skill invocation.
    ///
    /// Wire it as a `PreToolUse` hook with matcher `Skill` and as a
    /// `UserPromptSubmit` hook. It never fails and never prints: a hook's exit
    /// status and stdout reach the session, so every error — malformed input,
    /// an unwritable log, a flag this binary does not know — ends in exit 0
    /// with nothing written.
    Record {
        /// Deployed skills, for validating a typed `/<name>`. Defaults to
        /// `$HOME/.claude/skills`.
        #[arg(long)]
        skills_dir: Option<PathBuf>,

        /// The log. Defaults to `$XDG_STATE_HOME/skill-lint/usage.jsonl`, else
        /// `$HOME/.local/state/skill-lint/usage.jsonl`.
        #[arg(long)]
        log: Option<PathBuf>,
    },

    /// Counts per skill, deployed skills unused in the window, and the drop
    /// order the listing budget implies (least used first).
    Report(UsageReportArgs),

    #[command(
        about = "Per MCP server: calls, distinct tools used and last use in the window, then configured servers \
                 unused in it, most tool names first"
    )]
    McpReport(McpReportArgs),
}

#[derive(clap::Args)]
struct McpReportArgs {
    #[arg(long, default_value = skill_lint::usage::DEFAULT_SINCE, help = "Window: <N>h, <N>d, <N>w, or all")]
    since: String,

    #[arg(long, help = "The log. Same default as `record`")]
    log: Option<PathBuf>,

    #[arg(
        long,
        help = "A file with a top-level mcpServers map; repeat to union several. Defaults to $HOME/.claude.json, \
                the user scope every session loads"
    )]
    config: Vec<PathBuf>,

    #[arg(
        long,
        help = "A file listing tool names (mcp__<server>__<tool>, whitespace or JSON array); prices each server by \
                its tool count"
    )]
    tools: Option<PathBuf>,

    #[arg(long, hide = true)]
    now: Option<String>,

    #[arg(long, help = "Print the report as JSON on stdout instead of text")]
    json: bool,
}

#[derive(clap::Args)]
struct UsageReportArgs {
    /// Window: `<N>h`, `<N>d`, `<N>w`, or `all`.
    #[arg(long, default_value = skill_lint::usage::DEFAULT_SINCE)]
    since: String,

    /// Deployed skills to join against. Defaults to `$HOME/.claude/skills`.
    #[arg(long)]
    skills_dir: Option<PathBuf>,

    /// The log. Same default as `record`.
    #[arg(long)]
    log: Option<PathBuf>,

    /// Context window in tokens; the listing budget is a fraction of it.
    #[arg(long, default_value_t = 1_000_000)]
    window_tokens: usize,

    /// Exact listing budget in characters, overriding `--window-tokens`.
    #[arg(long)]
    budget_chars: Option<usize>,

    /// Per-entry cap (`skillListingMaxDescChars`).
    #[arg(long, default_value_t = skill_lint::budget::DEFAULT_MAX_DESC_CHARS)]
    max_desc_chars: usize,

    /// Evaluate the window at this RFC 3339 instant instead of now.
    #[arg(long, hide = true)]
    now: Option<String>,

    /// Print the report as JSON on stdout instead of text.
    #[arg(long)]
    json: bool,
}

/// Most a hook event may be. A larger one is not parsed, which records nothing.
const MAX_EVENT_BYTES: u64 = 8 << 20;

fn main() -> Result<()> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "usage") && args.get(2).is_some_and(|a| a == "record") {
        run_usage_record(args);
    }

    let cli = Cli::parse();

    match cli.command {
        Command::Budget {
            skills_dir,
            discover_under,
            window_tokens,
            budget_fraction,
            budget_chars,
            max_desc_chars,
            max_body_bytes,
            top,
            strict,
        } => run_budget(BudgetArgs {
            skills_dir,
            discover_under,
            window_tokens,
            budget_fraction,
            budget_chars,
            max_desc_chars,
            max_body_bytes,
            top,
            strict,
        }),

        Command::Check {
            skills_dir,
            skip_version,
            skip_sync,
            skip_frontmatter,
            skip_map_integrity,
            skip_path_resolution,
            require_tier_ledger,
            map_dir,
            max_age_days,
            max_desc_chars,
        } => {
            let config = CheckConfig {
                version: !skip_version,
                sync: !skip_sync,
                frontmatter: !skip_frontmatter,
                map_integrity: !skip_map_integrity,
                duplicate_concerns: !skip_map_integrity,
                path_resolution: !skip_path_resolution,
                max_age_days,
                today: None,
                max_desc_chars,
                require_tier_ledger: require_tier_ledger.into_iter().collect(),
            };

            let source = check::FsSource {
                skills_dir: &skills_dir,
                map_dir_override: map_dir.as_deref(),
            };
            let report = check::check_all(&source, &config)
                .with_context(|| format!("checking {}", skills_dir.display()))?;

            if report.is_ok() {
                eprintln!("skill-lint: all checks passed ({} skills)", report.skills_checked);
            } else {
                eprintln!("skill-lint: {} error(s):", report.errors.len());
                for err in &report.errors {
                    eprintln!("  - {err}");
                }
                process::exit(1);
            }

            Ok(())
        }

        Command::Claudemd {
            files,
            max_entry_bytes,
            max_file_bytes,
            index_heading,
            baseline,
            write_baseline,
            top,
        } => run_claudemd(
            &files,
            &ClaudeMdConfig { max_entry_bytes, max_file_bytes, index_heading },
            baseline.as_deref(),
            write_baseline.as_deref(),
            top,
        ),

        Command::Workflows { files, max_run_lines, baseline, write_baseline, top } => {
            run_workflows(&files, max_run_lines, baseline.as_deref(), write_baseline.as_deref(), top)
        }

        Command::Chain { dirs, home, global, max_bytes, max_file_bytes, json } => {
            run_chain(&dirs, home, global, max_bytes, max_file_bytes, json)
        }

        Command::Usage { action: UsageAction::Report(args) } => run_usage_report(&args),

        Command::Usage { action: UsageAction::McpReport(args) } => run_mcp_report(&args),

        // Dispatched before `Cli::parse`, so clap can never exit 2 on its behalf.
        Command::Usage { action: UsageAction::Record { .. } } => Ok(()),
    }
}

/// The `usage record` hook. Never returns, always exits 0, prints nothing.
///
/// It is dispatched before `Cli::parse` because clap exits 2 on a bad argument,
/// and exit 2 from a `PreToolUse` hook BLOCKS the tool call (from a
/// `UserPromptSubmit` hook it erases the prompt). A module that passed a flag an
/// older binary lacks would otherwise block every Skill call in every session.
/// A panic is caught and silenced for the same reason.
fn run_usage_record(args: Vec<std::ffi::OsString>) -> ! {
    use std::io::Read as _;
    use skill_lint::usage::{self, RecordEnv};

    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(move || {
        let cli = match Cli::try_parse_from(&args) {
            Ok(cli) => cli,
            Err(e) => {
                // `--help` is a person at a terminal, never a hook.
                if matches!(e.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion) {
                    let _ = e.print();
                }
                return;
            }
        };
        let Command::Usage { action: UsageAction::Record { skills_dir, log } } = cli.command else { return };
        let home = std::env::var_os("HOME");
        let Some(log) = log.or_else(|| usage::default_log_path(std::env::var_os("XDG_STATE_HOME"), home.clone()))
        else {
            return;
        };
        let skills_dir = skills_dir.or_else(|| usage::default_skills_dir(home));

        let mut input = Vec::new();
        if std::io::stdin().lock().take(MAX_EVENT_BYTES).read_to_end(&mut input).is_err() {
            return;
        }
        let env = RecordEnv { skills_dir: skills_dir.as_deref(), now: usage::now_secs() };
        if let Some(line) = usage::record(&input, &env) {
            let _ = usage::append(&log, &line);
        }
    });
    process::exit(0)
}

/// The `usage report` subcommand.
fn run_usage_report(args: &UsageReportArgs) -> Result<()> {
    use skill_lint::usage::{self, ReportInput, Window};

    let window: Window = args.since.parse().map_err(anyhow::Error::msg)?;
    let now = report_now(args.now.as_deref())?;
    let home = std::env::var_os("HOME");
    let log = report_log(args.log.as_deref(), home.clone())?;
    let skills_dir = match &args.skills_dir {
        Some(dir) => dir.clone(),
        None => usage::default_skills_dir(home).context("HOME is unset; pass --skills-dir")?,
    };
    let budget_chars = args.budget_chars.unwrap_or_else(|| {
        skill_lint::budget::budget_from_window(args.window_tokens, skill_lint::budget::DEFAULT_BUDGET_FRACTION)
    });

    let report = usage::report(&ReportInput {
        log: &log,
        skills_dir: &skills_dir,
        now,
        window,
        window_label: &args.since,
        budget_chars,
        max_desc_chars: args.max_desc_chars,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).context("rendering JSON")?);
    } else {
        print_usage_report(&report);
    }
    Ok(())
}

fn report_now(now: Option<&str>) -> Result<i64> {
    match now {
        Some(ts) => skill_lint::usage::parse_rfc3339(ts).with_context(|| format!("--now '{ts}' is not RFC 3339")),
        None => Ok(skill_lint::usage::now_secs()),
    }
}

fn report_log(log: Option<&std::path::Path>, home: Option<std::ffi::OsString>) -> Result<PathBuf> {
    match log {
        Some(log) => Ok(log.to_path_buf()),
        None => skill_lint::usage::default_log_path(std::env::var_os("XDG_STATE_HOME"), home)
            .context("neither XDG_STATE_HOME nor HOME is set; pass --log"),
    }
}

fn run_mcp_report(args: &McpReportArgs) -> Result<()> {
    use skill_lint::mcp::{self, McpReportInput};
    use skill_lint::usage::Window;

    let window: Window = args.since.parse().map_err(anyhow::Error::msg)?;
    let now = report_now(args.now.as_deref())?;
    let home = std::env::var_os("HOME");
    let log = report_log(args.log.as_deref(), home.clone())?;
    let configs = if args.config.is_empty() {
        let path = mcp::default_config_path(home).context("HOME is unset; pass --config")?;
        vec![mcp::read_config(&path, false)?]
    } else {
        args.config.iter().map(|p| mcp::read_config(p, true)).collect::<Result<Vec<_>>>()?
    };
    let tools = args.tools.as_deref().map(mcp::read_tools).transpose()?;

    let report = mcp::report(McpReportInput { log: &log, now, window, window_label: &args.since, configs, tools })?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).context("rendering JSON")?);
    } else {
        print_mcp_report(&report);
    }
    Ok(())
}

fn print_mcp_report(r: &skill_lint::mcp::McpReport) {
    let absent = if r.log_present { "" } else { "  (absent: nothing recorded yet)" };
    println!("usage log: {}{absent}", r.log);
    match &r.since {
        Some(since) => println!("window:    {} (since {since}, until {})", r.window, r.now),
        None => println!("window:    all (until {})", r.now),
    }
    println!(
        "events:    {} MCP call(s) in window; {} line(s) read, {} skill, {} malformed",
        r.events_in_window, r.lines_read, r.skill_lines, r.malformed_lines
    );
    for c in &r.configs {
        let state = if c.present { format!("{} server(s)", c.servers.len()) } else { "absent".to_owned() };
        println!("config:    {} ({state})", c.path);
    }
    println!("configured: {} distinct server(s)", r.configured);
    match &r.tools_file {
        Some(t) => println!("tools:     {} ({} tool name(s) across {} server(s))", t.path, t.tools, t.servers),
        None => println!("tools:     no --tools list given; servers are not priced by tool count"),
    }

    let count = |n: Option<usize>| n.map_or_else(|| "?".to_owned(), |n| n.to_string());
    println!("\nused in window ({}):", r.used.len());
    if !r.used.is_empty() {
        println!("  {:>5}  {:>7}  {:<20}  server", "calls", "tools", "last used");
    }
    for u in &r.used {
        let mark = match &u.config_name {
            None => "   (not configured)".to_owned(),
            Some(name) if *name != u.server => format!("   (configured as {name})"),
            Some(_) => String::new(),
        };
        let tools = format!("{}/{}", u.distinct_tools, count(u.tool_count));
        println!("  {:>5}  {:>7}  {:<20}  {}{mark}", u.calls, tools, u.last_used, u.server);
    }

    println!(
        "\nconfigured but unused in window ({}, {} tool name(s) priced) — retire candidates, most tools first:",
        r.unused.len(),
        r.unused_tool_names
    );
    if !r.unused.is_empty() {
        println!("  {:>5}  {:<20}  server", "tools", "last seen");
    }
    for u in &r.unused {
        let seen = u.last_seen.as_deref().unwrap_or("never");
        let mark = if u.config_name == u.server { String::new() } else { format!("   (configured as {})", u.config_name) };
        println!("  {:>5}  {:<20}  {}{mark}", count(u.tool_count), seen, u.server);
    }
}

fn print_usage_report(r: &skill_lint::usage::UsageReport) {
    let absent = if r.log_present { "" } else { "  (absent: nothing recorded yet)" };
    println!("usage log: {}{absent}", r.log);
    match &r.since {
        Some(since) => println!("window:    {} (since {since}, until {})", r.window, r.now),
        None => println!("window:    all (until {})", r.now),
    }
    println!(
        "events:    {} in window; {} line(s) read, {} malformed",
        r.events_in_window, r.lines_read, r.malformed_lines
    );
    println!("deployed:  {} skill(s) under {}", r.deployed, r.skills_dir);

    println!("\nused in window ({}):", r.used.len());
    if !r.used.is_empty() {
        println!("  {:>5} {:>5} {:>5}  {:<20}  skill", "total", "slash", "tool", "last used");
    }
    for u in &r.used {
        let mark = if u.deployed { "" } else { "   (not deployed)" };
        println!("  {:>5} {:>5} {:>5}  {:<20}  {}{mark}", u.total, u.slash, u.tool, u.last_used, u.skill);
    }

    println!("\ndeployed but unused in window ({}) — retire/merge candidates:", r.unused.len());
    for u in &r.unused {
        let seen = u.last_seen.as_deref().map_or_else(|| "never seen".to_owned(), |s| format!("last seen {s}"));
        println!("  {:<40} {seen}", u.skill);
    }

    let verdict = if r.drops_needed == 0 {
        format!("within the {}-char budget", r.budget_chars)
    } else {
        format!(
            "OVER the {}-char budget by {}: the first {} below go first",
            r.budget_chars,
            r.listing_chars.saturating_sub(r.budget_chars),
            r.drops_needed
        )
    };
    println!("\ndrop order, least used first — listing {} chars, {verdict}:", r.listing_chars);
    println!("  {:>5}  {:<20}  {:>7}  {:>10}  skill", "uses", "last seen", "listing", "cumulative");
    for (i, d) in r.drop_order.iter().enumerate() {
        println!(
            "  {:>5}  {:<20}  {:>7}  {:>10}  {}",
            d.uses,
            d.last_seen.as_deref().unwrap_or("never"),
            d.listing_chars,
            d.cumulative_chars,
            d.skill
        );
        if r.drops_needed > 0 && i + 1 == r.drops_needed {
            println!("  ── the listing fits once everything above this line is dropped ──");
        }
    }
}

/// The `claudemd` subcommand.
///
/// Every subcommand body lives in its own `run_*` function and `main` only
/// dispatches. Inline arms are what held `main` over `clippy::too_many_lines`
/// (202 lines at worst); with this one moved out it is under the limit.
fn run_claudemd(
    files: &[PathBuf],
    config: &ClaudeMdConfig,
    baseline: Option<&std::path::Path>,
    write_baseline: Option<&std::path::Path>,
    top: usize,
) -> Result<()> {
    use skill_lint::claudemd::{Baseline, FsDocSource};

    let known = match baseline {
        Some(path) => Baseline::load(path)?,
        None => Baseline::default(),
    };

    let source = FsDocSource { paths: files };
    let report = skill_lint::claudemd::lint_all(&source, config, known)
        .context("linting CLAUDE.md files")?;

    // Coverage is part of the gate: what was scanned is stated before
    // any verdict, so a green run over the wrong file set is visible
    // rather than merely green.
    println!("scanned {} file(s):", report.scans.len());
    for scan in &report.scans {
        match &scan.section {
            Some(section) => println!(
                "  {:<40} {:>8} B   index {:?}: {} entries",
                scan.key,
                scan.bytes,
                section.heading.trim(),
                section.entries.len()
            ),
            None => println!(
                "  {:<40} {:>8} B   no index section matching {:?}",
                scan.key, scan.bytes, config.index_heading
            ),
        }
    }

    let entries = report.entry_count();
    println!(
        "\nindex entries: {entries} across {} file(s) — {} over the {} B ceiling",
        report.scans.len(),
        report.over_ceiling(),
        config.max_entry_bytes
    );
    if let Some(median) = report.median_entry_bytes() {
        let largest = report.entries_by_size();
        println!(
            "  median {median} B, max {} B, total {} B",
            largest.first().map_or(0, |e| e.bytes),
            largest.iter().map(|e| e.bytes).sum::<usize>()
        );
        println!("  largest {top}:");
        for entry in largest.iter().take(top) {
            let over = entry.bytes.saturating_sub(config.max_entry_bytes);
            println!("    {:>6} B  (+{:>5})  {}", entry.bytes, over, entry.key);
        }
    }

    let census = report.census();
    println!("\ndirective census (a load signal, not a verdict):");
    println!("  skip-* waivers      {:>5}   {}", census.skip_total(), top_names(&census.skips));
    println!("  pending-* markers   {:>5}   {}", census.pending_total(), top_names(census.pendings()));
    println!(
        "  imperative lines    {:>5}   {}",
        census.imperative_lines,
        top_names(&census.imperatives)
    );
    println!("  \" never \" in prose  {:>5}", census.never_lowercase);

    if let Some(path) = write_baseline {
        let text = Baseline::render(&report.scans, config);
        std::fs::write(path, &text)
            .with_context(|| format!("writing baseline {}", path.display()))?;
        println!(
            "\nwrote baseline {} ({} recorded item(s))",
            path.display(),
            text.lines().filter(|l| !l.starts_with('#')).count()
        );
        return Ok(());
    }

    if report.is_ok() {
        eprintln!(
            "skill-lint claudemd: all checks passed ({} file(s), {entries} index entries)",
            report.scans.len()
        );
    } else {
        eprintln!("\nskill-lint claudemd: {} error(s):", report.errors.len());
        for err in &report.errors {
            eprintln!("  - {err}");
        }
        process::exit(1);
    }

    Ok(())
}

/// Arguments of the `budget` subcommand, bundled so the arm in `main` stays a
/// dispatch rather than a body.
struct BudgetArgs {
    skills_dir: Vec<PathBuf>,
    discover_under: Option<PathBuf>,
    window_tokens: usize,
    budget_fraction: f64,
    budget_chars: Option<usize>,
    max_desc_chars: usize,
    max_body_bytes: Option<usize>,
    top: usize,
    strict: bool,
}

/// The `budget` subcommand.
///
/// Its own function so `main` stays a dispatch; see [`run_claudemd`].
fn run_budget(args: BudgetArgs) -> Result<()> {
    let BudgetArgs {
        skills_dir,
        discover_under,
        window_tokens,
        budget_fraction,
        budget_chars,
        max_desc_chars,
        max_body_bytes,
        top,
        strict,
    } = args;

    let homes = if let Some(root) = discover_under.as_deref() {
        skill_lint::budget::discover_homes(root)
    } else if skills_dir.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        skills_dir
    };

    let budget = budget_chars
        .unwrap_or_else(|| skill_lint::budget::budget_from_window(window_tokens, budget_fraction));

    let report = skill_lint::budget::compute(&homes, budget, max_desc_chars)
        .context("computing skill-listing budget")?;

    println!("homes scanned ({}):", report.homes.len());
    for h in &report.homes {
        println!("  {h}");
    }
    println!("\nskills:            {}", report.entries.len());
    println!("listing chars:     {}", report.total_listing_chars);
    println!("budget chars:      {}  (estimated from a {window_tokens}-token window at {budget_fraction}; chars/token is approximate)", report.budget_chars);

    if report.over_budget() {
        println!(
            "OVER BUDGET by {} chars ({:.1}x)",
            report.overage_chars(),
            report.ratio()
        );
        println!(
            "  On overflow the platform drops descriptions starting with the skills you\n  \
             invoke LEAST. This tool cannot know that order — invocation counts are not on\n  \
             disk — so it does not guess which entries go. Run /context for the real\n  \
             post-budget size."
        );
    } else {
        println!("within budget ({} chars to spare)", report.budget_chars - report.total_listing_chars);
    }

    let truncated = report.truncated();
    if truncated.is_empty() {
        println!("\nper-entry cap ({}): all entries fit", report.max_desc_chars);
    } else {
        println!(
            "\nper-entry cap ({}): {} entries OVER, discarding {} chars outright",
            report.max_desc_chars,
            truncated.len(),
            report.total_truncated_chars()
        );
        for e in &truncated {
            println!("  {:6} over  {}", e.truncated_chars, e.name);
        }
    }

    println!("\nlargest {top}:");
    for e in report.entries.iter().take(top) {
        println!("  {:6}  {}", e.listing_chars, e.name);
    }

    if let Some(cap) = max_body_bytes {
        print_body_cap(&report, cap);
    }

    if report.findings.is_empty() {
        println!("\nevery SKILL.md parsed with a description");
    } else {
        println!(
            "\nunreadable or description-less SKILL.md: {} (not counted as authored)",
            report.findings.len()
        );
        for finding in &report.findings {
            println!("  {finding}");
        }
    }

    if strict {
        let failures = strict_failures(&report, max_body_bytes);
        if !failures.is_empty() {
            eprintln!("\nskill-lint budget --strict: {} failure(s):", failures.len());
            for failure in &failures {
                eprintln!("  - {failure}");
            }
            process::exit(1);
        }
        let body = max_body_bytes.map(|cap| format!(", every SKILL.md under {cap} bytes")).unwrap_or_default();
        eprintln!(
            "skill-lint budget --strict: within budget, every entry under the per-entry cap, every SKILL.md parsed{body}"
        );
    }
    Ok(())
}

/// Why a `--strict` budget run fails, one line per reason.
///
/// Two independent ways to lose text, so two conditions. Over the TOTAL, the
/// platform drops whole descriptions; past the PER-ENTRY cap, it cuts one
/// description mid-sentence, trigger phrases included. A strict gate that saw
/// only the first passed a corpus whose truncation report it had just printed.
///
/// A third: a `SKILL.md` the scan could not read, parse, or find a description
/// in. Skipped, it vanishes from both totals above, so the gate would pass a
/// corpus it never read — every finding is therefore a failure here.
fn strict_failures(report: &skill_lint::budget::BudgetReport, max_body_bytes: Option<usize>) -> Vec<String> {
    let mut failures = Vec::new();
    if report.over_budget() {
        failures.push(format!(
            "listing is {} chars, over the {}-char listing budget by {}",
            report.total_listing_chars,
            report.budget_chars,
            report.overage_chars()
        ));
    }
    for entry in report.truncated() {
        failures.push(format!(
            "'{}' description is {} chars, past the {}-char per-entry cap by {} — the platform discards the rest",
            entry.name, entry.desc_chars, report.max_desc_chars, entry.truncated_chars
        ));
    }
    if let Some(cap) = max_body_bytes {
        for entry in report.over_body_cap(cap) {
            failures.push(format!(
                "'{}' SKILL.md is {} bytes, past the {cap}-byte body cap by {} — loaded whole on every invocation",
                entry.name,
                entry.body_bytes,
                entry.body_bytes - cap
            ));
        }
    }
    failures.extend(report.findings.iter().map(ToString::to_string));
    failures
}

fn print_body_cap(report: &skill_lint::budget::BudgetReport, cap: usize) {
    let over = report.over_body_cap(cap);
    if over.is_empty() {
        match report.largest_body() {
            Some(largest) => println!(
                "\nSKILL.md body cap ({cap} bytes): all {} fit, largest {} bytes ({})",
                report.entries.len(),
                largest.body_bytes,
                largest.name
            ),
            None => println!("\nSKILL.md body cap ({cap} bytes): no skills scanned"),
        }
        return;
    }
    println!(
        "\nSKILL.md body cap ({cap} bytes): {} OVER, {} bytes past the cap in total",
        over.len(),
        over.iter().map(|e| e.body_bytes - cap).sum::<usize>()
    );
    for e in &over {
        println!("  {:7} bytes  {}  (+{})", e.body_bytes, e.name, e.body_bytes - cap);
    }
}

/// The `workflows` subcommand.
///
/// Its own function so `main` stays a dispatch; see [`run_claudemd`].
fn run_workflows(
    files: &[PathBuf],
    max_run_lines: usize,
    baseline: Option<&std::path::Path>,
    write_baseline: Option<&std::path::Path>,
    top: usize,
) -> Result<()> {
    use skill_lint::workflows::{Baseline, FsWorkflowSource, WorkflowConfig};

    let config = WorkflowConfig { max_run_lines };
    let known = match baseline {
        Some(path) => Baseline::load(path)?,
        None => Baseline::default(),
    };

    let source = FsWorkflowSource { paths: files };
    let report =
        skill_lint::workflows::lint_all(&source, &config, known).context("linting workflows")?;

    // Coverage before verdict, for the same reason claudemd does it: a green run
    // over the wrong file set should be visible, not merely green.
    println!("scanned {} workflow(s):", report.scans.len());
    for scan in &report.scans {
        let over = scan.runs.iter().filter(|r| r.shell_lines > config.max_run_lines).count();
        println!(
            "  {:<44} {:>3} block run(s), {:>3} over  |  {:>3} flow run(s)",
            scan.key,
            scan.runs.len(),
            over,
            scan.flow_runs
        );
    }

    println!(
        "\nblock-form run: steps {:>5}   ({} over the {}-line glue allowance)",
        report.block_runs(),
        report.over_limit(),
        config.max_run_lines
    );
    println!(
        "flow-form run: steps  {:>5}   (the target shape — never a finding)",
        report.flow_runs()
    );
    println!(
        "inline shell lines    {:>5}   (comments and blanks excluded)",
        report.total_shell_lines()
    );

    let longest = report.runs_by_size();
    if !longest.is_empty() {
        println!("\nlongest {top}:");
        for run in longest.iter().take(top) {
            let over = run.shell_lines.saturating_sub(config.max_run_lines);
            println!(
                "  {:>4} lines  (+{:>4})  {}{}  {}:{}",
                run.shell_lines,
                over,
                run.style,
                if run.body_lines > run.shell_lines { "*" } else { " " },
                run.key,
                run.line
            );
        }
        println!("  (`*` marks a step whose body also carries comments, which are not counted)");
    }

    if let Some(path) = write_baseline {
        let text = Baseline::render(&report.scans, &config);
        std::fs::write(path, &text)
            .with_context(|| format!("writing baseline {}", path.display()))?;
        println!(
            "\nwrote baseline {} ({} recorded step(s))",
            path.display(),
            text.lines().filter(|l| !l.starts_with('#')).count()
        );
        return Ok(());
    }

    if report.is_ok() {
        eprintln!(
            "skill-lint workflows: all checks passed ({} workflow(s), {} block run(s))",
            report.scans.len(),
            report.block_runs()
        );
    } else {
        eprintln!("\nskill-lint workflows: {} error(s):", report.errors.len());
        for err in &report.errors {
            eprintln!("  - {err}");
        }
        process::exit(1);
    }

    Ok(())
}

/// The `chain` subcommand.
fn run_chain(
    dirs: &[PathBuf],
    home: Option<PathBuf>,
    global: Option<PathBuf>,
    max_bytes: Option<usize>,
    max_file_bytes: Option<usize>,
    json: bool,
) -> Result<()> {
    use skill_lint::chain::{self, ChainConfig, ChainReport, Origin, RealFs, tilde};

    let home = match home {
        Some(home) => home,
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is unset; pass --home")?),
    };
    // Canonical, like every session directory, so `starts_with` compares like
    // with like: a symlinked $HOME must not make a directory under it look
    // outside it.
    let home = std::fs::canonicalize(&home).with_context(|| format!("resolving --home {}", home.display()))?;
    let global = global.unwrap_or_else(|| home.join(".claude/CLAUDE.md"));
    let config = ChainConfig { home, global, max_bytes, max_file_bytes };

    let sessions = dirs
        .iter()
        .map(|dir| {
            let dir = std::fs::canonicalize(dir).with_context(|| format!("resolving --dir {}", dir.display()))?;
            Ok(chain::resolve(&dir, &RealFs, &config))
        })
        .collect::<Result<Vec<_>>>()?;
    let report = ChainReport { sessions };

    if json {
        println!("{}", report.to_json(&config).context("rendering JSON")?);
    } else {
        let show = |path: &std::path::Path| tilde(path, &config.home);
        for session in &report.sessions {
            println!(
                "session {} — {} file(s), {} B",
                show(&session.dir),
                session.files.len(),
                session.total_bytes()
            );
            for file in &session.files {
                let (label, by) = match &file.origin {
                    Origin::Global => ("global", String::new()),
                    Origin::Chain => ("chain", String::new()),
                    Origin::Import { by, line } => ("import", format!("   <- {}:{line}", show(by))),
                };
                println!(
                    "  {:>8} B  {label:<6}  {}{}{by}",
                    file.bytes,
                    "  ".repeat(file.depth),
                    show(&file.path)
                );
            }
            match config.max_bytes {
                Some(cap) => println!("  total {} B of a {cap} B ceiling", session.total_bytes()),
                None => println!("  total {} B (no --max-bytes: reported, not gated)", session.total_bytes()),
            }
            for cycle in &session.cycles {
                println!(
                    "  cycle: {}:{} imports {}, already being loaded above it (a no-op, not a failure)",
                    show(&cycle.file),
                    cycle.line,
                    show(&cycle.target)
                );
            }
            println!();
        }
    }

    let files: usize = report.sessions.iter().map(|s| s.files.len()).sum();
    if report.is_ok() {
        eprintln!(
            "skill-lint chain: all checks passed ({} session(s), {files} file load(s))",
            report.sessions.len()
        );
    } else {
        let errors: Vec<_> = report.errors().collect();
        eprintln!("skill-lint chain: {} error(s):", errors.len());
        for err in errors {
            eprintln!("  - {err}");
        }
        process::exit(1);
    }

    Ok(())
}

/// Render the busiest few names of a census bucket, largest first.
///
/// A bare total says the load exists; the names say where it is.
fn top_names(counts: &std::collections::BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return String::new();
    }
    let mut pairs: Vec<(&String, &usize)> = counts.iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let shown: Vec<String> =
        pairs.iter().take(6).map(|(name, count)| format!("{name} {count}")).collect();
    let suffix = if pairs.len() > 6 { format!(", +{} more", pairs.len() - 6) } else { String::new() };
    format!("({}{suffix})", shown.join(", "))
}
