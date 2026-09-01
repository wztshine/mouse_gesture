use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::gesture::Outcome;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub mod x11_overlay;
#[cfg(target_os = "windows")]
pub mod win_overlay;
#[cfg(target_os = "windows")]
pub mod windows;

/// Platform-specific behavior: global input capture and foreground window info.
pub trait Platform {
    /// Identifier of the currently focused application
    /// (process name on Windows, WM_CLASS on Linux).
    fn foreground_app(&mut self) -> Option<String>;

    /// Re-send a synthetic right-button click so the native context menu still
    /// works after the platform swallowed the original press.
    fn replay_right_click(&mut self) -> Result<(), String>;

    /// Run the gesture capture loop. Blocks until an error occurs.
    ///
    /// :param config: Loaded gesture configuration.
    /// :return: Error message when the capture loop exits.
    fn run(&mut self, config: &Config) -> Result<(), String>;
}

/// Dispatch a recognized gesture outcome using the shared config.
pub fn dispatch(config: &Config, platform: &mut dyn Platform, outcome: Outcome) -> Result<(), String> {
    match outcome {
        Outcome::Click => platform.replay_right_click(),
        Outcome::Gesture(gesture) => {
            let app = platform.foreground_app();
            if let Some(keys) = config.lookup(app.as_deref(), &gesture) {
                eprintln!("[mouse] app={app:?} gesture={gesture} -> {keys:?}");
                crate::action::press_keys(keys)
            } else {
                eprintln!("[mouse] app={app:?} gesture={gesture} (no rule)");
                Ok(())
            }
        }
    }
}

/// Locate the config file.
///
/// Looks for `gestures.toml` next to the executable first, then in the
/// current working directory.
pub fn config_path() -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    for dir in exe_dir.into_iter().chain([PathBuf::from(".")]) {
        let candidate = dir.join("gestures.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("gestures.toml not found next to the executable or in the current directory".into())
}