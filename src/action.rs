use enigo::{Button, Direction, Enigo, Key, Keyboard, Mouse, Settings};

/// Simulate a key combination, e.g. ["ctrl", "alt", "t"].
///
/// Modifier keys are pressed first, then the final key is clicked and all
/// modifiers are released in reverse order.
///
/// :param keys: Key names as written in the config file.
/// :return: Error message on failure, or None on success.
pub fn press_keys(keys: &[String]) -> Result<(), String> {
    let parsed = keys
        .iter()
        .map(|k| parse_key(k))
        .collect::<Result<Vec<_>, _>>()?;
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("failed to create input simulator: {e}"))?;

    for &key in &parsed {
        enigo
            .key(key, Direction::Press)
            .map_err(|e| format!("failed to press {key:?}: {e}"))?;
    }
    for &key in parsed.iter().rev() {
        enigo
            .key(key, Direction::Release)
            .map_err(|e| format!("failed to release {key:?}: {e}"))?;
    }
    Ok(())
}

/// Send a synthetic right-button click at the current pointer position.
pub fn click_right() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("failed to create input simulator: {e}"))?;
    enigo
        .button(Button::Right, Direction::Click)
        .map_err(|e| format!("failed to click right button: {e}"))
}

/// Map a config key name to an enigo `Key`.
fn parse_key(name: &str) -> Result<Key, String> {
    let lower = name.to_ascii_lowercase();
    let key = match lower.as_str() {
        "ctrl" | "control" => Key::Control,
        "alt" => Key::Alt,
        "shift" => Key::Shift,
        "meta" | "win" | "super" => Key::Meta,
        "tab" => Key::Tab,
        "enter" | "return" => Key::Return,
        "esc" | "escape" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "up" | "arrowup" => Key::UpArrow,
        "down" | "arrowdown" => Key::DownArrow,
        "left" | "arrowleft" => Key::LeftArrow,
        "right" | "arrowright" => Key::RightArrow,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        s if s.chars().count() == 1 => {
            let c = s.chars().next().expect("single char");
            Key::Unicode(c)
        }
        _ => return Err(format!("unknown key: {name}")),
    };
    Ok(key)
}