# CLAUDE.md

## Przegląd projektu
Własna przeglądarka desktopowa na system **Windows** napisana w języku **Rust**, oparta o silnik Chromium poprzez **WebView2** (`tao` + `wry`), z własnym interfejsem użytkownika (UI) oraz obsługą niestandardowego serwera DNS (DoH).

## Kluczowe założenia architektoniczne
1. **Brak lokalnych serwerów w tle:** Aplikacja **nie uruchamia** żadnego lokalnego serwera proxy (SOCKS5/HTTP) ani lokalnego demona DNS.
2. **Konfiguracja DNS przy starcie WebView2:**
   - Własny serwer DoH (np. `https://203.0.113.10:8443/dns-query`) przekazywany przy tworzeniu środowiska WebView2 przez `additional_browser_arguments` (`WebContext` / `WebViewBuilderExtWindows::with_additional_browser_args`).
   - **Zweryfikowany mechanizm (PoC, WebView2 158):** przed utworzeniem WebView aplikacja zapisuje do `<user_data_folder>\EBWebView\Local State` (JSON) klucz `"dns_over_https": {"mode": "secure", "templates": "<url>"}`. Wynik na `1.1.1.1/help`: DoH = Yes.
     - Plik nie istnieje przy pierwszym starcie → utworzyć z samym kluczem `dns_over_https`; jeśli istnieje → zmerge'ować, zachowując pozostałe klucze.
     - Zapis tylko gdy żaden `msedgewebview2.exe` nie trzyma profilu (stąd `--wait-pid` przy restarcie).
   - **Odrzucone:** flagi `--enable-features=DnsOverHttps<...>` + `--force-fieldtrial(-params)` docierają do procesu, ale DoH = No. Polityki HKCU niepotrzebne.
   - Weryfikacja działania: `https://1.1.1.1/help` (dla Cloudflare) lub Wireshark — brak zapytań UDP/53 z `msedgewebview2.exe`.
   - **Bootstrap:** adres DoH najlepiej jako IP; nazwa hosta serwera DoH rozwiązywana jest systemowym DNS.
   - Zmiana DoH w UI → walidacja → zapis `config.json` → **restart aplikacji** (bez przeładowania w locie).
3. **Restart procesu:**
   - Nowy proces uruchamiany z `--wait-pid <stary_pid>`; przed utworzeniem środowiska czeka na zakończenie starego procesu (blokada `user_data_folder` przez `msedgewebview2.exe`), dodatkowo retry tworzenia WebView.
4. **Izolacja profilu:** dedykowany `user_data_folder` w `%LOCALAPPDATA%\<NazwaAplikacji>\WebView2` (nie obok exe — brak praw zapisu w `Program Files`). Config w `%LOCALAPPDATA%\<NazwaAplikacji>\config.json`.
5. **Własne UI:**
   - Górny pasek (karty, adres, wstecz/dalej/odśwież, panel DoH) jako osobny `WebView` (child) z protokołem `ui://` (`with_custom_protocol`). Na Windows wry mapuje go na `http://ui.localhost/` — uwzględnić w CSP i ścieżkach.
   - Treść stron: osobny child `WebView` na kartę (`build_as_child` + `set_bounds` + `set_visible`).
   - Bounds liczone z uwzględnieniem DPI (`LogicalSize`/`PhysicalSize`, `scale_factor`).
   - Zdarzenia do UI: `with_on_page_load_handler`, `with_document_title_changed_handler`, `with_navigation_handler` (zmiana URL) → `evaluate_script` na pasku.
   - `with_new_window_req_handler`: `target=_blank` / `window.open` → nowa karta.
   - `with_download_started_handler` / `with_download_completed_handler`: pobieranie do `Downloads`.
   - Skróty: Ctrl+T, Ctrl+W, Ctrl+L, Ctrl+Tab, F5, Alt+←/→ (obsługa fokusu między paskiem a kartą).
6. **Bezpieczeństwo:**
   - IPC handler **tylko** w WebView paska UI, nigdy w kartach treści.
   - Handler IPC: whitelist komend (enum serde `#[serde(tag = "cmd")]`), odrzucanie nieznanych.
   - Walidacja URL DoH (schemat `https`, host, port 1–65535, brak spacji/cudzysłowów) — zapobiega wstrzyknięciu flag do `additional_browser_arguments`.
7. **Start:** sprawdzenie obecności WebView2 Runtime; przy braku — czytelny komunikat błędu (MessageBox) z linkiem do instalatora.

## Stack technologiczny (Rust)
- **Okno i pętla zdarzeń:** `tao`
- **Silnik przeglądarki:** `wry` (backend WebView2)
- **Konfiguracja:** `serde`, `serde_json`
- **Restart procesu:** `std::process::Command` + `std::env::current_exe()` + `--wait-pid`

## Kolejność prac
1. **PoC DoH** — jedno okno, jeden WebView, flagi DoH, weryfikacja że DoH faktycznie działa. ⚠ Blokuje resztę.
2. Config (`config.rs`) + restart z `--wait-pid`.
3. Pasek UI + IPC.
4. Karty.
5. Nowe okna, pobieranie, skróty, DPI.

## Struktura katalogów
```text
├── Cargo.toml
├── CLAUDE.md
├── src/
│   ├── main.rs          # Wejście, --wait-pid, config, EventLoop, okno, sprawdzenie WebView2 Runtime
│   ├── config.rs        # Ustawienia (DoH URL, strona startowa), walidacja, odczyt/zapis JSON w %LOCALAPPDATA%
│   ├── browser.rs       # Karty (child WebView), bounds/DPI, flagi DoH do WebView2, handlery zdarzeń
│   └── ipc.rs           # Komendy z UI (enum), nawigacja, karty, zapis DoH + restart
└── ui/
    ├── index.html       # Pasek kart, pasek adresu, panel ustawień DoH
    ├── style.css        # Dark mode
    └── app.js           # Logika UI, window.ipc.postMessage(...)
```
