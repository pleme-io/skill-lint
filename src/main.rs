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

        /// Flag skills not verified within this many days as stale.
        #[arg(long)]
        max_age_days: Option<u32>,
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
            map_dir,
            max_age_days,
        } => {
            let config = CheckConfig {
                version: !skip_version,
                sync: !skip_sync,
                frontmatter: !skip_frontmatter,
                map_integrity: !skip_map_integrity,
                duplicate_concerns: !skip_map_integrity,
                max_age_days,
                today: None,
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
