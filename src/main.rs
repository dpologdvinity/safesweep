use clap::{Parser, Subcommand};
use safesweep::render::{self, human};
use safesweep::{Ctx, clean, collect, rules, tui, why, wsl};
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;
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
    /// Browse candidates interactively, select the safe ones and delete them
    Tui,
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

fn run(cli: Cli) -> Result<ExitCode, String> {
    let cmd = cli.cmd.unwrap_or(Cmd::Scan { json: false });
    if let Cmd::Why { interval_ms, top } = cmd {
        return why(interval_ms, top);
    }
    let ctx = Ctx::from_env()?;
    let selected = rules::select(&cli.categories)?;
    if matches!(cmd, Cmd::Tui) {
        if !std::io::stdout().is_terminal() {
            return Err("tui needs an interactive terminal".into());
        }
        let dir = ctx.home.join(".local/state/safesweep");
        let failed = tui::run(&selected, &ctx, &dir).map_err(|e| format!("terminal error: {e}"))?;
        return Ok(if failed {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        });
    }
    let rows = collect(&selected, &ctx);
    match cmd {
        Cmd::Why { .. } => unreachable!("handled above"),
        Cmd::Tui => unreachable!("handled above"),
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

fn why(interval_ms: u64, top: usize) -> Result<ExitCode, String> {
    // Sampling a zero-length window prints made-up numbers.
    if interval_ms < 100 {
        return Err("--interval-ms must be at least 100".into());
    }
    let proc_root = Path::new("/proc");
    let err = |e: std::io::Error| format!("cannot read /proc: {e}");
    let a = why::sample(proc_root).map_err(err)?;
    std::thread::sleep(Duration::from_millis(interval_ms));
    let b = why::sample(proc_root).map_err(err)?;
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
        let _ = writeln!(out, "{comm:<20} {pct:>6.1}  {n:>5}");
    }
    let _ = writeln!(out, "\n    PID   CPU%  COMMAND");
    for u in r.procs.iter().take(top) {
        let cmdline = fs::read(proc_root.join(u.pid.to_string()).join("cmdline"))
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .replace('\0', " ")
                    .trim()
                    .to_string()
            })
            .ok()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| format!("[{}]", u.comm));
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
