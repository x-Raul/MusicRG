#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod art;
mod audio;
mod autostart;
mod awake;
mod lock;
mod media;
mod menu_icons;
mod overlay;
mod screen;
mod settings;
mod tray;
mod updater;

use lock::lock;
use overlay::{NowPlaying, Overlay};
use settings::Settings;
use tauri::{AppHandle, Manager, Theme, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tray::{fill_pins, Header, Pins};

const URL: &str = "https://music.apple.com/";
const THEME_CSS: &str = include_str!("../inject/theme.css");
const SHELL_JS: &str = include_str!("../inject/shell.js");

/// Color de fondo del tema (--rg-bg). La barra de título se pinta igual.
const BG: (u8, u8, u8) = (0x0b, 0x0b, 0x0f);

/// Flags de Chromium. Los tres primeros `ms*` son los que Tauri pone por defecto;
/// el resto apaga servicios que un reproductor no usa. No se desactivan las
/// actualizaciones de componentes: el DRM (Widevine) llega por ahí.
const BROWSER_ARGS: &str = concat!(
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,",
    "msEdgeTranslate,Translate,OptimizationHints,MediaRouter,DialMediaRouteProvider,",
    "msEdgeShoppingUI,msEdgeCollections",
    " --autoplay-policy=no-user-gesture-required",
    " --renderer-process-limit=2",
    " --disable-domain-reliability",
    " --no-pings",
);

/// Solo en desarrollo: `MUSICRG_DEBUG_PORT=9222` abre la depuración remota de
/// Chromium para inspeccionar la página desde fuera.
fn browser_args() -> String {
    #[cfg(debug_assertions)]
    if let Ok(port) = std::env::var("MUSICRG_DEBUG_PORT") {
        return format!("{BROWSER_ARGS} --remote-debugging-port={port}");
    }
    BROWSER_ARGS.to_string()
}

// Los comandos de Tauri reciben sus argumentos por valor (los deserializa el
// propio Tauri), así que `needless_pass_by_value` no aplica.

/// La página avisa de cada cambio de canción o de play/pausa (ver `shell.js`).
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn now_playing(
    app: AppHandle,
    overlay: tauri::State<Overlay>,
    playing: bool,
    title: String,
    artist: String,
    art: String,
) {
    #[cfg(debug_assertions)]
    {
        eprintln!("[musicrg] now_playing playing={playing} title={title:?} art={}", !art.is_empty());
        if std::env::var_os("MUSICRG_DEMO").is_some() {
            return; // en modo demo manda la canción falsa
        }
    }
    let clip = |s: &str| s.chars().take(300).collect::<String>();
    let np = NowPlaying { playing, title: clip(&title), artist: clip(&artist), art: clip(&art) };
    app.state::<Header>().set_own(&np);
    overlay.now_playing(np);
}

/// La página manda los nombres de lo que hay en Destacar (ver `shell.js`), en
/// orden. Al pulsar uno, la página reproduce el de esa posición: aquí solo
/// viajan nombres, nada que se vaya a ejecutar.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn pins(app: AppHandle, pins: tauri::State<Pins>, names: Vec<String>) {
    let names: Vec<String> = names.into_iter().take(30).map(|n| n.chars().take(200).collect()).collect();
    let mut st = lock(&pins.state);
    if st.names != names {
        fill_pins(&app, &pins.menu, &names);
        if let Ok(dir) = app.path().config_dir() {
            settings::save_pins(&dir, &names);
        }
    }
    st.names = names;
    st.live = true;
    if let Some(want) = st.pending.take() {
        if let (Some(i), Some(win)) = (st.names.iter().position(|n| *n == want), app.get_webview_window("main")) {
            let _ = win.eval(format!("window.__rg&&window.__rg.playPin({i})"));
        }
    }
}

/// Botones propios de la ventana (no hay barra de título nativa). Solo estas
/// cuatro acciones: la página no recibe acceso general a la API de ventanas.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn window_ctl(window: WebviewWindow, action: String) {
    let _ = match action.as_str() {
        "minimize" => window.minimize(),
        "maximize" if window.is_maximized().unwrap_or(false) => window.unmaximize(),
        "maximize" => window.maximize(),
        // Igual que la X nativa: pasa por `CloseRequested` y se esconde en la bandeja.
        "close" => window.close(),
        "drag" => window.start_dragging(),
        _ => Ok(()),
    };
}

fn main() {
    // Solo en desarrollo: MUSICRG_OSD_DEMO=1 alterna el aviso de Bloq Mayús
    // cada pocos segundos y deja convivir con la MusicRG instalada.
    let osd_demo = cfg!(debug_assertions) && std::env::var_os("MUSICRG_OSD_DEMO").is_some();
    let mut builder = tauri::Builder::default();
    if !osd_demo {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)));
    }
    let result = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![now_playing, window_ctl, pins])
        .setup(move |app| {
            let config_dir = app.path().config_dir().ok();
            let prefs = config_dir.as_deref().map_or_else(Settings::default, settings::load);
            let handle = app.handle().clone();
            let overlay = Overlay::spawn(prefs.widget, prefs.caps, prefs.others, move || show(&handle));
            overlay.set_flat(prefs.flat);
            app.manage(overlay);
            awake::set(prefs.keep_awake);
            screen::set(prefs.screen_care, settings::screen_off_minutes());
            // La cabecera del menú existe desde ya: lo que suene antes de que
            // se monte el menú queda guardado y se muestra al montarlo.
            app.manage(Header::new(prefs.others));
            // Lo que suena en otras apps (navegador, Spotify...) para el widget
            // y para la cabecera del menú.
            let handle = app.handle().clone();
            media::spawn(move |np| {
                handle.state::<Header>().set_ext(&np);
                overlay::external(np);
            });
            #[cfg(debug_assertions)]
            dev_demos(app.handle(), osd_demo);

            // Arrancada por Windows al iniciar sesión: solo widget, aviso de
            // Bloq Mayús y bandeja. El reproductor (WebView2, cientos de MB) no
            // se crea hasta que se pide.
            if !autostart::launched_at_login() {
                open_player(app.handle(), true);
            }
            tray::build(app, prefs, config_dir)
        })
        .on_window_event(|window, event| {
            // Cerrar = esconder en la bandeja; la música sigue. "Quit" está en el menú.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                if let Some(win) = window.app_handle().get_webview_window(window.label()) {
                    set_low_memory(&win, true);
                }
            }
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("[musicrg] failed to start MusicRG: {e}");
        std::process::exit(1);
    }
}

/// Solo en desarrollo: `MUSICRG_DEMO=1` finge una canción sonando para probar
/// el widget sin tocar el reproductor; `MUSICRG_OSD_DEMO=1` alterna el aviso.
#[cfg(debug_assertions)]
fn dev_demos(app: &AppHandle, osd_demo: bool) {
    if std::env::var_os("MUSICRG_DEMO").is_some() {
        app.state::<Overlay>().now_playing(NowPlaying {
            playing: true,
            title: "Dark Dream (MusicRG demo)".into(),
            artist: "Test artist".into(),
            art: std::env::var("MUSICRG_DEMO_ART").unwrap_or_default(),
        });
    }
    if osd_demo {
        let handle = app.clone();
        let slow: f32 = std::env::var("MUSICRG_OSD_SLOW").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        std::thread::spawn(move || {
            let mut on = true;
            loop {
                std::thread::sleep(std::time::Duration::from_secs_f32(1.0));
                handle.state::<Overlay>().demo_osd(on);
                std::thread::sleep(std::time::Duration::from_secs_f32(2.2 * slow));
                on = !on;
            }
        });
    }
}

fn show(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(win) => {
            set_low_memory(&win, false);
            let _ = win.unminimize();
            let _ = win.show();
            let _ = win.set_focus();
        }
        None => open_player(app, true),
    }
}

/// Crea la ventana del reproductor (la página de Apple Music). Se hace en
/// otro hilo: crear un WebView2 desde un evento del hilo principal puede
/// bloquear la app en Windows.
fn open_player(app: &AppHandle, visible: bool) {
    static OPENING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if OPENING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if app.get_webview_window("main").is_none() {
            if let Err(e) = build_player(&app, visible) {
                eprintln!("[musicrg] no se pudo abrir el reproductor: {e}");
            }
        }
        OPENING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

fn build_player(app: &AppHandle, visible: bool) -> Result<(), Box<dyn std::error::Error>> {
    let css = match app.path().config_dir() {
        Ok(dir) => {
            let user = std::fs::read_to_string(dir.join("MusicRG").join("theme.css")).unwrap_or_default();
            format!("{THEME_CSS}\n{user}")
        }
        Err(_) => THEME_CSS.to_string(),
    };
    let script = SHELL_JS.replace("__RG_CSS__", &serde_json::to_string(&css)?);
    let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(URL.parse()?))
        .title("MusicRG")
        .visible(visible)
        .inner_size(1180.0, 760.0)
        .min_inner_size(420.0, 320.0)
        .center()
        // Sin barra de título: la página ocupa toda la ventana y
        // shell.js pone los botones de minimizar / maximizar / cerrar.
        .decorations(false)
        .shadow(true)
        .theme(Some(Theme::Dark))
        .background_color(tauri::webview::Color(BG.0, BG.1, BG.2, 255))
        .additional_browser_args(&browser_args())
        .initialization_script(&script)
        .build()?;
    remember_browser_pid(&win);
    if visible {
        let _ = win.set_focus();
    } else {
        set_low_memory(&win, true);
    }
    Ok(())
}

/// Con la ventana en la bandeja, pide a WebView2 que suelte la memoria que
/// pueda (caches, páginas inactivas). El audio sigue sonando.
#[cfg(windows)]
fn set_low_memory(win: &WebviewWindow, low: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    use windows_core::Interface;
    let level =
        if low { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
    let _ = win.with_webview(move |wv| {
        // SAFETY: Tauri llama a esto en el hilo de la interfaz, con el
        // controlador de WebView2 vivo durante todo el cierre.
        unsafe {
            if let Ok(core) = wv.controller().CoreWebView2() {
                if let Ok(core) = core.cast::<ICoreWebView2_19>() {
                    let _ = core.SetMemoryUsageTargetLevel(level);
                }
            }
        }
    });
}

/// El widget captura el audio del árbol de procesos de WebView2.
#[cfg(windows)]
fn remember_browser_pid(win: &WebviewWindow) {
    let _ = win.with_webview(|wv| {
        let mut pid = 0u32;
        // SAFETY: Tauri llama a esto en el hilo de la interfaz, con el
        // controlador vivo; `pid` es una variable local que vive toda la llamada.
        let ok = unsafe { wv.controller().CoreWebView2().and_then(|core| core.BrowserProcessId(&raw mut pid)) };
        if ok.is_ok() {
            audio::BROWSER_PID.store(pid, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

#[cfg(not(windows))]
fn remember_browser_pid(_: &WebviewWindow) {}

#[cfg(not(windows))]
fn set_low_memory(_: &WebviewWindow, _: bool) {}
