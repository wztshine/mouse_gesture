use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

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