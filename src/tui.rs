use crate::render::human;
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

#[derive(Debug, PartialEq)]
pub enum Action {
    Continue,
    Quit,
}

pub struct App {
    pub rows: Vec<Row>,
    pub state: ListState,
    /// Indexes into `rows`; only rows whose status is `ok` can be in here.
    pub selected: BTreeSet<usize>,
    pub confirming: bool,
    pub message: String,
}

impl App {
    pub fn new(rows: Vec<Row>) -> App {
        let mut state = ListState::default();
        state.select((!rows.is_empty()).then_some(0));
        App {
            rows,
            state,
            selected: BTreeSet::new(),
            confirming: false,
            message: String::new(),
        }
    }

    pub fn selected_bytes(&self) -> u64 {
        self.selected.iter().map(|&i| self.rows[i].bytes).sum()
    }

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
    /// then rescans so the list shows what is really left.
    fn delete(&mut self, ctx: &Ctx, rules: &[&'static Rule], manifest_dir: &Path) {
        let chosen: Vec<Row> = self
            .selected
            .iter()
            .map(|&i| self.rows[i].clone())
            .collect();
        self.message = match clean::clean(&chosen, ctx, true, manifest_dir) {
            Ok(o) => format!(
                "freed {} ({} removed, {} failed)",
                human(o.freed),
                o.removed,
                o.failed
            ),
            Err(e) => format!(
                "nothing deleted: cannot write manifest in {}: {e}",
                manifest_dir.display()
            ),
        };
        self.rows = collect(rules, ctx);
        self.selected.clear();
        let keep = self
            .state
            .selected()
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.state.select((!self.rows.is_empty()).then_some(keep));
    }

    /// Applies one key press. Deleting re-runs the full guard check through `clean::clean`
    /// and then rescans with `rules`.
    pub fn handle_key(
        &mut self,
        code: KeyCode,
        ctx: &Ctx,
        rules: &[&'static Rule],
        manifest_dir: &Path,
    ) -> Action {
        if self.confirming {
            self.confirming = false;
            if code == KeyCode::Char('y') {
                self.delete(ctx, rules, manifest_dir);
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

const BAR: usize = 12;

fn bar(bytes: u64, max: u64) -> String {
    let filled = if max == 0 {
        0
    } else {
        ((bytes as f64 / max as f64) * BAR as f64).round() as usize
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR - filled))
}

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
            let text = format!(
                "{mark} {:>10} {} {:<9} {}{tail}",
                human(r.bytes),
                bar(r.bytes, max),
                r.category,
                r.path.display()
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
        format!(
            "selected: {n} item{}, {}   {}",
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

/// Runs the interactive explorer until the user quits.
pub fn run(rules: &[&'static Rule], ctx: &Ctx, manifest_dir: &Path) -> io::Result<()> {
    let mut app = App::new(collect(rules, ctx));
    let mut terminal = ratatui::init();
    let result = (|| loop {
        terminal.draw(|f| draw(f, &mut app))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && app.handle_key(key.code, ctx, rules, manifest_dir) == Action::Quit
        {
            return Ok(());
        }
    })();
    ratatui::restore();
    result
}

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
            app.handle_key(k, ctx, &[], dir);
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
        a.handle_key(KeyCode::Char(' '), &c, &[], d); // unselect the skipped row: still nothing
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
        assert_eq!(
            app().handle_key(KeyCode::Char('q'), &c, &[], d),
            Action::Quit
        );
        assert_eq!(app().handle_key(KeyCode::Esc, &c, &[], d), Action::Quit);
        assert_eq!(
            app().handle_key(KeyCode::Down, &c, &[], d),
            Action::Continue
        );
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
            a.handle_key(k, &c, &rules, &dir);
        }
        assert!(a.confirming);
        assert!(
            c.tmp.join("t").exists(),
            "nothing deleted before confirming"
        );
        a.handle_key(KeyCode::Char('y'), &c, &rules, &dir);
        assert!(!c.tmp.join("t").exists());
        assert!(a.message.contains("freed"), "{}", a.message);
        assert!(!a.confirming && a.selected.is_empty() && a.rows.is_empty());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "manifest written");
    }

    #[test]
    fn any_other_key_cancels_deletion() {
        let (t, c, rules, _) = fixture();
        let dir = t.path().join("state");
        let mut a = App::new(collect(&rules, &c));
        for k in [KeyCode::Char(' '), KeyCode::Char('d'), KeyCode::Char('x')] {
            a.handle_key(k, &c, &rules, &dir);
        }
        assert!(!a.confirming);
        assert!(c.tmp.join("t").exists());
        assert!(!dir.exists());
    }

    #[test]
    fn delete_with_nothing_selected_does_not_prompt() {
        let (t, c, rules, _) = fixture();
        let mut a = App::new(collect(&rules, &c));
        a.handle_key(KeyCode::Char('d'), &c, &rules, &t.path().join("state"));
        assert!(!a.confirming);
        assert_eq!(a.message, "nothing selected");
    }
}
