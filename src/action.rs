use std::time::Duration;

use enigo::{Button, Direction, Enigo, Key, Keyboard, Mouse, Settings};

/// Time to wait between consecutive chords. The target window needs a moment
/// to react to the previous chord (for example, opening the system menu after
/// Alt+Space) before the next key is delivered.
const CHORD_GAP: Duration = Duration::from_millis(150);

/// Simulate a key sequence, e.g. ["alt", "space", "n"].
///
/// Leading modifier keys attach to the next regular key and form one chord;
/// a regular key ends the chord. Each chord is pressed and released before the
/// next one starts, with [`CHORD_GAP`] in between, so `["alt", "space", "n"]`
/// means Alt+Space followed by `n` instead of one three-key chord.
///
/// :param keys: Key names as written in the config file.
/// :return: Error message on failure, or `Ok(())` on success.
pub fn press_keys(keys: &[String]) -> Result<(), String> {
    let parsed = keys
        .iter()
        .map(|k| parse_key(k))
        .collect::<Result<Vec<_>, _>>()?;
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("failed to create input simulator: {e}"))?;

    for (index, chord) in chords(&parsed).into_iter().enumerate() {
        if index > 0 {
            std::thread::sleep(CHORD_GAP);
        }
        send_chord(&mut enigo, chord)?;
    }
    Ok(())
}

/// Split a key list into chords, keeping leading modifiers with the regular
/// key that follows them:
///
/// - `[ctrl, shift, t]` -> `[[ctrl, shift, t]]`
/// - `[alt, space, n]`  -> `[[alt, space], [n]]`
/// - `[ctrl, alt]`      -> `[[ctrl, alt]]` (trailing modifiers on their own)
fn chords(keys: &[Key]) -> Vec<&[Key]> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, &key) in keys.iter().enumerate() {
        if !is_modifier(key) {
            result.push(&keys[start..=index]);
            start = index + 1;
        }
    }
    if start < keys.len() {
        result.push(&keys[start..]);
    }
    result
}

/// Whether a key only modifies the key pressed while it is held down.
fn is_modifier(key: Key) -> bool {
    matches!(key, Key::Control | Key::Alt | Key::Shift | Key::Meta)
}

/// Press a chord, then release it in reverse order.
fn send_chord(enigo: &mut Enigo, chord: &[Key]) -> Result<(), String> {
    let mut pressed: Vec<Key> = Vec::new();
    for &key in chord {
        if let Err(e) = enigo.key(key, Direction::Press) {
            // Best-effort release so a partial failure never leaves a
            // modifier stuck down on the system.
            for &held in pressed.iter().rev() {
                let _ = enigo.key(held, Direction::Release);
            }
            return Err(format!("failed to press {key:?}: {e}"));
        }
        pressed.push(key);
    }
    for &key in chord.iter().rev() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_chord_keeps_modifiers_together() {
        let keys = [Key::Control, Key::Shift, Key::Tab];
        assert_eq!(chords(&keys), vec![&keys[..]]);
    }

    #[test]
    fn regular_keys_split_into_a_sequence() {
        let keys = [Key::Alt, Key::Space, Key::Unicode('n')];
        assert_eq!(chords(&keys), vec![&keys[..2], &keys[2..]]);
    }

    #[test]
    fn trailing_modifiers_form_their_own_chord() {
        let keys = [Key::Control, Key::Alt];
        assert_eq!(chords(&keys), vec![&keys[..]]);
    }

    #[test]
    fn empty_list_has_no_chords() {
        let keys: [Key; 0] = [];
        assert!(chords(&keys).is_empty());
    }
}