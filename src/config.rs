use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "Arcanum";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DohServer {
    /// IP (v4, v6 bez nawiasów) lub nazwa hosta.
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl DohServer {
    pub fn url(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let port = if self.port == 443 { String::new() } else { format!(":{}", self.port) };
        let path = if self.path.starts_with('/') { self.path.clone() } else { format!("/{}", self.path) };
        format!("https://{host}{port}{path}")
    }

    pub fn validate(&self) -> Result<(), String> {
        let h = self.host.trim();
        if h.is_empty() {
            return Err("Adres nie może być pusty".into());
        }
        if !h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c)) {
            return Err(format!("Niedozwolone znaki w adresie: {h}"));
        }
        if self.port == 0 {
            return Err("Port musi być 1–65535".into());
        }
        if !self
            .path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.~{}?=&%".contains(c))
        {
            return Err(format!("Niedozwolone znaki w ścieżce: {}", self.path));
        }
        Ok(())
    }
}

/// Zachowanie sesji przy starcie.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Przywróć karty automatycznie.
    #[default]
    Restore,
    /// Zapytaj przy starcie.
    Ask,
    /// Nie zachowuj sesji.
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Kolejność = priorytet.
    pub servers: Vec<DohServer>,
    /// true -> tryb "automatic" (po awarii listy systemowy DNS), false -> "secure".
    pub fallback_system: bool,
    pub start_url: String,
    pub session_mode: SessionMode,
    pub save_history: bool,
    pub clear_history_on_exit: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            servers: vec![
                DohServer { host: "1.1.1.1".into(), port: 443, path: "/dns-query".into() },
                DohServer { host: "9.9.9.9".into(), port: 443, path: "/dns-query".into() },
            ],
            fallback_system: false,
            start_url: "https://duckduckgo.com".into(),
            session_mode: SessionMode::Restore,
            save_history: true,
            clear_history_on_exit: false,
        }
    }
}

/// Portable: wszystkie dane w `<folder exe>\data`. Nic poza folderem aplikacji.
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data")
}

pub fn data_dir() -> PathBuf {
    app_dir().join("WebView2")
}

fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

impl Config {
    pub fn load() -> Self {
        fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Config>(&s).ok())
            .filter(|c| c.validate().is_ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        fs::create_dir_all(app_dir())?;
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(config_path(), json)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.servers.is_empty() {
            return Err("Dodaj co najmniej jeden serwer".into());
        }
        for s in &self.servers {
            s.validate()?;
        }
        Ok(())
    }

    /// Szablony rozdzielone spacją (format Chromium).
    pub fn templates(&self) -> String {
        self.servers.iter().map(DohServer::url).collect::<Vec<_>>().join(" ")
    }
}

/// Merge `dns_over_https` do `<data_dir>\EBWebView\Local State`.
/// Wywoływać PRZED utworzeniem WebView (profil nie może być otwarty).
pub fn apply_doh_to_profile(cfg: &Config) -> io::Result<()> {
    let dir = data_dir().join("EBWebView");
    fs::create_dir_all(&dir)?;
    let path = dir.join("Local State");

    let mut root: serde_json::Value = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));

    root["dns_over_https"] = serde_json::json!({
        "mode": if cfg.fallback_system { "automatic" } else { "secure" },
        "templates": cfg.templates(),
    });

    let tmp = dir.join("Local State.tmp");
    fs::write(&tmp, serde_json::to_vec(&root).map_err(io::Error::other)?)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(h: &str, p: u16, path: &str) -> DohServer {
        DohServer { host: h.into(), port: p, path: path.into() }
    }

    #[test]
    fn urls() {
        assert_eq!(s("1.1.1.1", 443, "/dns-query").url(), "https://1.1.1.1/dns-query");
        assert_eq!(s("203.0.113.10", 8443, "dns-query").url(), "https://203.0.113.10:8443/dns-query");
        assert_eq!(s("2606:4700::1111", 443, "/dns-query").url(), "https://[2606:4700::1111]/dns-query");
    }

    #[test]
    fn validation() {
        assert!(s("dns.example.com", 443, "/dns-query{?dns}").validate().is_ok());
        assert!(s("", 443, "/x").validate().is_err());
        assert!(s("a b", 443, "/x").validate().is_err());
        assert!(s("x\" --flag", 443, "/x").validate().is_err());
        assert!(s("1.1.1.1", 0, "/x").validate().is_err());
        assert!(s("1.1.1.1", 443, "/x y").validate().is_err());
        assert!(Config { servers: vec![], ..Default::default() }.validate().is_err());
    }

    #[test]
    fn templates_joined() {
        let c = Config::default();
        assert_eq!(c.templates(), "https://1.1.1.1/dns-query https://9.9.9.9/dns-query");
    }
}
