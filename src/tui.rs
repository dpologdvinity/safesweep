//! The interactive explorer behind `safesweep tui`.
//!
//! The screen lists the same candidates as `scan`, largest first. The user moves with the arrow
//! keys, selects the safe rows, and deletes the selection after confirming.
//!
//! # Design
//!
//! - **State and screen are separate.** [`App`] holds everything that can change (the rows, the
//!   cursor, the selection, the prompt) and [`App::handle_key`] is a plain function from a key
//!   to a new state. [`draw`] only paints the current state. Neither needs a real terminal, so
//!   both are unit-tested with a fake one. Only [`run`] talks to the real terminal.
//! - **Deleting goes through [`clean::clean`]**, exactly like `safesweep clean --apply`: every
//!   chosen row is re-measured and re-checked right before removal and logged to the manifest.
//!   Nothing in this module removes a file itself, so the interface cannot delete anything the
//!   command line would refuse to.
//! - **Rows that can never be deleted are not selectable.** Only rows whose status is `ok` can
//!   be selected; rows owned by other users are hidden altogether.
//! - **Scans can be cancelled.** Scanning runs on a background thread while the terminal is in
//!   raw mode, where Ctrl-C does not interrupt the program, so `q` or Esc cancels explicitly.

use crate::render::{escape_path, human};
use crate::rules::Rule;
use crate::{Ctx, Row, clean, collect};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

/// What the main loop should do after a key press.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Keep running and redraw.
    Continue,
    /// Leave the program.
    Quit,
}

/// Everything the explorer remembers between key presses. See the module docs.
pub struct App {
    /// The candidates being shown, largest first. Rows owned by other users are not in here.
    pub rows: Vec<Row>,
    /// The cursor: which row is highlighted (and the scroll position). `None` when `rows` is
    /// empty.
    pub state: ListState,
    /// Indexes into `rows`; only rows whose status is `ok` can be in here.
    pub selected: BTreeSet<usize>,
    /// True while the "Delete N items? y to confirm" prompt is showing. The next key either
    /// confirms (`y`) or cancels (anything else).
    pub confirming: bool,
    /// A one-line note for the status line: the result of the last delete, or why a key did
    /// nothing ("nothing selected").
    pub message: String,
    /// Set when a delete failed or could not start; makes `run` report failure.
    pub failed: bool,
    /// Rows left out because they belong to other users (never deletable, only clutter).
    pub hidden: usize,
    /// Set after a delete: `run` rescans in the background so the scan can be cancelled.
    pub needs_rescan: bool,
}

impl App {
    /// Starts with `rows` (as returned by [`crate::collect`]), nothing selected, and the cursor on
    /// the first row. Rows owned by other users are removed from the list and only counted.
    pub fn new(rows: Vec<Row>) -> App {
        let (rows, hidden) = shown(rows);
        let mut state = ListState::default();
        state.select((!rows.is_empty()).then_some(0));
        App {
            rows,
            state,
            selected: BTreeSet::new(),
            confirming: false,
            message: String::new(),
            failed: false,
            hidden,
            needs_rescan: false,
        }
    }

    /// Total size of the selected rows, in bytes.
    pub fn selected_bytes(&self) -> u64 {
        self.selected.iter().map(|&i| self.rows[i].bytes).sum()
    }

    /// Moves the cursor one row down or up and stops at the ends. Does nothing for an empty list.
    fn step(&mut self, down: bool) {
        let Some(i) = self.state.selected() else {
            return;
        };
        let last = self.rows.len() - 1;
        self.state.select(Some(if down {
            (i + 1).min(last)
        } else {
            i.saturating_sub(1)
        }));
    }

    /// Selects or unselects the row under the cursor. A row that is not `ok` cannot be selected;
    /// the reason is shown in the status line instead.
    fn toggle(&mut self) {
        let Some(i) = self.state.selected() else {
            return;
        };
        if self.rows[i].status != "ok" {
            self.message = format!("not selectable: {}", self.rows[i].status);
        } else if !self.selected.remove(&i) {
            self.selected.insert(i);
        }
    }

    /// Removes the selected rows through the same guarded path as `safesweep clean --apply`,
    /// reports the result in `message`, and asks the main loop for a rescan so the list shows
    /// what is really left.
    ///
    /// The message counts what was removed, what failed and what the re-check skipped, and
    /// quotes the first row that did not go, so a failure is never silent. A failed or unlogged
    /// delete sets `failed`, which makes the program exit with code 1.
    fn delete(&mut self, ctx: &Ctx, manifest_dir: &Path) {
        let chosen: Vec<Row> = self
            .selected
            .iter()
            .map(|&i| self.rows[i].clone())
            .collect();
        self.message = match clean::clean(&chosen, ctx, true, manifest_dir) {
            Ok(o) => {
                let skipped: Vec<&String> = o
                    .lines
                    .iter()
                    .filter(|l| l.starts_with("skipped"))
                    .collect();
                self.failed |= o.failed > 0;
                let mut msg = format!(
                    "freed {} ({} removed, {} failed",
                    human(o.freed),
                    o.removed,
                    o.failed
                );
                if !skipped.is_empty() {
                    msg += &format!(", {} skipped", skipped.len());
                }
                msg.push(')');
                // the first row that did not go, with its reason, so a failure is never silent
                match o
                    .lines
                    .iter()
                    .find(|l| l.starts_with("failed"))
                    .or(skipped.first().copied())
                {
                    Some(line) => format!("{msg}  {line}"),
                    None => msg,
                }
            }
            Err(e) => {
                self.failed = true;
                format!(
                    "nothing deleted: cannot write manifest in {}: {e}",
                    manifest_dir.display()
                )
            }
        };
        self.selected.clear();
        self.needs_rescan = true;
    }

    /// Replaces the list with the result of a rescan, clears the selection, and keeps the cursor
    /// near where it was. Rows owned by other users are hidden, as in [`App::new`].
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        (self.rows, self.hidden) = shown(rows);
        self.selected.clear();
        self.needs_rescan = false;
        let keep = self
            .state
            .selected()
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.state.select((!self.rows.is_empty()).then_some(keep));
    }

    /// Applies one key press and says whether to keep running.
    ///
    /// | Key | Effect |
    /// |---|---|
    /// | `j`, Down / `k`, Up | move the cursor |
    /// | Space | select or unselect the current row |
    /// | `a` / `n` | select every safe row / clear the selection |
    /// | `d`, Enter | ask for confirmation (only if something is selected) |
    /// | `y` (at the prompt) | delete; any other key cancels |
    /// | `q`, Esc | quit |
    ///
    /// Deleting re-runs the full guard check through [`clean::clean`] and then asks [`run`] for a
    /// rescan. `manifest_dir` is where removals are logged.
    pub fn handle_key(&mut self, code: KeyCode, ctx: &Ctx, manifest_dir: &Path) -> Action {
        if self.confirming {
            self.confirming = false;
            if code == KeyCode::Char('y') {
                self.delete(ctx, manifest_dir);
            } else {
                self.message = "cancelled".into();
            }
            return Action::Continue;
        }
        self.message.clear();
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => self.step(true),
            KeyCode::Up | KeyCode::Char('k') => self.step(false),
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('a') => {
                self.selected = (0..self.rows.len())
                    .filter(|&i| self.rows[i].status == "ok")
                    .collect();
            }
            KeyCode::Char('n') => self.selected.clear(),
            KeyCode::Char('d') | KeyCode::Enter if self.selected.is_empty() => {
                self.message = "nothing selected".into();
            }
            KeyCode::Char('d') | KeyCode::Enter => self.confirming = true,
            _ => {}
        }
        Action::Continue
    }
}

/// Splits off the rows owned by other users: they can never be deleted, and listing them only
/// buries the real candidates (and lets another user fill the list with junk names in `/tmp`).
/// Returns the rows to show and how many were left out.
fn shown(rows: Vec<Row>) -> (Vec<Row>, usize) {
    let total = rows.len();
    let rows: Vec<Row> = rows
        .into_iter()
        .filter(|r| r.status != "skipped: owned by another user")
        .collect();
    let hidden = total - rows.len();
    (rows, hidden)
}

/// Width of the size bar in a row, in characters.
const BAR: usize = 12;

/// A bar of [`BAR`] characters showing `bytes` relative to the largest row (`max`):
/// `████████░░░░`. An empty list (`max` of 0) gives an empty bar.
fn bar(bytes: u64, max: u64) -> String {
    let filled = if max == 0 {
        0
    } else {
        ((bytes as f64 / max as f64) * BAR as f64).round() as usize
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR - filled))
}

/// The key a terminal event stands for, if any.
///
/// Only plain key *presses* count: a release or repeat is ignored, and a key held with Ctrl or
/// Alt is ignored, so Ctrl-D cannot open the delete prompt and Ctrl-Y cannot confirm it. Shift is
/// allowed (capital letters). Ctrl-C is translated to Esc, which quits, because in raw mode the
/// terminal no longer turns it into an interrupt.
pub fn translate(key: ratatui::crossterm::event::KeyEvent) -> Option<KeyCode> {
    use ratatui::crossterm::event::KeyModifiers;
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
        return Some(KeyCode::Esc);
    }
    (key.modifiers - KeyModifiers::SHIFT)
        .is_empty()
        .then_some(key.code)
}

/// Shortens `s` from the left to at most `width` characters, marking the cut with `…`, so the
/// end of a path (the part that tells similar rows apart) stays visible: `…/cache/file`.
/// A string that already fits is returned unchanged; a `width` of 0 gives an empty string.
pub fn fit_left(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let tail: String = s.chars().skip(n - (width - 1)).collect();
    format!("…{tail}")
}

/// Paints the "Scanning…" screen shown while a scan runs, before there are rows to draw.
pub fn draw_scanning(f: &mut Frame) {
    f.render_widget(
        ratatui::text::Span::raw("Scanning…  (q to cancel)"),
        f.area(),
    );
}

/// Paints the whole screen from `app`: the list of rows (mark, size, size bar, rule, path, and
/// the skip reason for rows that are not safe), a status line, and a line of key help.
///
/// Paths are escaped and cut on the left to fit the width; the skip reason is always kept. A
/// highlighted cursor row is drawn reversed, skipped rows in grey.
pub fn draw(f: &mut Frame, app: &mut App) {
    let [list_area, status_area, help_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let max = app.rows.iter().map(|r| r.bytes).max().unwrap_or(0);
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let ok = r.status == "ok";
            let mark = match (ok, app.selected.contains(&i)) {
                (false, _) => " - ",
                (true, true) => "[x]",
                (true, false) => "[ ]",
            };
            let tail = if ok {
                String::new()
            } else {
                format!("  ({})", r.status)
            };
            let head = format!(
                "{mark} {:>10} {} {:<9} ",
                human(r.bytes),
                bar(r.bytes, max),
                r.category
            );
            // 2 columns go to the highlight symbol; the path gets what is left after head and reason
            let room = (list_area.width as usize)
                .saturating_sub(2 + head.chars().count() + tail.chars().count());
            let text = format!(
                "{head}{}{tail}",
                fit_left(&escape_path(&r.path), room.max(1))
            );
            ListItem::new(text).style(if ok {
                Style::new()
            } else {
                Style::new().fg(Color::DarkGray)
            })
        })
        .collect();
    let list = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    f.render_stateful_widget(list, list_area, &mut app.state);

    let n = app.selected.len();
    let status = if app.confirming {
        format!(
            "Delete {n} item{} ({})? y to confirm, any other key cancels",
            if n == 1 { "" } else { "s" },
            human(app.selected_bytes())
        )
    } else {
        let hidden = if app.hidden > 0 {
            format!("  ({} hidden: other users')", app.hidden)
        } else {
            String::new()
        };
        format!(
            "selected: {n} item{}, {}{hidden}   {}",
            if n == 1 { "" } else { "s" },
            human(app.selected_bytes()),
            app.message
        )
    };
    let style = if app.confirming {
        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    f.render_widget(ratatui::text::Span::styled(status, style), status_area);
    f.render_widget(
        ratatui::text::Span::styled(
            "j/k move  space select  a all safe  n none  d delete  q quit",
            Style::new().fg(Color::DarkGray),
        ),
        help_area,
    );
}

/// Collects rows on a background thread while showing the scanning screen.
///
/// Returns `Ok(None)` if the user cancels with `q` or Esc: raw mode swallows Ctrl-C, and a
/// `pycache` or `artifacts` scan can take minutes. The background thread is left to finish on
/// its own; its result is discarded.
///
/// # Errors
///
/// Returns an error if the terminal cannot be drawn to or read from.
fn scan(
    terminal: &mut ratatui::DefaultTerminal,
    rules: &[&'static Rule],
    ctx: &Ctx,
) -> io::Result<Option<Vec<Row>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (rules, ctx) = (rules.to_vec(), ctx.clone());
    std::thread::spawn(move || {
        let _ = tx.send(collect(&rules, &ctx));
    });
    loop {
        terminal.draw(draw_scanning)?;
        if let Ok(rows) = rx.try_recv() {
            return Ok(Some(rows));
        }
        if event::poll(std::time::Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && matches!(translate(key), Some(KeyCode::Char('q') | KeyCode::Esc))
        {
            return Ok(None);
        }
    }
}

/// Runs the interactive explorer on the real terminal until the user quits.
///
/// Scans `rules` first (cancellable), then loops: draw, wait for a key, apply it, and rescan
/// after a delete. The terminal is always restored, whether the loop ends normally or with an
/// error.
///
/// Returns `Ok(true)` if any delete failed or could not be logged, so the caller can exit with
/// code 1.
///
/// # Errors
///
/// Returns an error if the terminal cannot be set up, drawn to or read from.
pub fn run(rules: &[&'static Rule], ctx: &Ctx, manifest_dir: &Path) -> io::Result<bool> {
    let mut terminal = ratatui::try_init()?;
    let mut ctx = ctx.clone();
    let result = (|| -> io::Result<bool> {
        ctx.now = std::time::SystemTime::now();
        let Some(rows) = scan(&mut terminal, rules, &ctx)? else {
            return Ok(false);
        };
        let mut app = App::new(rows);
        loop {
            terminal.draw(|f| draw(f, &mut app))?;
            if let Event::Key(key) = event::read()?
                && let Some(code) = translate(key)
            {
                ctx.now = std::time::SystemTime::now();
                if app.handle_key(code, &ctx, manifest_dir) == Action::Quit {
                    return Ok(app.failed);
                }
            }
            if app.needs_rescan {
                ctx.now = std::time::SystemTime::now();
                match scan(&mut terminal, rules, &ctx)? {
                    Some(rows) => app.set_rows(rows),
                    None => return Ok(app.failed),
                }
            }
        }
    })();
    ratatui::restore();
    result
}

/// Unit tests. The state machine and the drawing are tested without a terminal (keys are fed
/// to [`App::handle_key`], the screen is drawn into an in-memory buffer); deletions run against
/// a temporary directory with a moved clock.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{RULES, Regen};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn tmp_rule() -> &'static Rule {
        RULES.iter().find(|r| r.id == "tmp").unwrap()
    }

    fn row(name: &str, bytes: u64, status: &str) -> Row {
        Row {
            category: "tmp",
            path: PathBuf::from(format!("/t/{name}")),
            bytes,
            regen: Regen::Free,
            status: status.into(),
            rule: tmp_rule(),
        }
    }

    fn app() -> App {
        App::new(vec![
            row("big", 3 << 20, "ok"),
            row("busy", 2 << 20, "skipped: in use by a process"),
            row("small", 1 << 20, "ok"),
        ])
    }

    fn keys(app: &mut App, ks: &[KeyCode], ctx: &Ctx, dir: &Path) {
        for &k in ks {
            app.handle_key(k, ctx, dir);
        }
    }

    fn ctx(tmp: &Path) -> Ctx {
        Ctx {
            home: PathBuf::new(),
            tmp: tmp.into(),
            proc_root: "/proc".into(),
            euid: unsafe { libc::geteuid() },
            // ctime cannot be backdated, so age fixtures by moving the clock
            now: SystemTime::now() + Duration::from_secs(86400 * 10),
        }
    }

    fn screen(app: &mut App) -> String {
        let mut t = Terminal::new(TestBackend::new(110, 10)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn space_selects_only_safe_rows() {
        let (c, d) = (ctx(Path::new("/nonexistent")), Path::new("/nonexistent"));
        let mut a = app();
        keys(
            &mut a,
            &[KeyCode::Char(' '), KeyCode::Down, KeyCode::Char(' ')],
            &c,
            d,
        );
        assert_eq!(a.selected, BTreeSet::from([0]));
        assert!(a.message.contains("in use by a process"), "{}", a.message);
        a.handle_key(KeyCode::Char(' '), &c, d); // unselect the skipped row: still nothing
        keys(&mut a, &[KeyCode::Up, KeyCode::Char(' ')], &c, d);
        assert!(a.selected.is_empty());
    }

    #[test]
    fn select_all_safe_and_total() {
        let (c, d) = (ctx(Path::new("/nonexistent")), Path::new("/nonexistent"));
        let mut a = app();
        keys(&mut a, &[KeyCode::Char('a')], &c, d);
        assert_eq!(a.selected, BTreeSet::from([0, 2]));
        assert_eq!(a.selected_bytes(), 4 << 20);
        keys(&mut a, &[KeyCode::Char('n')], &c, d);
        assert!(a.selected.is_empty());
    }

    #[test]
    fn cursor_stays_in_range() {
        let (c, d) = (ctx(Path::new("/nonexistent")), Path::new("/nonexistent"));
        let mut a = app();
        keys(&mut a, &[KeyCode::Up, KeyCode::Char('k')], &c, d);
        assert_eq!(a.state.selected(), Some(0));
        keys(&mut a, &[KeyCode::Down; 9], &c, d);
        assert_eq!(a.state.selected(), Some(2));
        let mut empty = App::new(vec![]);
        keys(
            &mut empty,
            &[KeyCode::Down, KeyCode::Char('j'), KeyCode::Char(' ')],
            &c,
            d,
        );
        assert_eq!(empty.state.selected(), None);
    }

    #[test]
    fn draws_sizes_marks_and_skip_reasons() {
        let (c, d) = (ctx(Path::new("/nonexistent")), Path::new("/nonexistent"));
        let mut a = app();
        keys(&mut a, &[KeyCode::Char(' ')], &c, d);
        let s = screen(&mut a);
        assert!(
            s.contains("[x]") && s.contains("3.0 MiB") && s.contains("/t/big"),
            "{s}"
        );
        assert!(s.contains("skipped: in use by a process"), "{s}");
        assert!(s.contains("selected: 1 item"), "{s}");
    }

    #[test]
    fn quit_keys() {
        let (c, d) = (ctx(Path::new("/nonexistent")), Path::new("/nonexistent"));
        assert_eq!(app().handle_key(KeyCode::Char('q'), &c, d), Action::Quit);
        assert_eq!(app().handle_key(KeyCode::Esc, &c, d), Action::Quit);
        assert_eq!(app().handle_key(KeyCode::Down, &c, d), Action::Continue);
    }

    /// `ctx.tmp/t/f` is one old, unused candidate for the real `tmp` rule.
    fn fixture() -> (tempfile::TempDir, Ctx, Vec<&'static Rule>, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let tmp = t.path().join("tmp");
        fs::create_dir_all(tmp.join("t")).unwrap();
        fs::write(tmp.join("t/f"), vec![1u8; 4096]).unwrap();
        let ctx = ctx(&tmp);
        (t, ctx, vec![tmp_rule()], PathBuf::new())
    }

    #[test]
    fn delete_requires_confirmation_then_removes_and_rescans() {
        let (t, c, rules, _) = fixture();
        let dir = t.path().join("state");
        let mut a = App::new(collect(&rules, &c));
        assert_eq!(a.rows.len(), 1);
        assert_eq!(a.rows[0].status, "ok");
        for k in [KeyCode::Char(' '), KeyCode::Char('d')] {
            a.handle_key(k, &c, &dir);
        }
        assert!(a.confirming);
        assert!(
            c.tmp.join("t").exists(),
            "nothing deleted before confirming"
        );
        a.handle_key(KeyCode::Char('y'), &c, &dir);
        assert!(!c.tmp.join("t").exists());
        assert!(a.message.contains("freed"), "{}", a.message);
        assert!(!a.confirming && a.selected.is_empty());
        // the rescan runs in `run` (interruptible); the app only asks for it
        assert!(a.needs_rescan);
        a.set_rows(collect(&rules, &c));
        assert!(!a.needs_rescan && a.rows.is_empty() && a.state.selected().is_none());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "manifest written");
    }

    #[test]
    fn delete_manifest_name_matches_ctx_now() {
        let (t, mut c, rules, _) = fixture();
        let dir = t.path().join("state");
        let future_now = SystemTime::now() + Duration::from_secs(86400 * 20);
        c.now = future_now;
        let secs = future_now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut a = App::new(collect(&rules, &c));
        for k in [KeyCode::Char(' '), KeyCode::Char('d'), KeyCode::Char('y')] {
            a.handle_key(k, &c, &dir);
        }
        assert!(
            dir.join(format!("{secs}.log")).exists(),
            "manifest named from ctx.now"
        );
    }

    #[test]
    fn any_other_key_cancels_deletion() {
        let (t, c, rules, _) = fixture();
        let dir = t.path().join("state");
        let mut a = App::new(collect(&rules, &c));
        for k in [KeyCode::Char(' '), KeyCode::Char('d'), KeyCode::Char('x')] {
            a.handle_key(k, &c, &dir);
        }
        assert!(!a.confirming);
        assert!(c.tmp.join("t").exists());
        assert!(!dir.exists());
    }

    #[test]
    fn delete_with_nothing_selected_does_not_prompt() {
        let (t, c, rules, _) = fixture();
        let mut a = App::new(collect(&rules, &c));
        a.handle_key(KeyCode::Char('d'), &c, &t.path().join("state"));
        assert!(!a.confirming);
        assert_eq!(a.message, "nothing selected");
    }

    #[test]
    fn draw_cannot_be_spoofed_by_newline_in_name() {
        let mut a = App::new(vec![row(
            "x\n[ ]   9.9 GiB ████████████ tmp  /fake",
            1,
            "ok",
        )]);
        let s = screen(&mut a);
        let lines: Vec<String> = s
            .chars()
            .collect::<Vec<_>>()
            .chunks(110)
            .map(|c| c.iter().collect())
            .collect();
        assert_eq!(
            lines.iter().filter(|l| l.contains("/fake")).count(),
            1,
            "{s}"
        );
        assert!(s.contains("x\\n[ ]"), "{s}");
    }

    #[test]
    fn delete_reports_what_the_recheck_skipped() {
        let (t, c, rules, _) = fixture();
        let dir = t.path().join("state");
        let mut a = App::new(collect(&rules, &c));
        a.handle_key(KeyCode::Char(' '), &c, &dir);
        let _held = fs::File::open(c.tmp.join("t/f")).unwrap(); // becomes busy after the scan
        for k in [KeyCode::Char('d'), KeyCode::Char('y')] {
            a.handle_key(k, &c, &dir);
        }
        assert!(c.tmp.join("t").exists());
        assert!(a.message.contains("1 skipped"), "{}", a.message);
        assert!(a.message.contains("in use by a process"), "{}", a.message);
        assert!(!a.failed);
    }

    #[test]
    fn unwritable_manifest_deletes_nothing_and_counts_as_failure() {
        let (t, c, rules, _) = fixture();
        let dir = t.path().join("state");
        fs::write(&dir, "not a dir").unwrap();
        let mut a = App::new(collect(&rules, &c));
        for k in [KeyCode::Char(' '), KeyCode::Char('d'), KeyCode::Char('y')] {
            a.handle_key(k, &c, &dir);
        }
        assert!(c.tmp.join("t").exists());
        assert!(a.message.contains("nothing deleted"), "{}", a.message);
        assert!(a.failed);
    }

    #[test]
    fn translate_ignores_modified_keys_and_releases() {
        use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyModifiers};
        let k = |code, mods| KeyEvent::new(code, mods);
        assert_eq!(
            translate(k(KeyCode::Char('d'), KeyModifiers::NONE)),
            Some(KeyCode::Char('d'))
        );
        assert_eq!(
            translate(k(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Some(KeyCode::Char('A'))
        );
        assert_eq!(
            translate(k(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(KeyCode::Esc)
        );
        assert_eq!(
            translate(k(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(translate(k(KeyCode::Char('y'), KeyModifiers::ALT)), None);
        let mut release = k(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(translate(release), None);
    }

    #[test]
    fn fit_left_keeps_the_end() {
        assert_eq!(fit_left("/a/b/c", 10), "/a/b/c");
        let f = fit_left("/home/me/.cache/pip/http/blob", 12);
        assert_eq!(f.chars().count(), 12);
        assert!(f.starts_with('…') && f.ends_with("http/blob"));
        assert_eq!(fit_left("abc", 0), "");
        assert_eq!(fit_left("abcdef", 1), "…");
    }

    #[test]
    fn long_paths_are_cut_on_the_left() {
        let long = format!("/very/long/{}/unique-tail", "dir/".repeat(30));
        let mut a = App::new(vec![row(&long, 1, "ok")]);
        let s = screen(&mut a);
        assert!(s.contains("unique-tail") && s.contains('…'), "{s}");
    }

    #[test]
    fn skipped_reason_stays_visible_when_path_is_cut() {
        let long = format!("/very/long/{}/unique-tail", "dir/".repeat(30));
        let mut a = App::new(vec![row(&long, 1, "skipped: in use by a process")]);
        let s = screen(&mut a);
        assert!(
            s.contains("in use by a process") && s.contains("unique-tail"),
            "{s}"
        );
    }

    #[test]
    fn scanning_screen_says_so() {
        let mut t = Terminal::new(TestBackend::new(40, 3)).unwrap();
        t.draw(draw_scanning).unwrap();
        let s: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains("Scanning"), "{s}");
    }

    #[test]
    fn other_users_rows_are_hidden_and_counted() {
        let mut a = App::new(vec![
            row("mine", 5, "ok"),
            row("theirs", 9, "skipped: owned by another user"),
            row("busy", 4, "skipped: in use by a process"),
        ]);
        let names: Vec<_> = a
            .rows
            .iter()
            .map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["mine", "busy"]);
        assert_eq!(a.hidden, 1);
        assert!(screen(&mut a).contains("1 hidden"));
    }
}
