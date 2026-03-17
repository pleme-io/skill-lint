use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use skill_lint::check;

#[derive(Parser)]
#[command(name = "skill-lint", about = "Validate Claude Code skill maps")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run all checks: sync, frontmatter, map integrity, version.
    Check {
        /// Path to the skills directory (contains skill subdirs + skill-map.yaml).
        #[arg(long, default_value = ".")]
        skills_dir: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Check { skills_dir } => {
            let report = check::check_all(&skills_dir)
                .with_context(|| format!("checking {}", skills_dir.display()))?;

            if report.is_ok() {
                eprintln!(
                    "skill-lint: all checks passed ({} skills)",
                    report.skills_checked
                );
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
