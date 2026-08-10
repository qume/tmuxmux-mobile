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

    /// Look in `dir` for a dropped-in config and import it, replacing the
    /// current host list. Supports `import.json` (this app's native format)
    /// and `hosts.toml` (desktop tmuxmux format, best-effort conversion).
    /// The source file is deleted after a successful import so it doesn't
    /// clobber in-app edits on the next launch. Returns a status message.
    pub fn import_from(&mut self, dir: &std::path::Path) -> Option<String> {
        let json = dir.join("import.json");
        if let Ok(s) = std::fs::read_to_string(&json) {
            match serde_json::from_str::<Config>(&s) {
                Ok(cfg) => {
                    let n = cfg.hosts.len();
                    self.hosts = cfg.hosts;
                    let _ = std::fs::remove_file(&json);
                    return Some(format!("imported {n} hosts from import.json"));
                }
                Err(e) => return Some(format!("import.json parse error: {e}")),
            }
        }

        let toml_path = dir.join("hosts.toml");
        if let Ok(s) = std::fs::read_to_string(&toml_path) {
            match toml::from_str::<TmuxmuxHostsFile>(&s) {
                Ok(file) => {
                    let mut hosts = Vec::new();
                    let mut skipped = 0;
                    for h in file.hosts {
                        match h.into_host() {
                            Some(host) => hosts.push(host),
                            None => skipped += 1,
                        }
                    }
                    let n = hosts.len();
                    self.hosts = hosts;
                    let _ = std::fs::remove_file(&toml_path);
                    return Some(format!(
                        "imported {n} hosts from hosts.toml ({skipped} skipped: local or cloudflared)"
                    ));
                }
                Err(e) => return Some(format!("hosts.toml parse error: {e}")),
            }
        }
        None
    }
}

/// Subset of the desktop tmuxmux `hosts.toml` schema we can map onto a direct
/// SSH connection.
#[derive(Deserialize)]
struct TmuxmuxHost {
    name: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    local: bool,
}

#[derive(Deserialize)]
struct TmuxmuxHostsFile {
    #[serde(default)]
    hosts: Vec<TmuxmuxHost>,
}

impl TmuxmuxHost {
    /// Convert to a connectable Host, or None if unsupported on mobile
    /// (local shells, or cloudflared-tunnelled ProxyCommand hosts that need
    /// the `ssh`/`cloudflared` binaries we don't have).
    fn into_host(self) -> Option<Host> {
        if self.local {
            return None;
        }
        match self.command {
            None => Some(Host {
                label: self.name.clone(),
                host: self.name,
                port: 22,
                username: self.username.unwrap_or_default(),
                ..Default::default()
            }),
            Some(cmd) => {
                // We cannot run a ProxyCommand (cloudflared) in-process.
                if cmd.contains("ProxyCommand") || cmd.contains("cloudflared") {
                    return None;
                }
                // Best-effort parse of `sshpass -p PW ssh ... user@host`.
                let toks: Vec<&str> = cmd.split_whitespace().collect();
                let mut password = String::new();
                let mut target = None;
                let mut i = 0;
                while i < toks.len() {
                    if toks[i] == "-p" && i + 1 < toks.len() {
                        password = toks[i + 1].to_string();
                        i += 2;
                        continue;
                    }
                    if toks[i].contains('@') && !toks[i].starts_with('-') {
                        target = Some(toks[i].to_string());
                    }
                    i += 1;
                }
                let (user, host) = match target {
                    Some(t) => match t.split_once('@') {
                        Some((u, h)) => (u.to_string(), h.to_string()),
                        None => (self.username.unwrap_or_default(), t),
                    },
                    None => return None,
                };
                Some(Host {
                    label: self.name,
                    host,
                    port: 22,
                    username: user,
                    password,
                    ..Default::default()
                })
            }
        }
    }
}
