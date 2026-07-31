use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use skill_lint::check::{self, CheckConfig};

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

        /// Show this many largest entries.
        #[arg(long, default_value_t = 20)]
        top: usize,

        /// Exit non-zero when over budget, for use as a gate.
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Budget {
            skills_dir,
            discover_under,
            window_tokens,
            budget_fraction,
            budget_chars,
            max_desc_chars,
            top,
            strict,
        } => {
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

            if strict && report.over_budget() {
                process::exit(1);
            }
            Ok(())
        }

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
        } => {
            use skill_lint::claudemd::{Baseline, ClaudeMdConfig, FsDocSource};

            let config = ClaudeMdConfig { max_entry_bytes, max_file_bytes, index_heading };
            let known = match baseline.as_deref() {
                Some(path) => Baseline::load(path)?,
                None => Baseline::default(),
            };

            let source = FsDocSource { paths: &files };
            let report = skill_lint::claudemd::lint_all(&source, &config, known)
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

            if let Some(path) = write_baseline.as_deref() {
                let text = Baseline::render(&report.scans, &config);
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
    }
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
