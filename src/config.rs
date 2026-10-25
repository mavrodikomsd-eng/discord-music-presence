//! Persistence for the Discord application Client ID.
//!
//! Stored in `%APPDATA%\MusicPresence\config.json`.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub client_id: String,
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("MusicPresence").join("config.json")
}

pub fn load() -> Config {
    let path = config_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Config::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(config: &Config) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("создание папки конфига: {e}"))?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| format!("запись конфига: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let json = serde_json::to_string(&Config {
            client_id: "1234567890".into(),
        })
        .unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.client_id, "1234567890");
    }

    #[test]
    fn empty_file_parses_to_default() {
        let parsed: Result<Config, _> = serde_json::from_str("");
        assert!(parsed.is_err());
        let partial: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(partial.client_id, "");
    }
}
