use std::io::{self, Write};
use std::sync::Arc;

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
        description: "Show this week's summary",
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

fn action_from_key(code: KeyCode, item_count: usize) -> Action {
    match code {
        KeyCode::Up | KeyCode::Char('k') => Action::MoveUp,
        KeyCode::Down | KeyCode::Char('j') => Action::MoveDown,
        KeyCode::Enter => Action::Select,
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let index = c.to_digit(10).unwrap_or(0) as usize;
            if index >= 1 && index <= item_count {
                Action::JumpTo(index - 1)
            } else {
                Action::None
            }
        }
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

pub fn run() -> Result<Option<String>, Box<dyn std::error::Error>> {
    terminal::enable_raw_mode()?;

    let prev_hook = std::panic::take_hook();
    let prev_hook_rc = Arc::new(prev_hook);
    let prev_hook_restore = prev_hook_rc.clone();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(&mut io::stdout());
        (*prev_hook_rc)(info);
    }));

    let result = run_inner();

    let _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| (*prev_hook_restore)(info)));
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Show)?;
    terminal::disable_raw_mode()?;

    result
}

fn run_inner() -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();
    let mut selected: usize = 0;

    execute!(
        stdout,
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0),
        cursor::Hide
    )?;
    draw(&mut stdout, selected)?;

    loop {
        if let Event::Key(key_event) = event::read()? {
            if key_event.kind != KeyEventKind::Press {
                continue;
            }

            let action = action_from_key(key_event.code, MENU_ITEMS.len());

            match action {
                Action::Select => {
                    execute!(
                        stdout,
                        terminal::Clear(ClearType::All),
                        cursor::MoveTo(0, 0)
                    )?;
                    return Ok(Some(MENU_ITEMS[selected].key.to_string()));
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
                    selected = apply_action(action, selected, MENU_ITEMS.len());
                    draw(&mut stdout, selected)?;
                }
            }
        }
    }
}

fn draw(stdout: &mut impl Write, selected: usize) -> io::Result<()> {
    execute!(stdout, cursor::MoveTo(0, 0))?;

    for line in BANNER.lines() {
        write!(stdout, "{line}\r\n")?;
    }

    write!(stdout, "\r\n")?;

    for (i, item) in MENU_ITEMS.iter().enumerate() {
        let number = i + 1;

        if i == selected {
            execute!(stdout, SetForegroundColor(Color::Cyan), SetAttribute(Attribute::Bold))?;
            write!(stdout, "> {number}. {:<12}  {}", item.label, item.description)?;
            execute!(stdout, SetAttribute(Attribute::Reset), ResetColor)?;
        } else {
            write!(stdout, "  {number}. {:<12}  {}", item.label, item.description)?;
        }

        write!(stdout, "\r\n")?;
    }

    write!(stdout, "\r\n")?;
    execute!(stdout, SetAttribute(Attribute::Dim))?;
    write!(
        stdout,
        "\u{2191}\u{2193} Navigate  |  Enter Select  |  1-{} Jump  |  Q Quit",
        MENU_ITEMS.len()
    )?;
    execute!(stdout, SetAttribute(Attribute::Reset))?;
    write!(stdout, "\r\n")?;

    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_from_key_maps_navigation_keys() {
        assert_eq!(action_from_key(KeyCode::Up, 8), Action::MoveUp);
        assert_eq!(action_from_key(KeyCode::Char('k'), 8), Action::MoveUp);
        assert_eq!(action_from_key(KeyCode::Down, 8), Action::MoveDown);
        assert_eq!(action_from_key(KeyCode::Char('j'), 8), Action::MoveDown);
        assert_eq!(action_from_key(KeyCode::Enter, 8), Action::Select);
        assert_eq!(action_from_key(KeyCode::Char('q'), 8), Action::Quit);
        assert_eq!(action_from_key(KeyCode::Esc, 8), Action::Quit);
    }

    #[test]
    fn action_from_key_maps_digit_keys_to_jump() {
        assert_eq!(action_from_key(KeyCode::Char('1'), 8), Action::JumpTo(0));
        assert_eq!(action_from_key(KeyCode::Char('8'), 8), Action::JumpTo(7));
        assert_eq!(action_from_key(KeyCode::Char('0'), 8), Action::None);
        assert_eq!(action_from_key(KeyCode::Char('9'), 8), Action::None);
    }

    #[test]
    fn action_from_key_ignores_unknown_keys() {
        assert_eq!(action_from_key(KeyCode::Char('x'), 8), Action::None);
        assert_eq!(action_from_key(KeyCode::Tab, 8), Action::None);
    }

    #[test]
    fn apply_action_move_up_saturates_at_zero() {
        assert_eq!(apply_action(Action::MoveUp, 0, 8), 0);
        assert_eq!(apply_action(Action::MoveUp, 3, 8), 2);
    }

    #[test]
    fn apply_action_move_down_clamps_at_last_item() {
        assert_eq!(apply_action(Action::MoveDown, 7, 8), 7);
        assert_eq!(apply_action(Action::MoveDown, 5, 8), 6);
    }

    #[test]
    fn apply_action_jump_to_sets_index_directly() {
        assert_eq!(apply_action(Action::JumpTo(4), 0, 8), 4);
    }

    #[test]
    fn menu_items_keys_are_valid_commands() {
        let valid = [
            "standup", "today", "yesterday", "week", "config", "metadata", "init", "uninstall",
        ];
        for item in MENU_ITEMS {
            assert!(
                valid.contains(&item.key),
                "unexpected menu key: {}",
                item.key
            );
        }
    }
}
