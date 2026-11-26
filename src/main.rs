#![windows_subsystem = "windows"]

mod browser;
mod config;
mod history;
mod ipc;

use std::time::{Duration, Instant};

use browser::{Browser, Kind, UserEvent};
use config::{Config, SessionMode, APP_NAME};
use history::{History, Session};
use ipc::{Cmd, HistoryCmd, SettingsCmd};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::windows::WindowBuilderExtWindows,
    window::{ResizeDirection, Window, WindowBuilder},
};
use wry::WebContext;

/// Czeka, aż proces o danym PID się zakończy (max `timeout`).
fn wait_for_pid(pid: u32, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        match out {
            Ok(o) if String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")) => {
                std::thread::sleep(Duration::from_millis(200));
            }
            _ => break,
        }
    }
    std::thread::sleep(Duration::from_millis(800));
}

fn spawn_restart() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .args(["--wait-pid", &std::process::id().to_string()])
            .spawn();
    }
}

fn parse_wait_pid() -> Option<u32> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--wait-pid")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn send_history(b: &Browser, h: &History, q: &str) {
    let list = serde_json::to_string(&h.search(q, 500)).unwrap_or_else(|_| "[]".into());
    b.eval_history(&format!("window.onHistory({list})"));
}

/// Stan aplikacji poza kartami.
struct App {
    cfg: Config,
    hist: History,
    hist_q: String,
    /// Sesja czekająca na decyzję (tryb "ask").
    pending: Option<Session>,
}

impl App {
    fn save_session(&self, b: &Browser) {
        if self.cfg.session_mode == SessionMode::None {
            return;
        }
        // Dopóki użytkownik nie zdecydował, nie nadpisuj starej sesji.
        if self.pending.is_some() {
            return;
        }
        let (urls, active) = b.session();
        Session { urls, active }.save();
    }

    /// Zapis przy wyjściu (sesja + historia / czyszczenie).
    fn on_exit(&mut self, b: &Browser, keep_session: bool) {
        if keep_session {
            self.save_session(b);
        } else if self.pending.is_none() {
            Session::default().save();
        }
        if self.cfg.clear_history_on_exit {
            self.hist.clear();
        }
        self.hist.save();
    }
}

fn restore(b: &mut Browser, window: &Window, s: &Session) -> wry::Result<()> {
    let mut ids = Vec::new();
    for u in &s.urls {
        ids.push(b.new_tab_bg(window, u, false)?);
    }
    if let Some(id) = ids.get(s.active).or(ids.first()).copied() {
        b.switch_to(id);
    }
    Ok(())
}

fn main() -> wry::Result<()> {
    if let Some(pid) = parse_wait_pid() {
        wait_for_pid(pid, Duration::from_secs(10));
    }

    let cfg = Config::load();
    if let Err(e) = config::apply_doh_to_profile(&cfg) {
        eprintln!("Nie udało się zapisać DoH do profilu: {e}");
    }
    let mut app = App { hist: History::load(), hist_q: String::new(), pending: None, cfg };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title(APP_NAME)
        .with_decorations(false)
        .with_undecorated_shadow(true)
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)
        .expect("okno");

    let ctx = WebContext::new(Some(config::data_dir()));
    let mut b = Browser::new(&window, proxy, ctx)?;

    let saved = Session::load();
    match (app.cfg.session_mode, saved) {
        (SessionMode::Restore, Some(s)) => restore(&mut b, &window, &s)?,
        (SessionMode::Ask, Some(s)) => {
            b.new_tab(&window, &app.cfg.start_url)?;
            app.pending = Some(s);
        }
        _ => b.new_tab(&window, &app.cfg.start_url)?,
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                app.on_exit(&b, true);
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. },
                ..
            } => {
                b.resize(&window);
                b.eval_bar(&format!("window.onMaximized && window.onMaximized({})", window.is_maximized()));
            }
            Event::UserEvent(UserEvent::Title(id, t)) => {
                if app.cfg.save_history {
                    if let Some(tab) = b.tabs.iter().find(|x| x.id == id) {
                        app.hist.set_title(&tab.url, &t);
                    }
                }
                b.set_title(id, t);
            }
            Event::UserEvent(UserEvent::Url(id, u)) => {
                let changed = b.tabs.iter().find(|x| x.id == id).is_some_and(|t| t.url != u);
                b.set_url(id, u.clone());
                if changed {
                    if app.cfg.save_history {
                        app.hist.visit(&u);
                        if let Some(t) = b.tabs.iter().find(|x| x.id == id) {
                            if !t.title.is_empty() {
                                let title = t.title.clone();
                                app.hist.set_title(&u, &title);
                            }
                        }
                        app.hist.save();
                    }
                    app.save_session(&b);
                }
            }
            Event::UserEvent(UserEvent::NewWindow(u)) => {
                let _ = b.new_tab(&window, &u);
                app.save_session(&b);
            }
            Event::UserEvent(UserEvent::Ipc(msg)) => {
                let Some(cmd) = ipc::parse(&msg) else { return };
                match cmd {
                    Cmd::Ready => {
                        b.push_state();
                        if app.pending.is_some() {
                            b.eval_bar("window.askSession()");
                        }
                    }
                    Cmd::RestoreSession { yes } => {
                        if let Some(s) = app.pending.take() {
                            if yes {
                                // Zastąp pustą kartę startową przywróconymi.
                                let start: Vec<u32> = b.tabs.iter().map(|t| t.id).collect();
                                let _ = restore(&mut b, &window, &s);
                                for id in start {
                                    b.close(id);
                                }
                            }
                            app.save_session(&b);
                        }
                    }
                    Cmd::Navigate { url } => {
                        b.set_overlay(&window, false);
                        let url = ipc::normalize_url(&url);
                        match b.active_tab() {
                            Some(t) if t.is_internal() => {
                                let _ = b.new_tab(&window, &url);
                            }
                            Some(t) => {
                                let _ = t.webview.load_url(&url);
                                let _ = t.webview.focus();
                            }
                            None => {}
                        }
                    }
                    Cmd::Back | Cmd::Forward | Cmd::Reload => {
                        if let Some(t) = b.active_tab().filter(|t| !t.is_internal()) {
                            let js = match cmd {
                                Cmd::Back => "history.back()",
                                Cmd::Forward => "history.forward()",
                                _ => "location.reload()",
                            };
                            let _ = t.webview.evaluate_script(js);
                        }
                    }
                    Cmd::NewTab => {
                        let _ = b.new_tab(&window, "about:blank");
                        b.focus_url();
                    }
                    Cmd::CloseTab { id } => {
                        if !b.close(id) {
                            app.on_exit(&b, false);
                            *control_flow = ControlFlow::Exit;
                        } else {
                            app.save_session(&b);
                        }
                    }
                    Cmd::SwitchTab { id } => {
                        b.switch_to(id);
                        app.save_session(&b);
                    }
                    Cmd::CycleTab { back } => b.cycle(back),
                    Cmd::OpenSettings => {
                        b.set_overlay(&window, false);
                        let _ = b.open_internal(&window, Kind::Settings);
                    }
                    Cmd::OpenHistory => {
                        b.set_overlay(&window, false);
                        let _ = b.open_internal(&window, Kind::History);
                    }
                    Cmd::Suggest { q } => {
                        let items = serde_json::to_string(&app.hist.suggest(&q, 8)).unwrap_or_else(|_| "[]".into());
                        b.eval_bar(&format!("window.onSuggest({}, {items})", js_str(&q)));
                    }
                    Cmd::Overlay { open } => b.set_overlay(&window, open),
                    Cmd::WinMinimize => window.set_minimized(true),
                    Cmd::WinMaximize => window.set_maximized(!window.is_maximized()),
                    Cmd::WinClose => {
                        app.on_exit(&b, true);
                        *control_flow = ControlFlow::Exit;
                    }
                    Cmd::WinDrag => {
                        let _ = window.drag_window();
                    }
                    Cmd::WinResize { dir } => {
                        let d = match dir.as_str() {
                            "n" => ResizeDirection::North,
                            "s" => ResizeDirection::South,
                            "e" => ResizeDirection::East,
                            "w" => ResizeDirection::West,
                            "ne" => ResizeDirection::NorthEast,
                            "nw" => ResizeDirection::NorthWest,
                            "se" => ResizeDirection::SouthEast,
                            "sw" => ResizeDirection::SouthWest,
                            _ => return,
                        };
                        if !window.is_maximized() {
                            let _ = window.drag_resize_window(d);
                        }
                    }
                }
            }
            Event::UserEvent(UserEvent::HistoryIpc(msg)) => {
                let Some(cmd) = ipc::parse_history(&msg) else { return };
                match cmd {
                    HistoryCmd::Search { q } => {
                        app.hist_q = q;
                        send_history(&b, &app.hist, &app.hist_q);
                    }
                    HistoryCmd::Open { url } => {
                        if history::is_recordable(&url) {
                            let _ = b.new_tab(&window, &url);
                        }
                    }
                    HistoryCmd::Delete { url } => {
                        app.hist.delete(&url);
                        app.hist.save();
                        send_history(&b, &app.hist, &app.hist_q);
                    }
                    HistoryCmd::Clear => {
                        app.hist.clear();
                        app.hist.save();
                        send_history(&b, &app.hist, &app.hist_q);
                    }
                }
            }
            Event::UserEvent(UserEvent::SettingsIpc(msg)) => {
                let Some(cmd) = ipc::parse_settings(&msg) else { return };
                let send_cfg = |b: &Browser, c: &Config| {
                    let j = serde_json::to_string(c).unwrap_or_default();
                    b.eval_settings(&format!("window.onSettings({j})"));
                };
                match cmd {
                    SettingsCmd::GetSettings => send_cfg(&b, &app.cfg),
                    SettingsCmd::ResetSettings => {
                        let d = Config {
                            servers: Config::default().servers,
                            fallback_system: false,
                            ..app.cfg.clone()
                        };
                        send_cfg(&b, &d);
                    }
                    SettingsCmd::SavePrefs { session_mode, save_history, clear_history_on_exit } => {
                        app.cfg.session_mode = session_mode;
                        app.cfg.save_history = save_history;
                        app.cfg.clear_history_on_exit = clear_history_on_exit;
                        match app.cfg.save() {
                            Ok(()) => {
                                if session_mode == SessionMode::None {
                                    Session::default().save();
                                } else {
                                    app.save_session(&b);
                                }
                                b.eval_settings("window.onPrefsSaved()");
                            }
                            Err(e) => b.eval_settings(&format!(
                                "window.onSettingsError({})",
                                js_str(&format!("Zapis nieudany: {e}"))
                            )),
                        }
                    }
                    SettingsCmd::SaveSettings { servers, fallback_system } => {
                        let new_cfg = Config { servers, fallback_system, ..app.cfg.clone() };
                        let res = new_cfg.validate().and_then(|_| {
                            new_cfg.save().map_err(|e| format!("Zapis nieudany: {e}"))
                        });
                        match res {
                            Ok(()) => {
                                app.cfg = new_cfg;
                                app.on_exit(&b, true);
                                spawn_restart();
                                *control_flow = ControlFlow::Exit;
                            }
                            Err(e) => b.eval_settings(&format!("window.onSettingsError({})", js_str(&e))),
                        }
                    }
                }
            }
            _ => {}
        }
    });
}
