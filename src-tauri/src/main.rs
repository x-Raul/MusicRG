#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod art;
mod audio;
mod autostart;
mod media;
mod overlay;
mod settings;
mod updater;

use overlay::{NowPlaying, Overlay};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Theme, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry,
};

/// Submenú "Pinned" de la bandeja: lo que el usuario tiene en Destacar.
struct Pins {
    menu: Submenu<Wry>,
    state: std::sync::Mutex<PinsState>,
}

#[derive(Default)]
struct PinsState {
    /// Nombres en el orden del menú.
    names: Vec<String>,
    /// La lista viene de la página abierta ahora (no de la caché): sus
    /// posiciones coinciden con las de shell.js.
    live: bool,
    /// Pedido desde la bandeja con el reproductor aún cargando.
    pending: Option<String>,
}

/// Rehace el submenú con esta lista.
fn fill_pins(app: &AppHandle, sub: &Submenu<Wry>, names: &[String]) {
    if let Ok(items) = sub.items() {
        for item in items {
            let _ = sub.remove(&item);
        }
    }
    for (i, name) in names.iter().enumerate() {
        let label: String = name.chars().take(60).collect::<String>().replace('&', "&&");
        if let Ok(item) = MenuItem::with_id(app, format!("pin:{i}"), label, true, None::<&str>) {
            let _ = sub.append(&item);
        }
    }
    if names.is_empty() {
        if let Ok(item) = MenuItem::with_id(app, "pin:none", "Nothing pinned", false, None::<&str>) {
            let _ = sub.append(&item);
        }
    }
}

/// Clic en algo de "Pinned". Si el reproductor no está abierto (arranque con
/// Windows) se abre escondido y suena en cuanto la página tenga la lista.
fn play_pin(app: &AppHandle, i: usize) {
    let pins = app.state::<Pins>();
    let mut st = pins.state.lock().unwrap();
    let Some(name) = st.names.get(i).cloned() else { return };
    match app.get_webview_window("main") {
        Some(win) if st.live => {
            let _ = win.eval(format!("window.__rg&&window.__rg.playPin({i})"));
        }
        win => {
            st.pending = Some(name);
            if win.is_none() {
                open_player(app, false);
            }
        }
    }
}

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

/// Solo en desarrollo: MUSICRG_DEBUG_PORT=9222 abre la depuración remota de
/// Chromium para inspeccionar la página desde fuera.
fn browser_args() -> String {
    #[cfg(debug_assertions)]
    if let Ok(port) = std::env::var("MUSICRG_DEBUG_PORT") {
        return format!("{BROWSER_ARGS} --remote-debugging-port={port}");
    }
    BROWSER_ARGS.to_string()
}

/// La página avisa de cada cambio de canción o de play/pausa (ver shell.js).
#[tauri::command]
fn now_playing(overlay: tauri::State<Overlay>, playing: bool, title: String, artist: String, art: String) {
    #[cfg(debug_assertions)]
    {
        eprintln!("[musicrg] now_playing playing={playing} title={title:?} art={}", !art.is_empty());
        if std::env::var_os("MUSICRG_DEMO").is_some() {
            return; // en modo demo manda la canción falsa
        }
    }
    let clip = |s: String| s.chars().take(300).collect::<String>();
    overlay.now_playing(NowPlaying { playing, title: clip(title), artist: clip(artist), art: clip(art) });
}

/// La página manda los nombres de lo que hay en Destacar (ver shell.js), en
/// orden. Al pulsar uno, la página reproduce el de esa posición: aquí solo
/// viajan nombres, nada que se vaya a ejecutar.
#[tauri::command]
fn pins(app: AppHandle, pins: tauri::State<Pins>, names: Vec<String>) {
    let names: Vec<String> = names.into_iter().take(30).map(|n| n.chars().take(200).collect()).collect();
    let mut st = pins.state.lock().unwrap();
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
fn window_ctl(window: WebviewWindow, action: String) {
    let _ = match action.as_str() {
        "minimize" => window.minimize(),
        "maximize" if window.is_maximized().unwrap_or(false) => window.unmaximize(),
        "maximize" => window.maximize(),
        // Igual que la X nativa: pasa por CloseRequested y se esconde en la bandeja.
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
    builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![now_playing, window_ctl, pins])
        .setup(move |app| {
            let config_dir = app.path().config_dir().ok();
            let prefs = config_dir
                .as_deref()
                .map(settings::load)
                .unwrap_or(settings::Settings { widget: true, caps: true, others: true, flat: false });
            let handle = app.handle().clone();
            let overlay = Overlay::spawn(prefs.widget, prefs.caps, prefs.others, move || show(&handle));
            overlay.set_flat(prefs.flat);
            app.manage(overlay);
            // Lo que suena en otras apps (navegador, Spotify...) para el widget.
            media::spawn(overlay::external);
            // Solo en desarrollo: MUSICRG_DEMO=1 finge una canción sonando para
            // probar el widget sin tocar el reproductor.
            #[cfg(debug_assertions)]
            if std::env::var_os("MUSICRG_DEMO").is_some() {
                app.state::<Overlay>().now_playing(NowPlaying {
                    playing: true,
                    title: "Dark Dream (MusicRG demo)".into(),
                    artist: "Test artist".into(),
                    art: std::env::var("MUSICRG_DEMO_ART").unwrap_or_default(),
                });
            }
            #[cfg(debug_assertions)]
            if osd_demo {
                let handle = app.handle().clone();
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

            // Arrancada por Windows al iniciar sesión: solo widget, aviso de
            // Bloq Mayús y bandeja. El reproductor (WebView2, cientos de MB) no
            // se crea hasta que se pide.
            if !autostart::launched_at_login() {
                open_player(app.handle(), true);
            }

            let widget_item = CheckMenuItem::with_id(
                app, "widget", "Taskbar widget", true, prefs.widget, None::<&str>,
            )?;
            let caps_item = CheckMenuItem::with_id(
                app, "caps", "Caps Lock toast", true, prefs.caps, None::<&str>,
            )?;
            let others_item = CheckMenuItem::with_id(
                app, "others", "Show other apps in widget", true, prefs.others, None::<&str>,
            )?;
            let flat_item = CheckMenuItem::with_id(
                app, "flat", "Flat audio bars", true, prefs.flat, None::<&str>,
            )?;
            let autostart_item = CheckMenuItem::with_id(
                app, "autostart", "Start with Windows", true, autostart::enabled(), None::<&str>,
            )?;
            // Primero la última lista guardada; la página la actualiza al cargar.
            let pins_menu = Submenu::with_id(app, "pins", "Pinned", true)?;
            let cached = config_dir.as_deref().map(settings::load_pins).unwrap_or_default();
            fill_pins(app.handle(), &pins_menu, &cached);
            app.manage(Pins {
                menu: pins_menu.clone(),
                state: std::sync::Mutex::new(PinsState { names: cached, ..Default::default() }),
            });
            let update_item = updater::menu_item(app.handle())?;
            let (wi, ci, oi, fi, ai) = (
                widget_item.clone(),
                caps_item.clone(),
                others_item.clone(),
                flat_item.clone(),
                autostart_item.clone(),
            );
            let menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "toggle", "Play / Pause", true, None::<&str>)?,
                    &MenuItem::with_id(app, "next", "Next", true, None::<&str>)?,
                    &MenuItem::with_id(app, "prev", "Previous", true, None::<&str>)?,
                    &pins_menu,
                    &PredefinedMenuItem::separator(app)?,
                    &widget_item,
                    &others_item,
                    &flat_item,
                    &caps_item,
                    &autostart_item,
                    &PredefinedMenuItem::separator(app)?,
                    &update_item,
                    &MenuItem::with_id(app, "show", "Show MusicRG", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
                ],
            )?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("MusicRG")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "show" => show(app),
                    "quit" => app.exit(0),
                    "update" => updater::on_click(app),
                    "autostart" => {
                        autostart::set(ai.is_checked().unwrap_or(false));
                        // Lo que diga el registro, por si no se pudo escribir.
                        let _ = ai.set_checked(autostart::enabled());
                    }
                    id if id.starts_with("pin:") => {
                        if let Ok(i) = id[4..].parse::<usize>() {
                            play_pin(app, i);
                        }
                    }
                    "widget" | "caps" | "others" | "flat" => {
                        let prefs = settings::Settings {
                            widget: wi.is_checked().unwrap_or(true),
                            caps: ci.is_checked().unwrap_or(true),
                            others: oi.is_checked().unwrap_or(true),
                            flat: fi.is_checked().unwrap_or(false),
                        };
                        let overlay = app.state::<Overlay>();
                        overlay.set_widget(prefs.widget);
                        overlay.set_caps(prefs.caps);
                        overlay.set_others(prefs.others);
                        overlay.set_flat(prefs.flat);
                        if let Some(dir) = &config_dir {
                            settings::save(dir, prefs);
                        }
                    }
                    cmd => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.eval(format!("window.__rg&&window.__rg.cmd('{cmd}')"));
                        }
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            updater::start(app.handle(), update_item);
            Ok(())
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
        .run(tauri::generate_context!())
        .expect("failed to start MusicRG");
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
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    use windows_core::Interface;
    let _ = win.with_webview(move |wv| unsafe {
        let level = if low {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        };
        if let Ok(core) = wv.controller().CoreWebView2() {
            if let Ok(core) = core.cast::<ICoreWebView2_19>() {
                let _ = core.SetMemoryUsageTargetLevel(level);
            }
        }
    });
}

/// El widget captura el audio del árbol de procesos de WebView2.
#[cfg(windows)]
fn remember_browser_pid(win: &WebviewWindow) {
    let _ = win.with_webview(|wv| unsafe {
        if let Ok(core) = wv.controller().CoreWebView2() {
            let mut pid = 0u32;
            if core.BrowserProcessId(&mut pid).is_ok() {
                audio::BROWSER_PID.store(pid, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
}

#[cfg(not(windows))]
fn remember_browser_pid(_: &WebviewWindow) {}

#[cfg(not(windows))]
fn set_low_memory(_: &WebviewWindow, _: bool) {}
