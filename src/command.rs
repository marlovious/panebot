use crossterm::event::{KeyCode, KeyEvent};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    #[default]
    Normal,
    Command,
    Passthrough,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    TogglePlay,
    ToggleMute,
    CycleAudio,
    CycleSubtitle,
    Keypress(String),
    ToggleFullscreen,
    Seek(f64),
    AdjustVolume(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Run(Action),
    EnterPassthrough,
    ExitPassthrough,
    ExitCommand,
}

pub fn interpret(mode: Mode, key: KeyEvent) -> Option<Input> {
    match mode {
        Mode::Normal => None,
        Mode::Command => match key.code {
            KeyCode::Char(' ') => Some(Input::Run(Action::TogglePlay)),
            KeyCode::Char('m') => Some(Input::Run(Action::ToggleMute)),
            KeyCode::Char('a') => Some(Input::Run(Action::CycleAudio)),
            KeyCode::Char('s') => Some(Input::Run(Action::CycleSubtitle)),
            KeyCode::Enter => Some(Input::Run(Action::Keypress("ENTER".into()))),
            KeyCode::Char('f') => Some(Input::Run(Action::ToggleFullscreen)),
            KeyCode::Left | KeyCode::Char('h') => Some(Input::Run(Action::Seek(-5.0))),
            KeyCode::Right | KeyCode::Char('l') => Some(Input::Run(Action::Seek(5.0))),
            KeyCode::Up | KeyCode::Char('k') => Some(Input::Run(Action::Seek(60.0))),
            KeyCode::Down | KeyCode::Char('j') => Some(Input::Run(Action::Seek(-60.0))),
            KeyCode::Char('9') => Some(Input::Run(Action::AdjustVolume(-5.0))),
            KeyCode::Char('0') => Some(Input::Run(Action::AdjustVolume(5.0))),
            KeyCode::Char('\\') => Some(Input::EnterPassthrough),
            KeyCode::Tab => Some(Input::ExitCommand),
            _ => None,
        },
        Mode::Passthrough => {
            if key.code == KeyCode::Char('\\') {
                Some(Input::ExitPassthrough)
            } else {
                mpv_key_name(key.code).map(|key| Input::Run(Action::Keypress(key)))
            }
        }
    }
}

fn mpv_key_name(code: KeyCode) -> Option<String> {
    let name = match code {
        KeyCode::Char(' ') => "SPACE",
        KeyCode::Enter => "ENTER",
        KeyCode::Esc => "ESC",
        KeyCode::Backspace => "BS",
        KeyCode::Delete => "DEL",
        KeyCode::Tab => "TAB",
        KeyCode::Up => "UP",
        KeyCode::Down => "DOWN",
        KeyCode::Left => "LEFT",
        KeyCode::Right => "RIGHT",
        KeyCode::PageUp => "PGUP",
        KeyCode::PageDown => "PGDWN",
        KeyCode::Home => "HOME",
        KeyCode::End => "END",
        KeyCode::F(number @ 1..=10) => return Some(format!("F{number}")),
        KeyCode::Char(character)
            if character.is_ascii_lowercase() || character.is_ascii_digit() =>
        {
            return Some(character.to_string());
        }
        _ => return None,
    };
    Some(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn passthrough_forwards_v_and_reserves_backslash() {
        let plain_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
        assert_eq!(
            interpret(Mode::Passthrough, plain_v),
            Some(Input::Run(Action::Keypress("v".into())))
        );
        let exit = KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::NONE);
        assert_eq!(
            interpret(Mode::Passthrough, exit),
            Some(Input::ExitPassthrough)
        );

        let f10 = KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE);
        assert_eq!(
            interpret(Mode::Passthrough, f10),
            Some(Input::Run(Action::Keypress("F10".into())))
        );
        let uppercase = KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT);
        assert_eq!(interpret(Mode::Passthrough, uppercase), None);
        let punctuation = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(interpret(Mode::Passthrough, punctuation), None);
    }

    #[test]
    fn production_seek_directions_are_preserved() {
        for (character, action) in [('a', Action::CycleAudio), ('s', Action::CycleSubtitle)] {
            let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE);
            assert_eq!(interpret(Mode::Command, key), Some(Input::Run(action)));
            assert_eq!(interpret(Mode::Normal, key), None);
            assert_eq!(
                interpret(Mode::Passthrough, key),
                Some(Input::Run(Action::Keypress(character.to_string())))
            );
        }
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            interpret(Mode::Command, down),
            Some(Input::Run(Action::Seek(-60.0)))
        );
        assert_eq!(
            interpret(Mode::Command, up),
            Some(Input::Run(Action::Seek(60.0)))
        );
    }
}
