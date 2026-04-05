use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use crate::activity_report::{self, ActivityReport, PERIOD_OPTIONS};
use crate::db::Database;
use crate::{RANGE_DATE_FORMATS, parse_supported_date, range_date_format_error};
use chrono::{Local, NaiveDate};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, ClearType},
};

const BANNER: &str = r#"
     _ _     _     _
  __| (_) __| | __| | ___
 / _` | |/ _` |/ _` |/ _ \
| (_| | | (_| | (_| | (_) |  https://github.com/drugoi/diddo-hooks
 \__,_|_|\__,_|\__,_|\___/   Let your commits write your standup.
"#;

struct MenuItem {
    key: &'static str,
    label: &'static str,
    description: &'static str,
}

const MENU_ITEMS: &[MenuItem] = &[
    MenuItem {
        key: "standup",
        label: "Standup",
        description: "Last 24 hours summary",
    },
    MenuItem {
        key: "today",
        label: "Today",
        description: "Show today's summary",
    },
    MenuItem {
        key: "yesterday",
        label: "Yesterday",
        description: "Show yesterday's summary",
    },
    MenuItem {
        key: "week",
        label: "Week",
        description: "Show summary for the last 7 days",
    },
    MenuItem {
        key: "month",
        label: "Month",
        description: "Show summary for the last 30 days",
    },
    MenuItem {
        key: "range",
        label: "Range",
        description: "Choose a custom date range",
    },
    MenuItem {
        key: "activity",
        label: "Activity Report",
        description: "Heatmap and statistics",
    },
    MenuItem {
        key: "config",
        label: "Config",
        description: "Show config and paths",
    },
    MenuItem {
        key: "metadata",
        label: "Metadata",
        description: "Show database metadata",
    },
    MenuItem {
        key: "init",
        label: "Init",
        description: "Install post-commit hook",
    },
    MenuItem {
        key: "uninstall",
        label: "Uninstall",
        description: "Remove hook and clean up",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    MoveUp,
    MoveDown,
    Select,
    Quit,
    JumpTo(usize),
    None,
}

#[derive(Debug, Clone, PartialEq)]
enum UiState {
    Menu {
        selected: usize,
    },
    RangeForm(RangeFormState),
    ActivityPeriodSelect {
        selected: usize,
    },
    ActivityReportView {
        report: ActivityReport,
        report_text: String,
        scroll: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RangeField {
    #[default]
    From,
    To,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RangeFormState {
    from: String,
    to: String,
    focus: RangeField,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RangeFormEvent {
    Stay,
    Submit(String),
    Cancel,
}

fn action_from_key(code: KeyCode, item_count: usize) -> Action {
    match code {
        KeyCode::Up | KeyCode::Char('k') => Action::MoveUp,
        KeyCode::Down | KeyCode::Char('j') => Action::MoveDown,
        KeyCode::Enter => Action::Select,
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Char('0') if item_count >= 10 => Action::JumpTo(9),
        KeyCode::Char(c) if c.is_ascii_digit() => match c.to_digit(10).unwrap_or(0) as usize {
            index if index >= 1 && index <= item_count => Action::JumpTo(index - 1),
            _ => Action::None,
        },
        _ => Action::None,
    }
}

fn apply_action(action: Action, selected: usize, item_count: usize) -> usize {
    match action {
        Action::MoveUp => selected.saturating_sub(1),
        Action::MoveDown => (selected + 1).min(item_count - 1),
        Action::JumpTo(index) => index,
        Action::Select | Action::Quit | Action::None => selected,
    }
}

fn restore_terminal(stdout: &mut impl Write) {
    let _ = execute!(
        stdout,
        cursor::Show,
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0)
    );
    let _ = terminal::disable_raw_mode();
}

pub fn run(db_path: Option<&Path>) -> Result<Option<String>, Box<dyn std::error::Error>> {
    terminal::enable_raw_mode()?;

    let prev_hook = std::panic::take_hook();
    let prev_hook_rc = Arc::new(prev_hook);
    let prev_hook_restore = prev_hook_rc.clone();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(&mut io::stdout());
        (*prev_hook_rc)(info);
    }));

    let result = run_inner(db_path);

    let _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| (*prev_hook_restore)(info)));
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Show)?;
    terminal::disable_raw_mode()?;

    result
}

fn run_inner(db_path: Option<&Path>) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();
    let mut state = UiState::Menu { selected: 0 };

    execute!(
        stdout,
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0),
        cursor::Hide
    )?;
    draw(&mut stdout, &state)?;

    loop {
        if let Event::Key(key_event) = event::read()? {
            if key_event.kind != KeyEventKind::Press {
                continue;
            }

            match &mut state {
                UiState::Menu { selected } => {
                    let action = action_from_key(key_event.code, MENU_ITEMS.len());

                    match action {
                        Action::Select => {
                            if let Some(command) = menu_selection_command(*selected) {
                                execute!(
                                    stdout,
                                    terminal::Clear(ClearType::All),
                                    cursor::MoveTo(0, 0)
                                )?;
                                return Ok(Some(command));
                            }
                            state = transition_from_menu(*selected);
                            draw(&mut stdout, &state)?;
                        }
                        Action::Quit => {
                            execute!(
                                stdout,
                                terminal::Clear(ClearType::All),
                                cursor::MoveTo(0, 0)
                            )?;
                            return Ok(None);
                        }
                        _ => {
                            *selected = apply_action(action, *selected, MENU_ITEMS.len());
                            draw(&mut stdout, &state)?;
                        }
                    }
                }
                UiState::RangeForm(form) => match handle_range_form_key(form, key_event.code) {
                    RangeFormEvent::Stay => draw(&mut stdout, &state)?,
                    RangeFormEvent::Submit(command) => {
                        execute!(
                            stdout,
                            terminal::Clear(ClearType::All),
                            cursor::MoveTo(0, 0)
                        )?;
                        return Ok(Some(command));
                    }
                    RangeFormEvent::Cancel => {
                        state = handle_range_form_escape();
                        draw(&mut stdout, &state)?;
                    }
                },
                UiState::ActivityPeriodSelect { selected } => {
                    let action = action_from_key(key_event.code, PERIOD_OPTIONS.len());
                    match action {
                        Action::Select => {
                            let (months, _) = PERIOD_OPTIONS[*selected];
                            match build_activity_report(db_path, months) {
                                Ok(report) => {
                                    let report_text = activity_report::render_terminal(&report);
                                    state = UiState::ActivityReportView {
                                        report,
                                        report_text,
                                        scroll: 0,
                                    };
                                }
                                Err(error) => {
                                    state = UiState::ActivityReportView {
                                        report: empty_report(months),
                                        report_text: format!("Error: {error}"),
                                        scroll: 0,
                                    };
                                }
                            }
                            draw(&mut stdout, &state)?;
                        }
                        Action::Quit => {
                            state = UiState::Menu {
                                selected: activity_menu_index(),
                            };
                            draw(&mut stdout, &state)?;
                        }
                        _ => {
                            *selected = apply_action(action, *selected, PERIOD_OPTIONS.len());
                            draw(&mut stdout, &state)?;
                        }
                    }
                }
                UiState::ActivityReportView {
                    report,
                    report_text,
                    scroll,
                    ..
                } => match key_event.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        state = UiState::Menu {
                            selected: activity_menu_index(),
                        };
                        draw(&mut stdout, &state)?;
                    }
                    KeyCode::Char('e') | KeyCode::Char('E') => {
                        let msg = match activity_report::export_markdown(report) {
                            Ok(path) => format!("Exported to {}", path.display()),
                            Err(error) => format!("Export failed: {error}"),
                        };
                        // Show brief confirmation before returning to menu
                        execute!(
                            stdout,
                            terminal::Clear(ClearType::All),
                            cursor::MoveTo(0, 0)
                        )?;
                        execute!(
                            stdout,
                            SetForegroundColor(Color::Green),
                            SetAttribute(Attribute::Bold)
                        )?;
                        write!(stdout, "{msg}\r\n")?;
                        execute!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
                        stdout.flush()?;
                        // Brief pause so user sees the message
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        state = UiState::Menu {
                            selected: activity_menu_index(),
                        };
                        draw(&mut stdout, &state)?;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        *scroll = scroll.saturating_sub(1);
                        draw(&mut stdout, &state)?;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let (_, term_height) = terminal::size().unwrap_or((80, 24));
                        let visible = (term_height as usize).saturating_sub(3);
                        let max_scroll = report_text.lines().count().saturating_sub(visible);
                        *scroll = (*scroll + 1).min(max_scroll);
                        draw(&mut stdout, &state)?;
                    }
                    _ => {}
                },
            }
        }
    }
}

fn menu_selection_command(selected: usize) -> Option<String> {
    let item = MENU_ITEMS.get(selected)?;
    match item.key {
        "range" | "activity" => None,
        _ => Some(item.key.to_string()),
    }
}

fn transition_from_menu(selected: usize) -> UiState {
    match MENU_ITEMS.get(selected).map(|item| item.key) {
        Some("range") => UiState::RangeForm(RangeFormState::default()),
        Some("activity") => UiState::ActivityPeriodSelect { selected: 0 },
        _ => UiState::Menu { selected },
    }
}

fn handle_range_form_escape() -> UiState {
    UiState::Menu {
        selected: range_menu_index(),
    }
}

fn range_menu_index() -> usize {
    MENU_ITEMS
        .iter()
        .position(|item| item.key == "range")
        .expect("range menu item must exist")
}

fn activity_menu_index() -> usize {
    MENU_ITEMS
        .iter()
        .position(|item| item.key == "activity")
        .expect("activity menu item must exist")
}

fn build_activity_report(
    db_path: Option<&Path>,
    months: u32,
) -> Result<ActivityReport, Box<dyn std::error::Error>> {
    let path = db_path.ok_or("Database path not available")?;
    let database = Database::open(path)?;
    let today = Local::now().date_naive();
    let (from, to) = activity_report::compute_period_range(months, today);
    let commits = database.query_date_range(from, to)?;
    Ok(activity_report::build_report(&commits, from, to, months))
}

fn empty_report(months: u32) -> ActivityReport {
    let today = Local::now().date_naive();
    let (from, to) = activity_report::compute_period_range(months, today);
    activity_report::build_report(&[], from, to, months)
}

impl RangeFormState {
    fn handle_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) if c.is_ascii_digit() || c == '-' || c == '.' => {
                self.active_field_mut().push(c);
                self.error = None;
            }
            KeyCode::Backspace => {
                self.active_field_mut().pop();
                self.error = None;
            }
            KeyCode::Tab | KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                self.focus = match self.focus {
                    RangeField::From => RangeField::To,
                    RangeField::To => RangeField::From,
                };
            }
            _ => {}
        }
    }

    fn active_field_mut(&mut self) -> &mut String {
        match self.focus {
            RangeField::From => &mut self.from,
            RangeField::To => &mut self.to,
        }
    }
}

fn parse_range_form_dates(form: &RangeFormState) -> Result<(NaiveDate, Option<NaiveDate>), String> {
    if form.from.trim().is_empty() {
        return Err("From date is required.".to_string());
    }

    let from = parse_supported_date(form.from.trim()).map_err(|_| range_date_format_error())?;
    let to = if form.to.trim().is_empty() {
        None
    } else {
        Some(parse_supported_date(form.to.trim()).map_err(|_| range_date_format_error())?)
    };

    if let Some(to) = to {
        if from > to {
            return Err("From date must be on or before to date.".to_string());
        }
        Ok((from, Some(to)))
    } else {
        Ok((from, None))
    }
}

fn submit_range_form(form: &mut RangeFormState) -> Option<String> {
    let (from, to) = match parse_range_form_dates(form) {
        Ok(parsed) => parsed,
        Err(message) => {
            form.error = Some(message);
            return None;
        }
    };

    form.error = None;

    match to {
        Some(to) => Some(format!("range --from {from} --to {to}")),
        None => Some(format!("range --from {from}")),
    }
}

fn handle_range_form_key(form: &mut RangeFormState, code: KeyCode) -> RangeFormEvent {
    match code {
        KeyCode::Enter => match submit_range_form(form) {
            Some(command) => RangeFormEvent::Submit(command),
            None => RangeFormEvent::Stay,
        },
        KeyCode::Esc => RangeFormEvent::Cancel,
        _ => {
            form.handle_key(code);
            RangeFormEvent::Stay
        }
    }
}

fn draw(stdout: &mut impl Write, state: &UiState) -> io::Result<()> {
    execute!(
        stdout,
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0)
    )?;

    match state {
        UiState::ActivityReportView {
            report_text,
            scroll,
            ..
        } => {
            let (_, term_height) = terminal::size().unwrap_or((80, 24));
            let visible_lines = (term_height as usize).saturating_sub(3); // reserve for hints
            let lines: Vec<&str> = report_text.lines().collect();
            let max_scroll = lines.len().saturating_sub(visible_lines);
            let scroll = (*scroll).min(max_scroll);

            for line in lines.iter().skip(scroll).take(visible_lines) {
                write!(stdout, "{line}\r\n")?;
            }

            write!(stdout, "\r\n")?;

            execute!(stdout, SetAttribute(Attribute::Dim))?;
            write!(
                stdout,
                "\u{2191}\u{2193} Scroll  |  E Export to markdown  |  Esc Back"
            )?;
            execute!(stdout, SetAttribute(Attribute::Reset))?;
            write!(stdout, "\r\n")?;
        }
        _ => {
            for line in BANNER.lines() {
                write!(stdout, "{line}\r\n")?;
            }
            write!(stdout, "\r\n")?;

            match state {
                UiState::Menu { selected } => {
                    for (i, item) in MENU_ITEMS.iter().enumerate() {
                        let number = i + 1;

                        if i == *selected {
                            execute!(
                                stdout,
                                SetForegroundColor(Color::Cyan),
                                SetAttribute(Attribute::Bold)
                            )?;
                            write!(
                                stdout,
                                "> {number:>2}. {:<16}  {}",
                                item.label, item.description
                            )?;
                            execute!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
                        } else {
                            write!(
                                stdout,
                                "  {number:>2}. {:<16}  {}",
                                item.label, item.description
                            )?;
                        }

                        write!(stdout, "\r\n")?;
                    }

                    write!(stdout, "\r\n")?;
                    execute!(stdout, SetAttribute(Attribute::Dim))?;
                    let jump_hint = if MENU_ITEMS.len() >= 10 {
                        "1-9 / 0 Jump"
                    } else {
                        "1-9 Jump"
                    };
                    write!(
                        stdout,
                        "\u{2191}\u{2193} Navigate  |  Enter Select  |  {jump_hint}  |  Q Quit"
                    )?;
                    execute!(stdout, SetAttribute(Attribute::Reset))?;
                    write!(stdout, "\r\n")?;
                }
                UiState::RangeForm(form) => {
                    write!(stdout, "Range\r\n")?;
                    write!(stdout, "\r\n")?;

                    draw_range_field(stdout, "From", &form.from, form.focus == RangeField::From)?;
                    draw_range_field(stdout, "To", &form.to, form.focus == RangeField::To)?;
                    write!(stdout, "  note: leave To blank to use today\r\n")?;
                    write!(stdout, "\r\n")?;

                    if let Some(error) = form.error.as_deref() {
                        execute!(
                            stdout,
                            SetForegroundColor(Color::Red),
                            SetAttribute(Attribute::Bold)
                        )?;
                        write!(stdout, "{error}\r\n")?;
                        execute!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
                        write!(stdout, "\r\n")?;
                    }

                    execute!(stdout, SetAttribute(Attribute::Dim))?;
                    write!(
                        stdout,
                        "Type {RANGE_DATE_FORMATS}  |  Tab/\u{2191}\u{2193} Switch field  |  Enter Submit  |  Esc Back"
                    )?;
                    execute!(stdout, SetAttribute(Attribute::Reset))?;
                    write!(stdout, "\r\n")?;
                }
                UiState::ActivityPeriodSelect { selected } => {
                    write!(stdout, "Activity Report — Select period\r\n")?;
                    write!(stdout, "\r\n")?;

                    for (i, (_, label)) in PERIOD_OPTIONS.iter().enumerate() {
                        let number = i + 1;
                        if i == *selected {
                            execute!(
                                stdout,
                                SetForegroundColor(Color::Cyan),
                                SetAttribute(Attribute::Bold)
                            )?;
                            write!(stdout, "> {number}. {label}")?;
                            execute!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
                        } else {
                            write!(stdout, "  {number}. {label}")?;
                        }
                        write!(stdout, "\r\n")?;
                    }

                    write!(stdout, "\r\n")?;
                    execute!(stdout, SetAttribute(Attribute::Dim))?;
                    write!(
                        stdout,
                        "\u{2191}\u{2193} Navigate  |  Enter Select  |  Esc Back"
                    )?;
                    execute!(stdout, SetAttribute(Attribute::Reset))?;
                    write!(stdout, "\r\n")?;
                }
                UiState::ActivityReportView { .. } => {
                    unreachable!("handled in outer match")
                }
            }
        }
    }

    stdout.flush()?;
    Ok(())
}

fn draw_range_field(
    stdout: &mut impl Write,
    label: &str,
    value: &str,
    focused: bool,
) -> io::Result<()> {
    if focused {
        execute!(
            stdout,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold)
        )?;
        write!(
            stdout,
            "> {label:<4}: {}",
            if value.is_empty() { "" } else { value }
        )?;
        execute!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
    } else {
        write!(stdout, "  {label:<4}: {}", value)?;
    }

    write!(stdout, "\r\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[test]
    fn action_from_key_maps_navigation_keys() {
        assert_eq!(action_from_key(KeyCode::Up, 10), Action::MoveUp);
        assert_eq!(action_from_key(KeyCode::Char('k'), 10), Action::MoveUp);
        assert_eq!(action_from_key(KeyCode::Down, 10), Action::MoveDown);
        assert_eq!(action_from_key(KeyCode::Char('j'), 10), Action::MoveDown);
        assert_eq!(action_from_key(KeyCode::Enter, 10), Action::Select);
        assert_eq!(action_from_key(KeyCode::Char('q'), 10), Action::Quit);
        assert_eq!(action_from_key(KeyCode::Esc, 10), Action::Quit);
    }

    #[test]
    fn action_from_key_maps_digit_keys_to_jump() {
        assert_eq!(action_from_key(KeyCode::Char('1'), 10), Action::JumpTo(0));
        assert_eq!(action_from_key(KeyCode::Char('9'), 10), Action::JumpTo(8));
        assert_eq!(action_from_key(KeyCode::Char('0'), 10), Action::JumpTo(9));
    }

    #[test]
    fn action_from_key_ignores_unknown_keys() {
        assert_eq!(action_from_key(KeyCode::Char('x'), 10), Action::None);
        assert_eq!(action_from_key(KeyCode::Tab, 10), Action::None);
    }

    #[test]
    fn apply_action_move_up_saturates_at_zero() {
        assert_eq!(apply_action(Action::MoveUp, 0, 10), 0);
        assert_eq!(apply_action(Action::MoveUp, 3, 10), 2);
    }

    #[test]
    fn apply_action_move_down_clamps_at_last_item() {
        assert_eq!(apply_action(Action::MoveDown, 9, 10), 9);
        assert_eq!(apply_action(Action::MoveDown, 5, 10), 6);
    }

    #[test]
    fn apply_action_jump_to_sets_index_directly() {
        assert_eq!(apply_action(Action::JumpTo(4), 0, 10), 4);
    }

    #[test]
    fn menu_items_keys_are_valid_commands() {
        let valid = [
            "standup",
            "today",
            "yesterday",
            "week",
            "month",
            "range",
            "activity",
            "config",
            "metadata",
            "init",
            "uninstall",
        ];
        for item in MENU_ITEMS {
            assert!(
                valid.contains(&item.key),
                "unexpected menu key: {}",
                item.key
            );
        }
    }

    #[test]
    fn menu_items_keep_summary_commands_grouped_at_the_top() {
        let keys = MENU_ITEMS.iter().map(|item| item.key).collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                "standup",
                "today",
                "yesterday",
                "week",
                "month",
                "range",
                "activity",
                "config",
                "metadata",
                "init",
                "uninstall",
            ]
        );
    }

    #[test]
    fn selecting_month_returns_month_command() {
        let month_idx = MENU_ITEMS
            .iter()
            .position(|item| item.key == "month")
            .unwrap();
        assert_eq!(menu_selection_command(month_idx), Some("month".to_string()));
    }

    #[test]
    fn selecting_activity_returns_none_for_inline_handling() {
        let activity_idx = MENU_ITEMS
            .iter()
            .position(|item| item.key == "activity")
            .unwrap();
        assert_eq!(menu_selection_command(activity_idx), None);
    }

    #[test]
    fn selecting_range_opens_form_state() {
        let range_idx = MENU_ITEMS
            .iter()
            .position(|item| item.key == "range")
            .unwrap();
        assert_eq!(
            transition_from_menu(range_idx),
            UiState::RangeForm(RangeFormState::default())
        );
    }

    #[test]
    fn selecting_activity_opens_period_select() {
        let activity_idx = MENU_ITEMS
            .iter()
            .position(|item| item.key == "activity")
            .unwrap();
        assert_eq!(
            transition_from_menu(activity_idx),
            UiState::ActivityPeriodSelect { selected: 0 }
        );
    }

    #[test]
    fn range_form_appends_input_to_from_field() {
        let mut form = RangeFormState::default();

        form.handle_key(KeyCode::Char('2'));
        form.handle_key(KeyCode::Char('0'));
        form.handle_key(KeyCode::Char('2'));
        form.handle_key(KeyCode::Char('6'));

        assert_eq!(form.from, "2026");
        assert_eq!(form.to, "");
        assert_eq!(form.focus, RangeField::From);
    }

    #[test]
    fn range_form_accepts_dotted_date_input() {
        let mut form = RangeFormState::default();

        for c in "01.03.2026".chars() {
            form.handle_key(KeyCode::Char(c));
        }

        assert_eq!(form.from, "01.03.2026");
    }

    #[test]
    fn range_form_tab_moves_focus_to_to_field() {
        let mut form = RangeFormState::default();

        form.handle_key(KeyCode::Tab);
        form.handle_key(KeyCode::Char('2'));

        assert_eq!(form.from, "");
        assert_eq!(form.to, "2");
        assert_eq!(form.focus, RangeField::To);
    }

    #[test]
    fn range_form_submit_without_to_omits_to_flag() {
        let mut form = RangeFormState {
            from: "2026-03-01".to_string(),
            ..RangeFormState::default()
        };

        let command = submit_range_form(&mut form).unwrap();

        assert_eq!(command, "range --from 2026-03-01");
        assert_eq!(form.error, None);
    }

    #[test]
    fn range_form_submit_with_dotted_from_normalizes_to_iso_command() {
        let mut form = RangeFormState {
            from: "01.03.2026".to_string(),
            ..RangeFormState::default()
        };

        let command = submit_range_form(&mut form).unwrap();

        assert_eq!(command, "range --from 2026-03-01");
        assert_eq!(form.error, None);
    }

    #[test]
    fn range_form_submit_with_explicit_to_includes_to_flag() {
        let mut form = RangeFormState {
            from: "2026-03-01".to_string(),
            to: "2026-03-05".to_string(),
            ..RangeFormState::default()
        };

        let command = submit_range_form(&mut form).unwrap();

        assert_eq!(command, "range --from 2026-03-01 --to 2026-03-05");
        assert_eq!(form.error, None);
    }

    #[test]
    fn range_form_submit_with_mixed_formats_includes_iso_to_flag() {
        let mut form = RangeFormState {
            from: "01.03.2026".to_string(),
            to: "2026-03-05".to_string(),
            ..RangeFormState::default()
        };

        let command = submit_range_form(&mut form).unwrap();

        assert_eq!(command, "range --from 2026-03-01 --to 2026-03-05");
        assert_eq!(form.error, None);
    }

    #[test]
    fn range_form_rejects_missing_from() {
        let mut form = RangeFormState::default();

        let command = submit_range_form(&mut form);

        assert_eq!(command, None);
        assert_eq!(form.error.as_deref(), Some("From date is required."));
    }

    #[test]
    fn range_form_rejects_malformed_dates() {
        let mut form = RangeFormState {
            from: "03-01-2026".to_string(),
            ..RangeFormState::default()
        };

        let command = submit_range_form(&mut form);

        assert_eq!(command, None);
        assert_eq!(
            form.error.as_deref(),
            Some("Dates must use YYYY-MM-DD or DD.MM.YYYY format.")
        );
    }

    #[test]
    fn range_form_rejects_from_after_to() {
        let mut form = RangeFormState {
            from: "2026-03-10".to_string(),
            to: "2026-03-01".to_string(),
            ..RangeFormState::default()
        };

        let command = submit_range_form(&mut form);

        assert_eq!(command, None);
        assert_eq!(
            form.error.as_deref(),
            Some("From date must be on or before to date.")
        );
    }

    #[test]
    fn range_form_escape_returns_to_menu_without_command() {
        let state = handle_range_form_escape();

        assert_eq!(
            state,
            UiState::Menu {
                selected: range_menu_index()
            }
        );
    }

    #[test]
    fn range_form_backspace_edits_active_field() {
        let mut form = RangeFormState {
            from: "2026-03-01".to_string(),
            ..RangeFormState::default()
        };

        form.handle_key(KeyCode::Backspace);

        assert_eq!(form.from, "2026-03-0");
    }

    #[test]
    fn range_form_validation_accepts_iso_dates() {
        let form = RangeFormState {
            from: "2026-03-01".to_string(),
            to: "2026-03-11".to_string(),
            ..RangeFormState::default()
        };

        let parsed = parse_range_form_dates(&form).unwrap();

        assert_eq!(parsed.0, NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
        assert_eq!(
            parsed.1,
            Some(NaiveDate::from_ymd_opt(2026, 3, 11).unwrap())
        );
    }

    #[test]
    fn range_form_validation_accepts_dotted_dates() {
        let form = RangeFormState {
            from: "01.03.2026".to_string(),
            to: "11.03.2026".to_string(),
            ..RangeFormState::default()
        };

        let parsed = parse_range_form_dates(&form).unwrap();

        assert_eq!(parsed.0, NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
        assert_eq!(
            parsed.1,
            Some(NaiveDate::from_ymd_opt(2026, 3, 11).unwrap())
        );
    }
}
