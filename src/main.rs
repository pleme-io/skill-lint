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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Check {
            skills_dir,
            skip_version,
            skip_sync,
            skip_frontmatter,
            skip_map_integrity,
            skip_path_resolution,
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
    }
}
