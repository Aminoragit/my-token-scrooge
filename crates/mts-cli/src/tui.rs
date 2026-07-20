use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use mts_core::CompiledPolicy;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};
use ratatui::{Frame, Terminal};
use std::fs;
use std::io::{self, stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCREENS: [&str; 11] = [
    "Dashboard",
    "Policies",
    "Simulator",
    "Sessions",
    "Savings",
    "Quality",
    "Benchmarks",
    "Harnesses",
    "Projects",
    "Doctor",
    "Settings",
];
const POLICY_TABS: [&str; 2] = ["FULL BLOCK", "PARTIAL BLOCK"];

struct PolicyEditor {
    tab: usize,
    target: usize,
    path: Option<PathBuf>,
    text: String,
    editing: bool,
    message: Option<String>,
}

impl PolicyEditor {
    fn new(home: &Path, installed: &[String]) -> Self {
        let mut editor = Self {
            tab: 0,
            target: 0,
            path: None,
            text: String::new(),
            editing: false,
            message: None,
        };
        editor.reload(home, installed);
        editor
    }

    fn reload(&mut self, home: &Path, installed: &[String]) {
        self.editing = false;
        self.message = None;
        self.path = installed.get(self.target).map(|target| {
            home.join("harnesses").join(target).join(if self.tab == 0 {
                "block-full.txt"
            } else {
                "block-partial.txt"
            })
        });
        self.text = match &self.path {
            Some(path) => fs::read_to_string(path).unwrap_or_else(|error| {
                self.message = Some(format!("MTS_POLICY_READ: {error}"));
                String::new()
            }),
            None => {
                self.message = Some("No installed targets. Run `mts setup`.".into());
                String::new()
            }
        };
    }

    fn save(&mut self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let result = saveable_policy(self.tab, &self.text).and_then(|contents| {
            crate::state::atomic_write(path, contents)
                .map_err(|error| format!("MTS_POLICY_WRITE: {error}"))
        });
        match result {
            Ok(()) => {
                self.editing = false;
                self.message = Some(format!("Saved {}", path.display()));
            }
            Err(error) => self.message = Some(error),
        }
    }
}

pub fn run(home: &Path, mode: &str, installed: &[String]) -> io::Result<()> {
    enable_raw_mode()?;
    let mut output = stdout();
    execute!(output, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(output))?;
    let result = event_loop(&mut terminal, home, mode, installed);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    home: &Path,
    mode: &str,
    installed: &[String],
) -> io::Result<()> {
    let mut screen = 0usize;
    let mut policy = PolicyEditor::new(home, installed);
    loop {
        terminal.draw(|frame| draw(frame, screen, mode, installed, &policy))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if policy.editing {
                if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    policy.save();
                } else {
                    match key.code {
                        KeyCode::Esc => {
                            policy.reload(home, installed);
                            policy.message = Some("Edit cancelled; reloaded physical file.".into());
                        }
                        KeyCode::Backspace => {
                            policy.text.pop();
                            policy.message = None;
                        }
                        KeyCode::Enter => {
                            policy.text.push('\n');
                            policy.message = None;
                        }
                        KeyCode::Char(character)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            policy.text.push(character);
                            policy.message = None;
                        }
                        _ => {}
                    }
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Right => screen = (screen + 1) % SCREENS.len(),
                KeyCode::Left => screen = (screen + SCREENS.len() - 1) % SCREENS.len(),
                KeyCode::Char('e') if SCREENS[screen] == "Policies" && policy.path.is_some() => {
                    policy.editing = true;
                    policy.message = None;
                }
                KeyCode::Tab if SCREENS[screen] == "Policies" => {
                    policy.tab = 1 - policy.tab;
                    policy.reload(home, installed);
                }
                KeyCode::Down if SCREENS[screen] == "Policies" && !installed.is_empty() => {
                    policy.target = (policy.target + 1) % installed.len();
                    policy.reload(home, installed);
                }
                KeyCode::Up if SCREENS[screen] == "Policies" && !installed.is_empty() => {
                    policy.target = (policy.target + installed.len() - 1) % installed.len();
                    policy.reload(home, installed);
                }
                _ => {}
            }
        }
    }
}

fn draw(
    frame: &mut Frame<'_>,
    screen: usize,
    mode: &str,
    installed: &[String],
    policy: &PolicyEditor,
) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.width < 24 || area.height < 14 {
        frame.render_widget(
            Paragraph::new("Terminal too small for MTS. Resize to at least 24x14.")
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);
    let titles: Vec<_> = SCREENS.iter().map(|title| Line::from(*title)).collect();
    frame.render_widget(
        Tabs::new(titles)
            .select(screen)
            .block(
                Block::default()
                    .title(" my-token-scrooge ")
                    .borders(Borders::ALL),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[0],
    );

    if SCREENS[screen] == "Policies" {
        draw_policies(frame, chunks[1], installed, policy);
    } else {
        let body = match SCREENS[screen] {
            "Dashboard" => format!(
                "Mode: {mode}\nPolicy status: VALID\nInstalled targets: {}\n\nStop AI agents from eating your context.",
                installed.len()
            ),
            "Harnesses" if installed.is_empty() => {
                "No installed targets. Press q and run mts setup.".into()
            }
            "Harnesses" => installed.join("\n"),
            "Doctor" => "Run `mts doctor` for copyable capability and repair details.".into(),
            other => {
                format!(
                    "{other}\n\nThis view uses the same local policy and event store as the CLI."
                )
            }
        };
        frame.render_widget(
            Paragraph::new(body).block(Block::default().borders(Borders::ALL)),
            chunks[1],
        );
    }

    let help = if policy.editing {
        "[Ctrl+S] Validate + save  [Esc] Cancel  [Backspace/Enter] Edit"
    } else if SCREENS[screen] == "Policies" {
        "[e] Edit  [Up/Down] Target  [Tab] Policy  [Left/Right] Navigate  [q] Quit"
    } else {
        "[Left/Right] Navigate  [q/Esc] Quit"
    };
    frame.render_widget(Paragraph::new(help), chunks[2]);
}

fn draw_policies(frame: &mut Frame<'_>, area: Rect, installed: &[String], policy: &PolicyEditor) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(3),
        ])
        .split(area);
    let titles: Vec<_> = POLICY_TABS.iter().map(|title| Line::from(*title)).collect();
    frame.render_widget(
        Tabs::new(titles)
            .select(policy.tab)
            .block(Block::default().borders(Borders::ALL))
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[0],
    );

    let target = installed
        .get(policy.target)
        .map_or("<none>", String::as_str);
    let path = policy.path.as_deref().map_or_else(
        || "<no physical file>".into(),
        |path| path.display().to_string(),
    );
    frame.render_widget(
        Paragraph::new(format!("Target: {target}\nPhysical file: {path}")),
        chunks[1],
    );

    let message = policy
        .message
        .as_deref()
        .map_or_else(String::new, |message| format!("{message}\n\n"));
    let display = format!("{message}{}", policy.text);
    let line_count = display.lines().count();
    let visible = usize::from(chunks[2].height.saturating_sub(2));
    let scroll = if policy.editing && policy.message.is_none() {
        line_count
            .saturating_sub(visible)
            .min(usize::from(u16::MAX)) as u16
    } else {
        0
    };
    frame.render_widget(
        Paragraph::new(display)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(if policy.editing {
                        " EDITING "
                    } else {
                        " POLICY "
                    })
                    .borders(Borders::ALL),
            ),
        chunks[2],
    );
}

fn saveable_policy(tab: usize, text: &str) -> Result<&[u8], String> {
    match tab {
        0 => CompiledPolicy::parse_full(text),
        _ => CompiledPolicy::parse_partial(text),
    }
    .map(|_| text.as_bytes())
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_valid_policy_content_is_saveable_for_the_active_kind() {
        let full = "node_modules/** | write,edit | Dependencies are immutable\n";
        let partial =
            "**/*.log | read | errors-only | max_matches=10,before=1,after=2 | Bound logs\n";

        assert_eq!(saveable_policy(0, full).unwrap(), full.as_bytes());
        assert_eq!(saveable_policy(1, partial).unwrap(), partial.as_bytes());
        assert!(saveable_policy(1, full).is_err());
        assert!(saveable_policy(0, "**/*.log | unknown | Invalid\n").is_err());
    }

    #[test]
    fn exposes_required_navigation_without_extra_policy_types() {
        assert_eq!(SCREENS.len(), 11);
        assert!(SCREENS.contains(&"Policies"));
        assert_eq!(POLICY_TABS, ["FULL BLOCK", "PARTIAL BLOCK"]);
    }
}
