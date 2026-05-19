//! Persisted settings under ~/Library/Application Support/claude-o-meter/.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub refresh_secs: u64,
    pub idle_refresh_secs: u64,
    pub notify_session: bool,
    pub notify_weekly: bool,
    pub thresholds: Vec<f64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_secs: 7 * 60,
            idle_refresh_secs: 20 * 60,
            notify_session: true,
            notify_weekly: true,
            thresholds: vec![0.75, 0.90, 0.95],
        }
    }
}

fn settings_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("com", "cynkra", "claude-o-meter")
        .context("could not resolve project directories")?;
    Ok(dirs.config_dir().join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        let Ok(path) = settings_path() else {
            return Settings::default();
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "settings.json unreadable; using defaults");
                return Settings::default();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                let backup = path.with_extension("json.bak");
                tracing::warn!(
                    path = %path.display(),
                    backup = %backup.display(),
                    error = %e,
                    "settings.json malformed; backing up and using defaults",
                );
                let _ = std::fs::rename(&path, &backup);
                Settings::default()
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let p = settings_path()?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("encode settings")?;
        std::fs::write(&p, bytes).with_context(|| format!("write {p:?}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert!(s.refresh_secs >= 60);
        assert!(!s.thresholds.is_empty());
    }

    #[test]
    fn round_trip_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s.refresh_secs, back.refresh_secs);
        assert_eq!(s.thresholds, back.thresholds);
    }
}
