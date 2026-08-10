//! Host configuration, persisted as JSON in the platform data directory.
//!
//! On Android this lives in the app's internal storage; on desktop it's
//! `~/.config/tmuxmux-mobile/config.json`. Credentials are stored in
//! plaintext — this is a personal tool, not a secrets manager.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_port() -> u16 {
    22
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Host {
    /// Friendly name shown in the selector. Falls back to `host` if empty.
    #[serde(default)]
    pub label: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    /// Password auth. Used when `private_key` is empty.
    #[serde(default)]
    pub password: String,
    /// OpenSSH/PEM private key text. Takes precedence over password.
    #[serde(default)]
    pub private_key: String,
    /// Passphrase for an encrypted `private_key`.
    #[serde(default)]
    pub key_passphrase: String,
}

impl Default for Host {
    fn default() -> Self {
        Host {
            label: String::new(),
            host: String::new(),
            port: 22,
            username: String::new(),
            password: String::new(),
            private_key: String::new(),
            key_passphrase: String::new(),
        }
    }
}

impl Host {
    pub fn display_name(&self) -> String {
        if self.label.trim().is_empty() {
            if self.username.is_empty() {
                self.host.clone()
            } else {
                format!("{}@{}", self.username, self.host)
            }
        } else {
            self.label.clone()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub hosts: Vec<Host>,
}

impl Config {
    fn path(data_dir: &PathBuf) -> PathBuf {
        data_dir.join("config.json")
    }

    pub fn load(data_dir: &PathBuf) -> Config {
        let p = Self::path(data_dir);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("config parse error ({e}); starting empty");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, data_dir: &PathBuf) {
        let p = Self::path(data_dir);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&p, s) {
                    log::error!("failed to save config to {}: {e}", p.display());
                }
            }
            Err(e) => log::error!("failed to serialize config: {e}"),
        }
    }
}
