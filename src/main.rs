//! The `safesweep` command-line program: argument parsing and output only.
//!
//! The work is done by the `safesweep` library (see its crate documentation); this file turns
//! command-line arguments into library calls and library results into text and exit codes.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | success |
//! | 1 | the command ran but something failed: a removal failed or could not be logged, `/proc`  or the terminal could not be used |
//! | 2 | bad usage: unknown `--category`, unsafe `HOME` or `TMPDIR`, a flag that does not apply |

use clap::{CommandFactory, Parser, Subcommand};
use safesweep::render::{self, human};
use safesweep::{Ctx, clean, collect, rules, tui, why, wsl};
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
use std::{env, fs};

// The doc comments on `Cli` and `Cmd` are the `--help` text users see, so they are written for
// users, not for developers.

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
    /// Browse candidates interactively, select the safe ones and delete them
    Tui,
    /// Print a shell completion script, e.g. `safesweep completions bash > ~/.local/share/bash-completion/completions/safesweep`
    Completions { shell: clap_complete::Shell },
    /// Print the man page (roff)
    #[command(hide = true)]
    Man,
    /// Show which processes and programs are using the CPU
    Why {
        /// How long to sample
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        /// How many processes to list
        #[arg(long, default_value_t = 10)]
        top: usize,
    },
}

fn main() -> ExitCode {
    run(Cli::parse()).unwrap_or_else(|msg| {
        eprintln!("error: {msg}");
        ExitCode::from(2)
    })
}

/// Prints `msg` as an error and returns exit code 1: the command was valid but something on the
/// system failed. Exit code 2, for bad usage, comes from returning `Err` out of [`run`].
fn runtime_error(msg: &str) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::from(1)
}

/// Runs the chosen subcommand. `Err` means bad usage (exit code 2); everything else is reported
/// through the returned exit code.
fn run(cli: Cli) -> Result<ExitCode, String> {
    let cmd = cli.cmd.unwrap_or(Cmd::Scan { json: false });
    // These subcommands do not use the rule table, so a --category would silently do nothing.
    let ignores_category = match &cmd {
        Cmd::Why { .. } => Some("why"),
        Cmd::Completions { .. } => Some("completions"),
        Cmd::Man => Some("man"),
        _ => None,
    };
    if let Some(name) = ignores_category
        && !cli.categories.is_empty()
    {
        return Err(format!("--category does not apply to {name}"));
    }
    match cmd {
        Cmd::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "safesweep",
                &mut std::io::stdout(),
            );
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Man => Ok(
            match clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => runtime_error(&format!("cannot write man page: {e}")),
            },
        ),
        Cmd::Why { interval_ms, top } => run_why(interval_ms, top),
        Cmd::Scan { json } => {
            let (_, rows) = scan_rows(&cli.categories)?;
            if json {
                println!("{}", render::json(&rows));
            } else {
                print!("{}", render::table(&rows));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Clean { apply } => {
            let (ctx, rows) = scan_rows(&cli.categories)?;
            Ok(run_clean(&ctx, &rows, apply))
        }
        Cmd::Tui => run_tui(&cli.categories),
    }
}

/// Validates the environment and the selected rule ids, and returns the context together with
/// the judged candidates of those rules.
///
/// # Errors
///
/// Bad usage: an unsafe `HOME` or `TMPDIR` (see [`Ctx::from_env`]) or an unknown rule id.
fn scan_rows(categories: &[String]) -> Result<(Ctx, Vec<safesweep::Row>), String> {
    let ctx = Ctx::from_env()?;
    let selected = rules::select(categories)?;
    let rows = collect(&selected, &ctx);
    Ok((ctx, rows))
}

/// Where the log of removals is kept: `~/.local/state/safesweep/`.
fn manifest_dir(ctx: &Ctx) -> PathBuf {
    ctx.home.join(".local/state/safesweep")
}

/// `safesweep clean [--apply]`: prints what was (or would be) removed and the space freed, plus
/// the compaction hint on WSL after a real cleanup. Returns exit code 1 if any removal failed or
/// the log could not be created.
fn run_clean(ctx: &Ctx, rows: &[safesweep::Row], apply: bool) -> ExitCode {
    let dir = manifest_dir(ctx);
    let out = match clean::clean(rows, ctx, apply, &dir) {
        Ok(out) => out,
        Err(e) => {
            eprintln!("error: cannot write manifest in {}: {e}", dir.display());
            return ExitCode::from(1);
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
    let on_wsl = fs::read_to_string(ctx.proc_root.join("version")).is_ok_and(|v| wsl::is_wsl(&v));
    if apply && out.freed > 0 && on_wsl {
        let distro = env::var("WSL_DISTRO_NAME").unwrap_or_else(|_| "<distro>".into());
        print!("\n{}", wsl::hint(&distro));
    }
    if out.failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// `safesweep tui`: the interactive explorer.
///
/// # Errors
///
/// Bad usage (exit 2) if standard output is not a terminal, or `HOME`, `TMPDIR` or the category
/// ids are invalid. A terminal failure, or a delete that failed during the session, exits 1.
fn run_tui(categories: &[String]) -> Result<ExitCode, String> {
    let ctx = Ctx::from_env()?;
    let selected = rules::select(categories)?;
    if !std::io::stdout().is_terminal() {
        return Err("tui needs an interactive terminal".into());
    }
    Ok(match tui::run(&selected, &ctx, &manifest_dir(&ctx)) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(e) => runtime_error(&format!("terminal error: {e}")),
    })
}

/// `safesweep why`: samples the CPU for `interval_ms`, then prints how busy the machine is, CPU
/// per program, and the busiest processes with their command lines.
///
/// All output is built in memory and written once, ignoring write errors, so that
/// `safesweep why | head` (which closes the pipe early) is not a crash.
///
/// # Errors
///
/// Returns a usage error (exit 2) if `interval_ms` is under 100: a zero-length window would
/// print made-up numbers. An unreadable `/proc` exits 1 instead.
fn run_why(interval_ms: u64, top: usize) -> Result<ExitCode, String> {
    // Sampling a zero-length window prints made-up numbers.
    if interval_ms < 100 {
        return Err("--interval-ms must be at least 100".into());
    }
    let proc_root = Path::new("/proc");
    let err = |e: std::io::Error| runtime_error(&format!("cannot read /proc: {e}"));
    let a = match why::sample(proc_root) {
        Ok(s) => s,
        Err(e) => return Ok(err(e)),
    };
    std::thread::sleep(Duration::from_millis(interval_ms));
    let b = match why::sample(proc_root) {
        Ok(s) => s,
        Err(e) => return Ok(err(e)),
    };
    let r = why::diff(&a, &b);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "CPU busy: {:.0}% of {} cores (sampled {:.1}s)",
        r.busy_pct,
        r.ncpu,
        interval_ms as f64 / 1000.0
    );
    let _ = writeln!(out, "\nPROGRAM               CPU%  PROCS");
    for (comm, pct, n) in why::by_program(&r.procs).iter().take(top) {
        let _ = writeln!(out, "{:<20} {pct:>6.1}  {n:>5}", render::escape_text(comm));
    }
    let _ = writeln!(out, "\n    PID   CPU%  COMMAND");
    for u in r.procs.iter().take(top) {
        let raw = fs::read(proc_root.join(u.pid.to_string()).join("cmdline")).unwrap_or_default();
        let cmdline = why::command_line(&raw, &u.comm);
        let cmdline: String = cmdline.chars().take(100).collect();
        let _ = writeln!(out, "{:>7} {:>6.1}  {cmdline}", u.pid, u.pct);
    }
    if r.procs.is_empty() {
        let _ = writeln!(out, "(no process used measurable CPU)");
    }
    if fs::read_to_string(proc_root.join("version")).is_ok_and(|v| wsl::is_wsl(&v)) {
        let _ = writeln!(
            out,
            "\nWSL: only Linux processes are visible here. If Task Manager shows vmmem busy\n\
             but this list is quiet, the load is on the Windows side."
        );
    }
    // Ignore write errors: `safesweep why | head` closes the pipe early, and that is not a crash.
    let _ = std::io::stdout().write_all(out.as_bytes());
    Ok(ExitCode::SUCCESS)
}
