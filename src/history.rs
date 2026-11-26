use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::config::app_dir;

const MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    pub title: String,
    /// Sekundy od epoki (ostatnia wizyta).
    pub ts: u64,
    pub visits: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<Entry>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn history_path() -> PathBuf {
    app_dir().join("history.json")
}

fn session_path() -> PathBuf {
    app_dir().join("session.json")
}

/// Adresy, których nie zapisujemy (wewnętrzne/puste).
pub fn is_recordable(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl History {
    pub fn load() -> Self {
        fs::read_to_string(history_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(app_dir());
        if let Ok(j) = serde_json::to_vec(self) {
            let tmp = app_dir().join("history.json.tmp");
            if fs::write(&tmp, j).is_ok() {
                let _ = fs::rename(tmp, history_path());
            }
        }
    }

    /// Wizyta: nowy wpis lub aktualizacja istniejącego (na początek listy).
    pub fn visit(&mut self, url: &str) {
        if !is_recordable(url) {
            return;
        }
        if let Some(i) = self.entries.iter().position(|e| e.url == url) {
            let mut e = self.entries.remove(i);
            e.ts = now();
            e.visits += 1;
            self.entries.insert(0, e);
        } else {
            self.entries.insert(0, Entry { url: url.into(), title: String::new(), ts: now(), visits: 1 });
            self.entries.truncate(MAX_ENTRIES);
        }
    }

    pub fn set_title(&mut self, url: &str, title: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.url == url) {
            e.title = title.into();
        }
    }

    pub fn delete(&mut self, url: &str) {
        self.entries.retain(|e| e.url != url);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Wyszukiwanie (url/tytuł, bez wielkości liter). Puste zapytanie -> najnowsze.
    pub fn search(&self, q: &str, limit: usize) -> Vec<&Entry> {
        let q = q.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| q.is_empty() || e.url.to_lowercase().contains(&q) || e.title.to_lowercase().contains(&q))
            .take(limit)
            .collect()
    }

    /// Podpowiedzi do paska: dopasowania posortowane po liczbie wizyt, potem świeżości.
    pub fn suggest(&self, q: &str, limit: usize) -> Vec<&Entry> {
        let q = q.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let strip = |u: &str| -> String {
            u.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.").to_lowercase()
        };
        let mut m: Vec<(bool, &Entry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let s = strip(&e.url);
                if s.starts_with(&q) {
                    Some((true, e))
                } else if s.contains(&q) || e.title.to_lowercase().contains(&q) {
                    Some((false, e))
                } else {
                    None
                }
            })
            .collect();
        m.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.visits.cmp(&a.1.visits)).then(b.1.ts.cmp(&a.1.ts)));
        m.into_iter().take(limit).map(|(_, e)| e).collect()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Session {
    pub urls: Vec<String>,
    pub active: usize,
}

impl Session {
    pub fn load() -> Option<Self> {
        fs::read_to_string(session_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Session>(&s).ok())
            .filter(|s| !s.urls.is_empty())
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(app_dir());
        if let Ok(j) = serde_json::to_vec(self) {
            let _ = fs::write(session_path(), j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visit_and_suggest() {
        let mut h = History::default();
        h.visit("https://example.com/");
        h.visit("https://www.rust-lang.org/");
        h.visit("https://example.com/");
        h.visit("about:blank");
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[0].visits, 2);
        assert_eq!(h.suggest("rust", 5)[0].url, "https://www.rust-lang.org/");
        assert_eq!(h.suggest("ex", 5)[0].url, "https://example.com/");
        assert!(h.suggest("", 5).is_empty());
        h.delete("https://example.com/");
        assert_eq!(h.entries.len(), 1);
    }
}
