use clap::{Parser, Subcommand};
use safesweep::render::{self, human};
use safesweep::{Ctx, clean, collect, rules, wsl};
use std::process::ExitCode;
use std::{env, fs};

/// Clean caches and temp files that are provably safe to delete.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Only use this rule (repeatable)
    #[arg(long = "category", value_name = "ID", global = true)]
    categories: Vec<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List candidates and whether each is safe to delete (default)
    Scan {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Show what would be removed; remove it with --apply
    Clean {
        /// Actually delete
        #[arg(long)]
        apply: bool,
    },
}

fn main() -> ExitCode {
    run(Cli::parse()).unwrap_or_else(|msg| {
        eprintln!("error: {msg}");
        ExitCode::from(2)
    })
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let ctx = Ctx::from_env()?;
    let rows = collect(&rules::select(&cli.categories)?, &ctx);
    match cli.cmd.unwrap_or(Cmd::Scan { json: false }) {
        Cmd::Scan { json: true } => println!("{}", render::json(&rows)),
        Cmd::Scan { json: false } => print!("{}", render::table(&rows)),
        Cmd::Clean { apply } => {
            let dir = ctx.home.join(".local/state/safesweep");
            let out = match clean::clean(&rows, &ctx, apply, &dir) {
                Ok(out) => out,
                Err(e) => {
                    eprintln!("error: cannot write manifest in {}: {e}", dir.display());
                    return Ok(ExitCode::from(1));
                }
            };
            for line in &out.lines {
                println!("{line}");
            }
            println!(
                "{} {}",
                if apply { "freed" } else { "would free" },
                human(out.freed)
            );
            let on_wsl =
                fs::read_to_string(ctx.proc_root.join("version")).is_ok_and(|v| wsl::is_wsl(&v));
            if apply && out.freed > 0 && on_wsl {
                let distro = env::var("WSL_DISTRO_NAME").unwrap_or_else(|_| "<distro>".into());
                print!("\n{}", wsl::hint(&distro));
            }
            if out.failed > 0 {
                return Ok(ExitCode::from(1));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
