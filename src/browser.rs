use tao::{event_loop::EventLoopProxy, window::Window};
use wry::{
    dpi::{LogicalPosition, LogicalSize},
    http::Request,
    NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder, WebViewExtWindows,
};

/// Wysokość paska UI (karty 36 + pasek 44), w pikselach logicznych.
pub const BAR_H: f64 = 80.0;
/// Tło nowych WebView (kolor motywu) — brak białego mignięcia.
const BG: (u8, u8, u8, u8) = (0x1e, 0x1f, 0x22, 0xff);
pub const SETTINGS_URL: &str = "arcanum://settings";
pub const HISTORY_URL: &str = "arcanum://history";
const UI_HTML: &str = include_str!("../ui/index.html");
const SETTINGS_HTML: &str = include_str!("../ui/settings.html");
const HISTORY_HTML: &str = include_str!("../ui/history.html");

#[derive(Debug)]
pub enum UserEvent {
    Ipc(String),
    SettingsIpc(String),
    HistoryIpc(String),
    Title(u32, String),
    Url(u32, String),
    NewWindow(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Web,
    Settings,
    History,
}

pub struct Tab {
    pub id: u32,
    pub webview: WebView,
    pub title: String,
    pub url: String,
    pub kind: Kind,
}

impl Tab {
    pub fn is_internal(&self) -> bool {
        self.kind != Kind::Web
    }
}

pub struct Browser {
    pub bar: WebView,
    pub tabs: Vec<Tab>,
    pub active: u32,
    next_id: u32,
    proxy: EventLoopProxy<UserEvent>,
    ctx: WebContext,
    overlay: bool,
}

fn size_of(window: &Window) -> LogicalSize<f64> {
    window.inner_size().to_logical(window.scale_factor())
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { position: LogicalPosition::new(x, y).into(), size: LogicalSize::new(w, h.max(0.0)).into() }
}

fn content_rect(window: &Window) -> Rect {
    let s = size_of(window);
    rect(0.0, BAR_H, s.width, s.height - BAR_H)
}

/// Podnosi kontener WebView na wierzch (z-order) wśród rodzeństwa.
fn raise(wv: &WebView) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE},
    };
    unsafe {
        let mut hwnd = HWND::default();
        if wv.controller().ParentWindow(&mut hwnd).is_ok() {
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// Nawigacja dozwolona tylko w obrębie wbudowanej strony (bez http/file).
fn internal_only(u: String) -> bool {
    !(u.starts_with("http") || u.starts_with("file"))
}

impl Browser {
    pub fn new(window: &Window, proxy: EventLoopProxy<UserEvent>, mut ctx: WebContext) -> wry::Result<Self> {
        let p = proxy.clone();
        let s = size_of(window);
        let bar = WebViewBuilder::new_with_web_context(&mut ctx)
            .with_html(UI_HTML)
            .with_transparent(true)
            .with_bounds(rect(0.0, 0.0, s.width, BAR_H))
            .with_navigation_handler(internal_only)
            .with_ipc_handler(move |req: Request<String>| {
                let _ = p.send_event(UserEvent::Ipc(req.into_body()));
            })
            .build_as_child(window)?;
        Ok(Self { bar, tabs: Vec::new(), active: 0, next_id: 1, proxy, ctx, overlay: false })
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Zwykła karta (bez IPC). `activate=false` -> w tle (np. przywracanie sesji).
    pub fn new_tab_bg(&mut self, window: &Window, url: &str, activate: bool) -> wry::Result<u32> {
        let id = self.alloc_id();
        let (p1, p2, p3, p4) = (self.proxy.clone(), self.proxy.clone(), self.proxy.clone(), self.proxy.clone());
        let webview = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_url(url)
            .with_bounds(content_rect(window))
            .with_visible(activate)
            .with_focused(false)
            .with_background_color(BG)
            .with_devtools(true)
            .with_document_title_changed_handler(move |t| {
                let _ = p1.send_event(UserEvent::Title(id, t));
            })
            .with_navigation_handler(move |u| {
                let _ = p2.send_event(UserEvent::Url(id, u));
                true
            })
            .with_on_page_load_handler(move |ev, u| {
                if let PageLoadEvent::Finished = ev {
                    let _ = p3.send_event(UserEvent::Url(id, u));
                }
            })
            .with_new_window_req_handler(move |u, _| {
                let _ = p4.send_event(UserEvent::NewWindow(u));
                NewWindowResponse::Deny
            })
            .build_as_child(window)?;
        self.tabs.push(Tab { id, webview, title: String::new(), url: url.into(), kind: Kind::Web });
        if activate {
            self.switch_to(id);
        }
        Ok(id)
    }

    pub fn new_tab(&mut self, window: &Window, url: &str) -> wry::Result<()> {
        self.new_tab_bg(window, url, true).map(|_| ())
    }

    /// Karta wewnętrzna (ustawienia/historia): jedna instancja, własny kanał IPC.
    pub fn open_internal(&mut self, window: &Window, kind: Kind) -> wry::Result<()> {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == kind).map(|t| t.id) {
            self.switch_to(id);
            return Ok(());
        }
        let (html, title, url) = match kind {
            Kind::Settings => (SETTINGS_HTML, "Ustawienia", SETTINGS_URL),
            Kind::History => (HISTORY_HTML, "Historia", HISTORY_URL),
            Kind::Web => return Ok(()),
        };
        let id = self.alloc_id();
        let p = self.proxy.clone();
        let webview = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_html(html)
            .with_bounds(content_rect(window))
            .with_background_color(BG)
            .with_navigation_handler(internal_only)
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_ipc_handler(move |req: Request<String>| {
                let body = req.into_body();
                let ev = if kind == Kind::Settings { UserEvent::SettingsIpc(body) } else { UserEvent::HistoryIpc(body) };
                let _ = p.send_event(ev);
            })
            .build_as_child(window)?;
        self.tabs.push(Tab { id, webview, title: title.into(), url: url.into(), kind });
        self.switch_to(id);
        Ok(())
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active)
    }

    fn tab_of(&self, kind: Kind) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.kind == kind)
    }

    pub fn switch_to(&mut self, id: u32) {
        if !self.tabs.iter().any(|t| t.id == id) {
            return;
        }
        self.active = id;
        for t in &self.tabs {
            let _ = t.webview.set_visible(t.id == id);
        }
        self.push_state();
    }

    pub fn cycle(&mut self, back: bool) {
        let n = self.tabs.len();
        if n < 2 {
            return;
        }
        let i = self.tabs.iter().position(|t| t.id == self.active).unwrap_or(0);
        let j = if back { (i + n - 1) % n } else { (i + 1) % n };
        self.switch_to(self.tabs[j].id);
    }

    /// Zwraca false, gdy zamknięto ostatnią kartę.
    pub fn close(&mut self, id: u32) -> bool {
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return true };
        if self.tabs.len() == 1 {
            self.tabs.remove(i);
            return false;
        }
        if self.active == id {
            // Najpierw pokaż następną kartę, potem usuń starą (bez mignięcia).
            let j = if i + 1 < self.tabs.len() { i + 1 } else { i - 1 };
            let next = self.tabs[j].id;
            self.active = next;
            let _ = self.tabs[j].webview.set_visible(true);
            self.tabs.remove(i);
            for t in &self.tabs {
                if t.id != next {
                    let _ = t.webview.set_visible(false);
                }
            }
        } else {
            self.tabs.remove(i);
        }
        self.push_state();
        true
    }

    /// Menu/podpowiedzi: pasek na całe okno i na wierzch; po zamknięciu 80 px.
    pub fn set_overlay(&mut self, window: &Window, open: bool) {
        self.overlay = open;
        self.resize(window);
        if open {
            raise(&self.bar);
        }
    }

    pub fn resize(&self, window: &Window) {
        let s = size_of(window);
        let h = if self.overlay { s.height } else { BAR_H };
        let _ = self.bar.set_bounds(rect(0.0, 0.0, s.width, h));
        let r = content_rect(window);
        for t in &self.tabs {
            let _ = t.webview.set_bounds(r);
        }
    }

    pub fn set_title(&mut self, id: u32, title: String) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id && !t.is_internal()) {
            t.title = title;
        }
        self.push_state();
    }

    pub fn set_url(&mut self, id: u32, url: String) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id && !t.is_internal()) {
            t.url = url;
        }
        self.push_state();
    }

    pub fn eval_bar(&self, js: &str) {
        let _ = self.bar.evaluate_script(js);
    }

    /// Fokus systemowy na pasek + fokus pola adresu (zaznaczenie niebieskie, można pisać).
    pub fn focus_url(&self) {
        let _ = self.bar.focus();
        self.eval_bar("window.focusUrl()");
    }

    pub fn eval_settings(&self, js: &str) {
        if let Some(t) = self.tab_of(Kind::Settings) {
            let _ = t.webview.evaluate_script(js);
        }
    }

    pub fn eval_history(&self, js: &str) {
        if let Some(t) = self.tab_of(Kind::History) {
            let _ = t.webview.evaluate_script(js);
        }
    }

    /// Adresy zwykłych kart + indeks aktywnej (do zapisu sesji).
    pub fn session(&self) -> (Vec<String>, usize) {
        let web: Vec<&Tab> = self.tabs.iter().filter(|t| !t.is_internal()).collect();
        let active = web.iter().position(|t| t.id == self.active).unwrap_or(0);
        (web.iter().map(|t| t.url.clone()).collect(), active)
    }

    pub fn push_state(&self) {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|t| serde_json::json!({ "id": t.id, "title": t.title, "url": t.url }))
            .collect();
        let s = serde_json::json!({ "tabs": tabs, "active": self.active });
        self.eval_bar(&format!("window.onState && window.onState({s})"));
    }
}
