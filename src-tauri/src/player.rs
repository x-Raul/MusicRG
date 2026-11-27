//! Ciclo de vida del reproductor (el WebView2 con la web de música):
//! - Qué web abre: Apple Music (por defecto), Spotify, `YouTube Music`, una
//!   dirección propia o ninguna (Off: sin procesos de WebView2).
//! - "Free memory when idle": con la ventana cerrada y nada sonando durante
//!   `IDLE_AFTER`, se destruye entero (WebView2 suelta sus procesos, unos
//!   100 MB) y MusicRG vuelve a ocupar lo mismo que al arrancar con Windows.
//!   Se vuelve a crear al abrirlo desde la bandeja.
//!
//! Un solo hilo (`rg-idle`) espera la cuenta atrás con una `Condvar`: sin
//! sondeo, y dormido del todo mientras no hay cuenta atrás.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::lock::lock;
use crate::settings::{self, Player};

/// Cuánto espera cerrado y en silencio antes de descargar el reproductor.
const IDLE_AFTER: Duration = Duration::from_mins(30);

#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
struct Idle {
    hidden: bool,
    own_playing: bool,
    ext_playing: bool,
    enabled: bool,
    /// Cuándo se descarga, si se cumplen las condiciones.
    deadline: Option<Instant>,
}

struct Watch {
    state: Mutex<Idle>,
    wake: Condvar,
    app: AppHandle,
}

static WATCH: OnceLock<Watch> = OnceLock::new();

/// Arranca la vigilancia (una vez, en el `setup`).
pub fn init(app: &AppHandle, enabled: bool) {
    let watch =
        Watch { state: Mutex::new(Idle { enabled, ..Idle::default() }), wake: Condvar::new(), app: app.clone() };
    if WATCH.set(watch).is_ok() {
        let _ = std::thread::Builder::new().name("rg-idle".into()).spawn(idle_loop);
    }
}

fn update(f: impl FnOnce(&mut Idle)) {
    let Some(w) = WATCH.get() else { return };
    let mut st = lock(&w.state);
    f(&mut st);
    let idle = st.enabled && st.hidden && !st.own_playing && !st.ext_playing;
    st.deadline = match (idle, st.deadline) {
        (true, Some(d)) => Some(d), // la cuenta atrás sigue
        (true, None) => Some(Instant::now() + idle_after()),
        (false, _) => None,
    };
    w.wake.notify_one();
}

/// La ventana del reproductor se escondió (`true`) o se mostró.
pub fn set_hidden(hidden: bool) {
    update(|s| s.hidden = hidden);
}

/// Lo que cuenta la propia MusicRG (página o su sesión multimedia).
pub fn set_own_playing(playing: bool) {
    update(|s| s.own_playing = playing);
}

/// Otra app sonando: no se descarga nada por si es el propio reproductor
/// con un servicio que no se identifica.
pub fn set_ext_playing(playing: bool) {
    update(|s| s.ext_playing = playing);
}

pub fn set_enabled(on: bool) {
    update(|s| s.enabled = on);
}

#[cfg(debug_assertions)]
fn idle_after() -> Duration {
    // Solo en desarrollo: MUSICRG_IDLE_SECS=10 para probarlo sin esperar.
    std::env::var("MUSICRG_IDLE_SECS").ok().and_then(|s| s.parse().ok()).map_or(IDLE_AFTER, Duration::from_secs)
}

#[cfg(not(debug_assertions))]
fn idle_after() -> Duration {
    IDLE_AFTER
}

fn idle_loop() {
    let Some(w) = WATCH.get() else { return };
    let mut st = lock(&w.state);
    loop {
        st = match st.deadline {
            None => w.wake.wait(st).unwrap_or_else(std::sync::PoisonError::into_inner),
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    st.deadline = None;
                    let app = w.app.clone();
                    let _ = w.app.run_on_main_thread(move || unload(&app));
                    st
                } else {
                    w.wake.wait_timeout(st, d - now).unwrap_or_else(std::sync::PoisonError::into_inner).0
                }
            }
        };
    }
}

/// Destruye el reproductor si sigue escondido. Hilo principal.
fn unload(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else { return };
    if win.is_visible().unwrap_or(true) {
        return;
    }
    #[cfg(debug_assertions)]
    eprintln!("[musicrg] player: unloaded after being idle");
    forget(app);
    let _ = win.destroy();
}

/// Lo que dependía de la página abierta deja de valer.
fn forget(app: &AppHandle) {
    crate::audio::BROWSER_PID.store(0, Ordering::Relaxed);
    lock(&app.state::<crate::tray::Pins>().state).live = false;
}

/// La dirección que abre el reproductor según los ajustes; `None` si el
/// reproductor está apagado (Off): entonces no se crea ningún WebView2.
pub fn url(app: &AppHandle) -> Option<String> {
    let prefs = crate::tray::current(app);
    let default = || Player::AppleMusic.url().unwrap_or_default().to_string();
    match (prefs.player, prefs.player.url()) {
        (Player::Off, _) => None,
        (_, Some(u)) => Some(u.to_string()),
        (_, None) => {
            Some(app.path().config_dir().ok().and_then(|d| settings::load_custom_url(&d)).unwrap_or_else(default))
        }
    }
}

/// ¿Está apagado el reproductor?
pub fn off(app: &AppHandle) -> bool {
    crate::tray::current(app).player == Player::Off
}

/// ¿Es Apple Music? Solo a esa web se le inyectan el tema y `shell.js` (y
/// solo ella puede llamar a los comandos de la app, ver `capabilities`).
pub fn is_apple(url: &str) -> bool {
    tauri::Url::parse(url).is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("music.apple.com"))
}

/// Cambió el servicio: si el reproductor existe, se rehace con la web nueva
/// (visible si lo estaba). Si no existe, la próxima vez ya abre la nueva.
pub fn reload(app: &AppHandle) {
    static BUSY: AtomicBool = AtomicBool::new(false);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(win) = handle.get_webview_window("main") else { return };
        if BUSY.swap(true, Ordering::SeqCst) {
            return;
        }
        let visible = win.is_visible().unwrap_or(false);
        forget(&handle);
        let _ = win.destroy();
        // La ventana se va del todo un poco después: se espera fuera del
        // hilo principal y entonces se crea la nueva.
        std::thread::spawn(move || {
            for _ in 0..60 {
                if handle.get_webview_window("main").is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            BUSY.store(false, Ordering::SeqCst);
            crate::open_player(&handle, visible);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::is_apple;

    #[test]
    fn only_apple_music_gets_the_shell() {
        assert!(is_apple("https://music.apple.com/"));
        assert!(is_apple("https://music.apple.com/es/browse"));
        assert!(!is_apple("http://music.apple.com/"));
        assert!(!is_apple("https://music.apple.com.evil.com/"));
        assert!(!is_apple("https://open.spotify.com/"));
    }
}
