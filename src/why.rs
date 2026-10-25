//! "What is using my CPU?": two samples of `/proc`, compared.
//!
//! The kernel keeps running totals of CPU time (in *ticks*, normally 1/100 s) for the whole
//! machine (`/proc/stat`) and for each process (`/proc/<pid>/stat`). One reading says nothing
//! about *now*, so [`sample`] is called twice, some time apart, and [`diff`] turns the
//! difference into percentages. This is the same method `top` uses.
//!
//! This module only reads `/proc`; it changes nothing and shares no state with the cleaning
//! code. Text that came from other processes (names, command lines) is untrusted and must be
//! escaped before printing, see [`command_line`].

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

/// Everything read from `/proc` at one instant: the machine's CPU counters and one entry per
/// process. Two of these, taken apart in time, are the input to [`diff`].
pub struct Sample {
    /// Total CPU time of all cores since boot, in ticks: the first eight fields of the aggregate
    /// `cpu` line in `/proc/stat` (user, nice, system, idle, iowait, irq, softirq, steal).
    /// Guest time is left out because the kernel already counts it inside user time.
    pub total: u64,
    /// Ticks spent idle, including waiting for input or output.
    pub idle: u64,
    /// Number of CPU cores (the number of `cpuN` lines).
    pub ncpu: usize,
    /// For each process id: `(name, ticks used, start time)`. The name is `comm`, at most 15
    /// characters. Ticks are user plus system time. The start time (in ticks since boot) tells
    /// a new process that reused an old id from the same process seen again.
    pub procs: HashMap<u32, (String, u64, u64)>,
}

/// One process's share of the CPU between two samples.
pub struct Usage {
    /// Process id.
    pub pid: u32,
    /// The process name (`comm`), as the kernel reports it. May contain any characters: escape
    /// before printing.
    pub comm: String,
    /// Percent of one core, like `top`.
    pub pct: f64,
}

/// The result of comparing two [`Sample`]s.
pub struct Report {
    /// Percent of the whole machine that was busy, from 0 to 100.
    pub busy_pct: f64,
    /// Number of CPU cores, so percentages per process can be read against `ncpu * 100`.
    pub ncpu: usize,
    /// Processes that used CPU in the interval, busiest first. Processes that did nothing, started
    /// or exited in the interval are not listed.
    pub procs: Vec<Usage>,
}

/// Reads `(total ticks, idle ticks, core count)` from the text of `/proc/stat`.
///
/// Returns `None` if there is no aggregate `cpu` line or it has too few fields. See [`Sample`]
/// for what the numbers mean.
pub fn parse_total(proc_stat: &str) -> Option<(u64, u64, usize)> {
    let line = proc_stat.lines().find(|l| l.starts_with("cpu "))?;
    let f: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    let idle = f.get(3)? + f.get(4).unwrap_or(&0);
    let ncpu = proc_stat
        .lines()
        .filter(|l| {
            l.strip_prefix("cpu")
                .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
        })
        .count();
    // guest and guest_nice (fields 9, 10) are already counted in user and nice
    Some((f.iter().take(8).sum(), idle, ncpu))
}

/// Reads `(name, ticks used, start time)` from the text of `/proc/<pid>/stat`.
///
/// The name sits in parentheses and may itself contain spaces and parentheses, so it is taken
/// from the first `(` to the *last* `)`. The fields after it are counted from there: user time
/// and system time are fields 14 and 15 of the file, the start time is field 22.
///
/// Returns `None` for a line that is too short or has non-numeric fields.
pub fn parse_proc(stat: &str) -> Option<(String, u64, u64)> {
    let (open, close) = (stat.find('(')?, stat.rfind(')')?);
    let comm = stat.get(open + 1..close)?.to_string();
    let f: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    Some((comm, ticks, f.get(19)?.parse().ok()?))
}

/// Reads the machine's CPU counters and every process's counters from `proc_root` (normally
/// `/proc`) at this moment.
///
/// A process that exits while being read is simply left out.
///
/// # Errors
///
/// Returns an error if `proc_root` or its `stat` file cannot be read or understood.
pub fn sample(proc_root: &Path) -> io::Result<Sample> {
    let stat = fs::read_to_string(proc_root.join("stat"))?;
    let (total, idle, ncpu) =
        parse_total(&stat).ok_or_else(|| io::Error::other("cannot parse /proc/stat"))?;
    let mut procs = HashMap::new();
    for dir in fs::read_dir(proc_root)?.flatten() {
        let Ok(pid) = dir.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        // processes can exit between listing and reading
        // read as bytes: the process name may be any bytes, and read_to_string would drop it
        if let Some(p) = fs::read(dir.path().join("stat"))
            .ok()
            .and_then(|b| parse_proc(&String::from_utf8_lossy(&b)))
        {
            procs.insert(pid, p);
        }
    }
    Ok(Sample {
        total,
        idle,
        ncpu,
        procs,
    })
}

/// Compares two samples, `a` taken before `b`, and reports CPU use between them.
///
/// A process's percentage is the share of *one core* it used, like `top` shows, so a busy
/// process on a 12-core machine can reach 1200%. The machine-wide figure is clamped to 0 to 100.
/// A process that has the same id in both samples but a different start time is a different,
/// newer process and is ignored.
///
/// If no time passed between the samples (identical totals), nothing is reported as busy:
/// dividing by zero would produce made-up numbers.
pub fn diff(a: &Sample, b: &Sample) -> Report {
    let elapsed = b.total.saturating_sub(a.total) as f64;
    if elapsed == 0.0 {
        return Report {
            busy_pct: 0.0,
            ncpu: b.ncpu,
            procs: Vec::new(),
        };
    }
    let idle = b.idle.saturating_sub(a.idle) as f64;
    let per_core = elapsed / b.ncpu.max(1) as f64;
    let mut procs: Vec<Usage> = b
        .procs
        .iter()
        .filter_map(|(&pid, (comm, ticks, start))| {
            let (_, before, started) = a.procs.get(&pid)?;
            if started != start {
                return None;
            }
            let used = ticks.saturating_sub(*before);
            (used > 0).then(|| Usage {
                pid,
                comm: comm.clone(),
                pct: used as f64 / per_core * 100.0,
            })
        })
        .collect();
    procs.sort_by(|x, y| y.pct.total_cmp(&x.pct));
    Report {
        busy_pct: ((elapsed - idle) / elapsed * 100.0).clamp(0.0, 100.0),
        ncpu: b.ncpu,
        procs,
    }
}

/// A process's command line, safe to print, from the raw bytes of `/proc/<pid>/cmdline`.
///
/// The arguments are separated by NUL bytes in the file; they are joined with spaces. Any
/// process can put anything there, including terminal escape sequences, so the text is escaped
/// with [`crate::render::escape_text`]. Kernel threads have no command line; for them the
/// escaped name is returned in brackets, `[kworker/0:1]`.
pub fn command_line(raw: &[u8], comm: &str) -> String {
    let args: Vec<String> = raw
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| crate::render::escape_text(&String::from_utf8_lossy(a)))
        .collect();
    if args.is_empty() {
        format!("[{}]", crate::render::escape_text(comm))
    } else {
        args.join(" ")
    }
}

/// Truncates a command line to at most `max_chars` characters, appending `…` if truncated.
pub fn fit_cmdline(cmdline: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if cmdline.chars().nth(max_chars).is_some() {
        let prefix: String = cmdline.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{prefix}…")
    } else {
        cmdline.to_string()
    }
}

/// Adds up the processes that share a name: `(name, total percent, number of processes)`,
/// busiest first. Twelve `rustc` jobs then show as one line instead of twelve.
pub fn by_program(procs: &[Usage]) -> Vec<(String, f64, usize)> {
    let mut g: HashMap<&str, (f64, usize)> = HashMap::new();
    for u in procs {
        let e = g.entry(&u.comm).or_default();
        e.0 += u.pct;
        e.1 += 1;
    }
    let mut out: Vec<_> = g
        .into_iter()
        .map(|(c, (p, n))| (c.to_string(), p, n))
        .collect();
    out.sort_by(|x, y| y.1.total_cmp(&x.1));
    out
}

/// Unit tests for parsing, the percentage arithmetic and the handling of hostile names.
#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 0 50 800 50 0 0 0 7000 7000\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\nintr 1\n";

    fn s(total: u64, idle: u64, procs: &[(u32, &str, u64)]) -> Sample {
        Sample {
            total,
            idle,
            ncpu: 2,
            procs: procs
                .iter()
                .map(|&(p, c, t)| (p, (c.to_string(), t, 1)))
                .collect(),
        }
    }

    #[test]
    fn parses_total_idle_and_cpus() {
        assert_eq!(parse_total(STAT), Some((1000, 850, 2)));
    }

    #[test]
    fn parses_comm_with_spaces_and_parens() {
        let line = "1234 (my (odd) proc) R 1 1 1 0 -1 0 0 0 0 0 30 12 0 0 20 0 1 0 5 0 0";
        assert_eq!(parse_proc(line), Some(("my (odd) proc".into(), 42, 5)));
    }

    #[test]
    fn diff_computes_core_percent_and_busy() {
        // 2 cpus, 200 ticks elapsed (100 per core), 50 idle -> 75% busy
        let a = s(
            1000,
            500,
            &[(1, "rustc", 0), (2, "node", 10), (3, "gone", 5)],
        );
        let b = s(
            1200,
            550,
            &[(1, "rustc", 90), (2, "node", 30), (4, "new", 7)],
        );
        let r = diff(&a, &b);
        assert!((r.busy_pct - 75.0).abs() < 0.01);
        let got: Vec<_> = r
            .procs
            .iter()
            .map(|u| (u.pid, u.pct.round() as u32))
            .collect();
        assert_eq!(got, [(1, 90), (2, 20)]);
    }

    #[test]
    fn diff_ignores_reused_pid() {
        // same pid, different start time: a new process, not a continuation
        let a = s(1000, 500, &[(2, "old", 5)]);
        let mut b = s(1200, 550, &[(2, "new", 500)]);
        b.procs.get_mut(&2).unwrap().2 = 99;
        assert!(diff(&a, &b).procs.is_empty());
    }

    #[test]
    fn diff_with_no_elapsed_time_is_zero_not_made_up() {
        let a = s(1000, 500, &[(1, "x", 0)]);
        let b = s(1000, 500, &[(1, "x", 50)]);
        let r = diff(&a, &b);
        assert_eq!(r.busy_pct, 0.0);
        assert!(r.procs.is_empty());
    }

    #[test]
    fn groups_by_program() {
        let u = |pid, comm: &str, pct| Usage {
            pid,
            comm: comm.into(),
            pct,
        };
        let g = by_program(&[u(1, "rustc", 40.0), u(2, "node", 50.0), u(3, "rustc", 30.0)]);
        assert_eq!(g, [("rustc".into(), 70.0, 2), ("node".into(), 50.0, 1)]);
    }

    #[test]
    fn samples_real_proc() {
        let smp = sample(Path::new("/proc")).unwrap();
        assert!(smp.ncpu >= 1 && smp.total > 0);
        assert!(smp.procs.contains_key(&std::process::id()));
    }

    #[test]
    fn command_line_is_escaped() {
        let raw = b"evil\x1b]52;c;aGk=\x07\0arg\nnext\0";
        let got = command_line(raw, "evil");
        assert!(
            !got.contains('\x1b') && !got.contains('\x07') && !got.contains('\n'),
            "{got:?}"
        );
        assert!(
            got.starts_with("evil\\u{1b}]52") && got.contains(" arg\\nnext"),
            "{got:?}"
        );
        assert_eq!(command_line(b"", "kworker/0:1"), "[kworker/0:1]");
        assert_eq!(command_line(b"", "x\x1by"), "[x\\u{1b}y]");
    }

    #[test]
    fn fit_cmdline_truncates_with_ellipsis() {
        assert_eq!(fit_cmdline("short", 10), "short");
        assert_eq!(fit_cmdline("exact_ten!", 10), "exact_ten!");
        assert_eq!(fit_cmdline("eleven_char", 10), "eleven_ch…");
        assert_eq!(fit_cmdline("", 5), "");
        assert_eq!(fit_cmdline("abc", 0), "");
    }

    #[test]
    fn sample_keeps_processes_whose_name_is_not_utf8() {
        // any process can set its name (comm) to arbitrary bytes; it must not vanish from the list
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("stat"),
            "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 100 0 50 800 50 0 0 0 0 0\n",
        )
        .unwrap();
        fs::create_dir(root.path().join("4242")).unwrap();
        let mut stat = b"4242 (bad".to_vec();
        stat.extend_from_slice(b"\xff\xfe");
        stat.extend_from_slice(b") S 1 1 1 0 -1 0 0 0 0 0 30 12 0 0 20 0 1 0 5 0 0\n");
        fs::write(root.path().join("4242/stat"), stat).unwrap();
        let smp = sample(root.path()).unwrap();
        let (comm, ticks, _) = &smp.procs[&4242];
        assert!(comm.starts_with("bad"), "{comm:?}");
        assert_eq!(*ticks, 42);
    }
}
