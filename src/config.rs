use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::Deserialize;

/// Shared live configuration. Updated in place when the config file changes,
/// so gesture rules take effect without restarting the program.
static SHARED: OnceLock<RwLock<Config>> = OnceLock::new();

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub app: HashMap<String, HashMap<String, Vec<String>>>,
}

impl Config {
    /// Load config from the given TOML file path.
    ///
    /// :param path: Path to the `gestures.toml` file.
    /// :return: Parsed config, or an error message.
    pub fn load(path: &Path) -> Result<Config, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        toml::from_str(&content).map_err(|e| format!("failed to parse {}: {}", path.display(), e))
    }

    /// Install the given path as the source for the shared live config and
    /// load it once.
    ///
    /// :param path: Path to the `gestures.toml` file.
    /// :return: The freshly loaded config.
    pub fn init_shared(path: &Path) -> Result<Config, String> {
        let config = Config::load(path)?;
        let _ = SHARED.set(RwLock::new(config.clone()));
        Ok(config)
    }

    /// Spawn a background thread that reloads the shared config whenever the
    /// file's modification time changes.
    ///
    /// :param path: Path to the `gestures.toml` file.
    /// :param interval: Polling interval.
    pub fn watch(path: PathBuf, interval: std::time::Duration) {
        std::thread::spawn(move || {
            let mut last_modified = modified_time(&path);
            loop {
                std::thread::sleep(interval);
                let now = modified_time(&path);
                if now.is_some() && now != last_modified {
                    last_modified = now;
                    match Config::load(&path) {
                        Ok(config) => {
                            if let Some(shared) = SHARED.get()
                                && let Ok(mut guard) = shared.write()
                            {
                                *guard = config;
                            }
                            eprintln!("[mouse] config reloaded");
                        }
                        Err(e) => eprintln!("[mouse] config reload failed: {e}"),
                    }
                }
            }
        });
    }

    /// Snapshot the key combination for a gesture, cloned out of the shared
    /// lock so the caller does not hold it while simulating input.
    ///
    /// :param app: Foreground app identifier (None falls back to default).
    /// :param gesture: Gesture direction string, e.g. "R,U".
    /// :return: The key combination, or None when no rule matches.
    pub fn lookup_current(app: Option<&str>, gesture: &str) -> Option<Vec<String>> {
        let guard = SHARED.get()?.read().ok()?;
        guard.lookup(app, gesture).map(|k| k.to_vec())
    }

    /// Look up the key combination for a gesture in the given app context.
    ///
    /// Falls back to `default` when the app has no match.
    ///
    /// :param app: Foreground app identifier (process name on Windows, WM_CLASS on Linux).
    /// :param gesture: Gesture direction string, e.g. "R,U".
    /// :return: The key combination, or None when no rule matches.
    pub fn lookup<'a>(&'a self, app: Option<&str>, gesture: &str) -> Option<&'a [String]> {
        let app_match = app.and_then(|app| {
            self.app
                .get(app)
                .and_then(|rules| rules.get(gesture))
        });
        match app_match {
            Some(keys) => Some(keys),
            None => self.default.get(gesture).map(Vec::as_slice),
        }
    }
}

/// Last modification time of a file, or None when it does not exist.
fn modified_time(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}