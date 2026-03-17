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
        /// Path to the skills directory (contains skill subdirs + skill-map.yaml).
        #[arg(long, default_value = ".")]
        skills_dir: PathBuf,

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
        } => {
            let config = CheckConfig {
                version: !skip_version,
                sync: !skip_sync,
                frontmatter: !skip_frontmatter,
                map_integrity: !skip_map_integrity,
                duplicate_concerns: !skip_map_integrity,
            };

            let report = check::check_path_with_config(&skills_dir, &config)
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
