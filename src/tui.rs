use crate::ports::{PortRow, PortSnapshot, collect_snapshot};
use crate::state::StateManager;
use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{Frame, Terminal};
use std::fs;
use std::io::{self, Stdout, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn run_watch(interval_ms: u64, include_udp: bool, filter: Option<String>) -> Result<()> {
    let state_manager = StateManager::new().context("Failed to initialize state manager")?;
    let tick_rate = Duration::from_millis(interval_ms.max(250));
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal, state_manager, tick_rate, include_udp, filter);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;
    Terminal::new(CrosstermBackend::new(stdout)).context("Failed to initialize terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Port,
    Pid,
    Command,
    Runa,
    Conflict,
}

impl SortBy {
    fn next(self) -> Self {
        match self {
            Self::Port => Self::Pid,
            Self::Pid => Self::Command,
            Self::Command => Self::Runa,
            Self::Runa => Self::Conflict,
            Self::Conflict => Self::Port,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailMode {
    Details,
    Logs,
}

#[derive(Debug, Clone)]
enum Action {
    StopRuna(String),
    RestartRuna(String),
    KillPid(i32),
}

impl Action {
    fn label(&self) -> String {
        match self {
            Self::StopRuna(name) => format!("stop Runa process '{name}'"),
            Self::RestartRuna(name) => format!("restart Runa process '{name}'"),
            Self::KillPid(pid) => format!("kill PID {pid}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum MouseTarget {
    Stop,
    Restart,
    Kill,
    Logs,
    Copy,
}

struct App {
    include_udp: bool,
    filter: String,
    editing_filter: bool,
    selected: usize,
    snapshot: PortSnapshot,
    last_refresh: Instant,
    tick_rate: Duration,
    sort_by: SortBy,
    detail_mode: DetailMode,
    pending_action: Option<Action>,
    message: String,
    table_area: Rect,
    mouse_targets: Vec<(Rect, MouseTarget)>,
}

impl App {
    fn new(
        state_manager: &StateManager,
        tick_rate: Duration,
        include_udp: bool,
        filter: Option<String>,
    ) -> Self {
        Self {
            include_udp,
            filter: filter.unwrap_or_default(),
            editing_filter: false,
            selected: 0,
            snapshot: collect_snapshot(state_manager, include_udp),
            last_refresh: Instant::now(),
            tick_rate,
            sort_by: SortBy::Port,
            detail_mode: DetailMode::Details,
            pending_action: None,
            message: "ready".to_string(),
            table_area: Rect::default(),
            mouse_targets: Vec::new(),
        }
    }

    fn refresh(&mut self, state_manager: &StateManager) {
        self.snapshot = collect_snapshot(state_manager, self.include_udp);
        self.last_refresh = Instant::now();
        self.clamp_selection();
    }

    fn visible_rows(&self) -> Vec<&PortRow> {
        let filter = self.filter.trim().to_lowercase();
        let mut rows: Vec<&PortRow> = self
            .snapshot
            .rows
            .iter()
            .filter(|row| {
                if filter.is_empty() {
                    return true;
                }
                let runa = row.runa.as_ref().map(|r| r.name.as_str()).unwrap_or("");
                let haystack = format!(
                    "{} {} {} {} {} {} {}",
                    row.entry.port,
                    row.entry.protocol.as_str(),
                    row.entry.address,
                    row.entry.pid,
                    row.entry.command,
                    row.entry.user,
                    runa
                )
                .to_lowercase();
                haystack.contains(&filter)
            })
            .collect();

        rows.sort_by(|a, b| match self.sort_by {
            SortBy::Port => a
                .entry
                .port
                .cmp(&b.entry.port)
                .then_with(|| a.entry.pid.cmp(&b.entry.pid)),
            SortBy::Pid => a.entry.pid.cmp(&b.entry.pid),
            SortBy::Command => a.entry.command.cmp(&b.entry.command),
            SortBy::Runa => runa_name(a).cmp(runa_name(b)),
            SortBy::Conflict => b
                .conflict_count
                .cmp(&a.conflict_count)
                .then_with(|| a.entry.port.cmp(&b.entry.port)),
        });
        rows
    }

    fn selected_row(&self) -> Option<PortRow> {
        let rows = self.visible_rows();
        rows.get(self.selected).map(|row| (*row).clone())
    }

    fn clamp_selection(&mut self) {
        let len = self.visible_rows().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }
}

fn runa_name(row: &PortRow) -> &str {
    row.runa.as_ref().map(|r| r.name.as_str()).unwrap_or("")
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state_manager: StateManager,
    tick_rate: Duration,
    include_udp: bool,
    filter: Option<String>,
) -> Result<()> {
    let mut app = App::new(&state_manager, tick_rate, include_udp, filter);

    loop {
        terminal
            .draw(|frame| draw(frame, &mut app, &state_manager))
            .context("Failed to draw TUI")?;

        let timeout = app
            .tick_rate
            .saturating_sub(app.last_refresh.elapsed())
            .min(Duration::from_millis(200));

        if event::poll(timeout).context("Failed to poll terminal events")? {
            match event::read().context("Failed to read terminal event")? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press
                        && handle_key(&mut app, &state_manager, key.code)? =>
                {
                    return Ok(());
                }
                Event::Mouse(mouse) => handle_mouse(&mut app, &state_manager, mouse)?,
                _ => {}
            }
        }

        if app.last_refresh.elapsed() >= app.tick_rate {
            app.refresh(&state_manager);
        }
    }
}

fn handle_key(app: &mut App, state_manager: &StateManager, code: KeyCode) -> Result<bool> {
    if let Some(action) = app.pending_action.clone() {
        match code {
            KeyCode::Char('y') | KeyCode::Enter => {
                execute_action(app, state_manager, action)?;
                app.pending_action = None;
                app.refresh(state_manager);
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                app.pending_action = None;
                app.message = "action cancelled".to_string();
            }
            _ => {}
        }
        return Ok(false);
    }

    if app.editing_filter {
        match code {
            KeyCode::Esc | KeyCode::Enter => {
                app.editing_filter = false;
                app.clamp_selection();
            }
            KeyCode::Backspace => {
                app.filter.pop();
                app.clamp_selection();
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
                app.clamp_selection();
            }
            _ => {}
        }
        return Ok(false);
    }

    match code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('/') => app.editing_filter = true,
        KeyCode::Char('u') => {
            app.include_udp = !app.include_udp;
            app.refresh(state_manager);
        }
        KeyCode::Char('r') => app.refresh(state_manager),
        KeyCode::Char('t') => {
            app.sort_by = app.sort_by.next();
            app.clamp_selection();
        }
        KeyCode::Char('o') => app.detail_mode = toggle_detail_mode(app.detail_mode),
        KeyCode::Char('c') => copy_selected(app, state_manager),
        KeyCode::Char('s') => queue_stop(app),
        KeyCode::Char('R') => queue_restart(app),
        KeyCode::Char('K') => queue_kill(app),
        KeyCode::Down | KeyCode::Char('j') => {
            let len = app.visible_rows().len();
            if len > 0 {
                app.selected = (app.selected + 1).min(len - 1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.selected = app.selected.saturating_sub(1);
        }
        KeyCode::Home => app.selected = 0,
        KeyCode::End => {
            let len = app.visible_rows().len();
            if len > 0 {
                app.selected = len - 1;
            }
        }
        _ => {}
    }

    Ok(false)
}

fn handle_mouse(app: &mut App, state_manager: &StateManager, mouse: MouseEvent) -> Result<()> {
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            let len = app.visible_rows().len();
            if len > 0 {
                app.selected = (app.selected + 3).min(len - 1);
            }
        }
        MouseEventKind::ScrollUp => {
            app.selected = app.selected.saturating_sub(3);
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        MouseEventKind::Up(MouseButton::Left) => {}
        MouseEventKind::Drag(MouseButton::Left) => {}
        MouseEventKind::Moved => {}
        MouseEventKind::Down(MouseButton::Right) => {}
        MouseEventKind::Up(MouseButton::Right) => {}
        MouseEventKind::Drag(MouseButton::Right) => {}
        MouseEventKind::Down(MouseButton::Middle) => {}
        MouseEventKind::Up(MouseButton::Middle) => {}
        MouseEventKind::Drag(MouseButton::Middle) => {}
        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {}
    }

    if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
        handle_click(app, state_manager, mouse.column, mouse.row)?;
    }
    Ok(())
}

fn handle_click(app: &mut App, state_manager: &StateManager, column: u16, row: u16) -> Result<()> {
    for (area, target) in app.mouse_targets.clone() {
        if rect_contains(area, column, row) {
            match target {
                MouseTarget::Stop => queue_stop(app),
                MouseTarget::Restart => queue_restart(app),
                MouseTarget::Kill => queue_kill(app),
                MouseTarget::Logs => app.detail_mode = toggle_detail_mode(app.detail_mode),
                MouseTarget::Copy => copy_selected(app, state_manager),
            }
            return Ok(());
        }
    }

    if rect_contains(app.table_area, column, row) && row > app.table_area.y + 1 {
        let index = row.saturating_sub(app.table_area.y + 2) as usize;
        let len = app.visible_rows().len();
        if index < len {
            app.selected = index;
        }
    }
    Ok(())
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn toggle_detail_mode(mode: DetailMode) -> DetailMode {
    match mode {
        DetailMode::Details => DetailMode::Logs,
        DetailMode::Logs => DetailMode::Details,
    }
}

fn queue_stop(app: &mut App) {
    let Some(row) = app.selected_row() else {
        return;
    };
    if let Some(runa) = row.runa {
        app.pending_action = Some(Action::StopRuna(runa.name));
    } else if can_signal_pid(row.entry.pid) {
        app.pending_action = Some(Action::KillPid(row.entry.pid));
    } else {
        app.message = "refusing to stop protected PID".to_string();
    }
}

fn queue_restart(app: &mut App) {
    let Some(row) = app.selected_row() else {
        return;
    };
    if let Some(runa) = row.runa {
        app.pending_action = Some(Action::RestartRuna(runa.name));
    } else {
        app.message = "selected row is not Runa-managed".to_string();
    }
}

fn queue_kill(app: &mut App) {
    let Some(row) = app.selected_row() else {
        return;
    };
    if !can_signal_pid(row.entry.pid) {
        app.message = "refusing to kill protected PID".to_string();
        return;
    }
    app.pending_action = Some(Action::KillPid(row.entry.pid));
}

fn can_signal_pid(pid: i32) -> bool {
    pid > 1 && pid != std::process::id() as i32
}

fn execute_action(app: &mut App, state_manager: &StateManager, action: Action) -> Result<()> {
    match action {
        Action::StopRuna(name) => {
            let meta = state_manager
                .get_process(&name)
                .context("Failed to read Runa process state")?;
            signal::kill(Pid::from_raw(meta.pid), Signal::SIGTERM)
                .context("Failed to stop Runa process")?;
            app.message = format!("sent SIGTERM to {name}");
        }
        Action::RestartRuna(name) => {
            let meta = state_manager
                .get_process(&name)
                .context("Failed to read Runa process state")?;
            signal::kill(Pid::from_raw(meta.pid), Signal::SIGHUP)
                .context("Failed to restart Runa process")?;
            app.message = format!("sent SIGHUP to {name}");
        }
        Action::KillPid(pid) => {
            signal::kill(Pid::from_raw(pid), Signal::SIGTERM).context("Failed to kill PID")?;
            app.message = format!("sent SIGTERM to PID {pid}");
        }
    }
    Ok(())
}

fn copy_selected(app: &mut App, state_manager: &StateManager) {
    let Some(row) = app.selected_row() else {
        return;
    };
    let text = selected_summary(&row, state_manager);
    match copy_to_clipboard(&text) {
        Ok(()) => app.message = "copied selected port".to_string(),
        Err(err) => app.message = format!("copy failed: {err}"),
    }
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let program = "pbcopy";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xclip";

    let mut child = Command::new(program)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{program} exited with {status}")
    }
}

fn selected_summary(row: &PortRow, state_manager: &StateManager) -> String {
    let runa = row.runa.as_ref().map(|r| r.name.as_str()).unwrap_or("-");
    let log_dir = state_manager.get_run_dir();
    format!(
        "{} {}:{} pid={} command={} runa={} stdout={} stderr={}",
        row.entry.protocol.as_str(),
        row.entry.address,
        row.entry.port,
        row.entry.pid,
        row.entry.command,
        runa,
        log_dir.join(format!("{runa}.out.log")).display(),
        log_dir.join(format!("{runa}.err.log")).display()
    )
}

fn draw(frame: &mut Frame, app: &mut App, state_manager: &StateManager) {
    app.mouse_targets.clear();
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(5),
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(chunks[1]);
    app.table_area = body[0];
    draw_table(frame, app, body[0]);
    draw_side_panel(frame, app, state_manager, body[1]);
    draw_command_bar(frame, app, chunks[2]);

    if app.editing_filter {
        draw_filter_popup(frame, app, area);
    }
    if let Some(action) = &app.pending_action {
        draw_confirm_popup(frame, action, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let age = app
        .snapshot
        .refreshed_at
        .elapsed()
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    let visible = app.visible_rows().len();
    let action_chip = Span::styled(
        " ACTIONS ",
        Style::default().bg(Color::Yellow).fg(Color::Black),
    );
    let line = Line::from(vec![
        Span::styled(
            " Runa Ports ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        action_chip,
        Span::raw(format!(
            " total:{} visible:{} tcp:{} udp:{} runa:{} conflicts:{} sort:{:?} refreshed:{}s ",
            app.snapshot.rows.len(),
            visible,
            app.snapshot.tcp_count(),
            app.snapshot.udp_count(),
            app.snapshot.runa_count(),
            app.snapshot.conflict_count(),
            app.sort_by,
            age
        )),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible_rows = app.visible_rows();
    let rows = visible_rows.iter().map(|row| {
        let conflict = if row.conflict_count > 1 {
            format!("x{}", row.conflict_count)
        } else {
            "-".to_string()
        };
        let runa = row
            .runa
            .as_ref()
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "-".to_string());
        let chips = row_chips(row);
        let style = if row.conflict_count > 1 {
            Style::default().fg(Color::Red)
        } else if row.runa.is_some() {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::White)
        };
        Row::new(vec![
            Cell::from(row.entry.port.to_string()),
            Cell::from(row.entry.protocol.as_str()),
            Cell::from(row.entry.address.clone()),
            Cell::from(row.entry.pid.to_string()),
            Cell::from(row.entry.command.clone()),
            Cell::from(runa),
            Cell::from(chips),
            Cell::from(conflict),
        ])
        .style(style)
    });

    let header = Row::new(vec![
        "PORT", "PROTO", "ADDRESS", "PID", "COMMAND", "RUNA", "STATUS", "CONFLICT",
    ])
    .style(
        Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
    );
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(17),
            Constraint::Length(8),
            Constraint::Percentage(24),
            Constraint::Percentage(18),
            Constraint::Length(18),
            Constraint::Length(9),
        ],
    )
    .header(header)
    .block(Block::default().title("Ports").borders(Borders::ALL))
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">");

    let mut state = TableState::default();
    if !visible_rows.is_empty() {
        state.select(Some(app.selected.min(visible_rows.len() - 1)));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn row_chips(row: &PortRow) -> String {
    let mut chips = Vec::new();
    if row.runa.is_some() {
        chips.push("RUNA");
    } else {
        chips.push("SYSTEM");
    }
    if row.conflict_count > 1 {
        chips.push("CONFLICT");
    }
    if row.entry.protocol.as_str() == "UDP" {
        chips.push("UDP");
    }
    chips.join(" ")
}

fn draw_side_panel(frame: &mut Frame, app: &mut App, state_manager: &StateManager, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(8)])
        .split(area);

    let selected = app.selected_row();
    let action_line = Line::from(vec![
        action_span(
            " Stop ",
            selected.as_ref().and_then(|r| r.runa.as_ref()).is_some(),
        ),
        Span::raw(" "),
        action_span(
            " Restart ",
            selected.as_ref().and_then(|r| r.runa.as_ref()).is_some(),
        ),
        Span::raw(" "),
        action_span(" Logs ", true),
        Span::raw(" "),
        action_span(" Copy ", true),
        Span::raw(" "),
        action_span(" Kill ", selected.is_some()),
    ]);
    frame.render_widget(
        Paragraph::new(action_line).block(Block::default().title("Actions").borders(Borders::ALL)),
        chunks[0],
    );

    let button_y = chunks[0].y + 1;
    let mut x = chunks[0].x + 1;
    for (label, target) in [
        (" Stop ", MouseTarget::Stop),
        (" Restart ", MouseTarget::Restart),
        (" Logs ", MouseTarget::Logs),
        (" Copy ", MouseTarget::Copy),
        (" Kill ", MouseTarget::Kill),
    ] {
        app.mouse_targets.push((
            Rect {
                x,
                y: button_y,
                width: label.len() as u16,
                height: 1,
            },
            target,
        ));
        x += label.len() as u16 + 1;
    }

    draw_info_panel(frame, app, state_manager, chunks[1]);
}

fn action_span(label: &'static str, enabled: bool) -> Span<'static> {
    if enabled {
        Span::styled(label, Style::default().bg(Color::Cyan).fg(Color::Black))
    } else {
        Span::styled(label, Style::default().fg(Color::DarkGray))
    }
}

fn draw_info_panel(frame: &mut Frame, app: &App, state_manager: &StateManager, area: Rect) {
    let selected = app.selected_row();
    let mut items = Vec::new();

    if let Some(row) = selected {
        match app.detail_mode {
            DetailMode::Details => details_items(&mut items, &row, state_manager),
            DetailMode::Logs => log_items(&mut items, &row, state_manager),
        }
    } else {
        items.push(ListItem::new("No matching ports."));
    }

    for error in &app.snapshot.errors {
        items.push(ListItem::new(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Red),
        ))));
    }

    let title = match app.detail_mode {
        DetailMode::Details => "Details",
        DetailMode::Logs => "Logs",
    };
    frame.render_widget(
        List::new(items).block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

fn details_items(items: &mut Vec<ListItem>, row: &PortRow, state_manager: &StateManager) {
    items.push(ListItem::new(format!(
        "{} {}:{}",
        row.entry.protocol.as_str(),
        row.entry.address,
        row.entry.port
    )));
    items.push(ListItem::new(format!(
        "pid={} command={} user={}",
        row.entry.pid, row.entry.command, row.entry.user
    )));
    if let Some(runa) = &row.runa {
        items.push(ListItem::new(format!(
            "runa={} status={:?}",
            runa.name, runa.status
        )));
        items.push(ListItem::new(format!(
            "supervisor={} child={}",
            runa.supervisor_pid,
            runa.child_pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".to_string())
        )));
        items.push(ListItem::new(format!(
            "restarts={} last_exit={} last_exit_at={}",
            runa.restart_count,
            runa.last_exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string()),
            runa.last_exit_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        )));
        items.push(ListItem::new(format!("cmd={}", runa.cmd)));
        let dir = state_manager.get_run_dir();
        items.push(ListItem::new(format!(
            "stdout={}",
            dir.join(format!("{}.out.log", runa.name)).display()
        )));
        items.push(ListItem::new(format!(
            "stderr={}",
            dir.join(format!("{}.err.log", runa.name)).display()
        )));
    } else {
        items.push(ListItem::new("runa=-"));
    }
}

fn log_items(items: &mut Vec<ListItem>, row: &PortRow, state_manager: &StateManager) {
    let Some(runa) = &row.runa else {
        items.push(ListItem::new("No logs for non-Runa process."));
        return;
    };
    let dir = state_manager.get_run_dir();
    for label in ["out", "err"] {
        let path = dir.join(format!("{}.{}.log", runa.name, label));
        items.push(ListItem::new(format!("--- {} ---", path.display())));
        match fs::read_to_string(&path) {
            Ok(content) => {
                for line in content
                    .lines()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    items.push(ListItem::new(line.to_string()));
                }
            }
            Err(_) => items.push(ListItem::new("no log file")),
        }
    }
}

fn draw_command_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let udp = if app.include_udp { "on" } else { "off" };
    let filter = if app.filter.is_empty() {
        "-".to_string()
    } else {
        app.filter.clone()
    };
    let text = format!(
        "q quit  / filter  u udp:{}  t sort  s stop/kill  R restart Runa  o logs  c copy  K kill  filter:{}  | {}",
        udp, filter, app.message
    );
    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_filter_popup(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(60, 3, area);
    frame.render_widget(Clear, popup);
    let input = Paragraph::new(app.filter.as_str())
        .block(Block::default().title("Filter").borders(Borders::ALL))
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(input, popup);
}

fn draw_confirm_popup(frame: &mut Frame, action: &Action, area: Rect) {
    let popup = centered_rect(70, 5, area);
    frame.render_widget(Clear, popup);
    let text = format!(
        "Confirm: {}\n\nEnter/y confirm, Esc/n cancel",
        action.label()
    );
    let input = Paragraph::new(text)
        .block(
            Block::default()
                .title("Confirm Action")
                .borders(Borders::ALL),
        )
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: true });
    frame.render_widget(input, popup);
}

fn centered_rect(width_percent: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min((area.height.saturating_sub(height)) / 2),
        ])
        .split(area);
    let width = area.width.saturating_mul(width_percent).saturating_div(100);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min((area.width.saturating_sub(width)) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}
