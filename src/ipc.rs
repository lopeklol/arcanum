use serde::Deserialize;

use crate::config::{DohServer, SessionMode};

/// Komendy z paska UI. Nieznane -> błąd deserializacji -> odrzucone.
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    Ready,
    Navigate { url: String },
    Back,
    Forward,
    Reload,
    NewTab,
    CloseTab { id: u32 },
    SwitchTab { id: u32 },
    CycleTab { #[serde(default)] back: bool },
    OpenSettings,
    OpenHistory,
    /// Odpowiedź na pytanie o przywrócenie sesji (tryb "ask").
    RestoreSession { yes: bool },
    /// Podpowiedzi z historii dla tekstu w pasku adresu.
    Suggest { q: String },
    /// Pasek powiększony na całe okno (menu/podpowiedzi otwarte) lub z powrotem 80 px.
    Overlay { open: bool },
    WinMinimize,
    WinMaximize,
    WinClose,
    WinDrag,
    /// Kierunek: n, s, e, w, ne, nw, se, sw.
    WinResize { dir: String },
}

/// Komendy ze strony ustawień (osobny kanał, osobny WebView).
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum SettingsCmd {
    GetSettings,
    /// Zmiana DNS -> zapis + restart.
    SaveSettings { servers: Vec<DohServer>, fallback_system: bool },
    /// Preferencje historii/sesji -> zapis bez restartu.
    SavePrefs { session_mode: SessionMode, save_history: bool, clear_history_on_exit: bool },
    ResetSettings,
}

/// Komendy ze strony historii (osobny kanał, osobny WebView).
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum HistoryCmd {
    Search { q: String },
    Open { url: String },
    Delete { url: String },
    Clear,
}

pub fn parse_history(msg: &str) -> Option<HistoryCmd> {
    serde_json::from_str(msg).ok()
}

pub fn parse(msg: &str) -> Option<Cmd> {
    serde_json::from_str(msg).ok()
}

pub fn parse_settings(msg: &str) -> Option<SettingsCmd> {
    serde_json::from_str(msg).ok()
}

/// Tekst z paska adresu -> URL (lub wyszukiwanie).
pub fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return "about:blank".into();
    }
    let lower = s.to_ascii_lowercase();
    if ["http://", "https://", "about:", "file://"].iter().any(|p| lower.starts_with(p)) {
        return s.into();
    }
    let looks_like_host = !s.contains(' ')
        && (s.contains('.') || s.starts_with("localhost"))
        && !s.starts_with('.');
    if looks_like_host {
        format!("https://{s}")
    } else {
        format!("https://duckduckgo.com/?q={}", encode(s))
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("http://a.b/c"), "http://a.b/c");
        assert_eq!(normalize_url("rust lang"), "https://duckduckgo.com/?q=rust+lang");
    }

    #[test]
    fn cmds() {
        assert!(matches!(parse(r#"{"cmd":"close_tab","id":3}"#), Some(Cmd::CloseTab { id: 3 })));
        assert!(parse(r#"{"cmd":"rm_rf"}"#).is_none());
        // Komendy ustawień nie przechodzą przez kanał paska i odwrotnie.
        assert!(parse(r#"{"cmd":"reset_settings"}"#).is_none());
        assert!(parse_settings(r#"{"cmd":"new_tab"}"#).is_none());
        assert!(matches!(
            parse_settings(r#"{"cmd":"save_settings","servers":[{"host":"1.1.1.1","port":443,"path":"/dns-query"}],"fallback_system":false}"#),
            Some(SettingsCmd::SaveSettings { .. })
        ));
    }
}
