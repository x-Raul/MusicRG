//! Menú del clic derecho en el icono de la bandeja.
//!
//! Es el menú nativo de Windows (el de Tauri): no cuesta memoria ni tiempo de
//! apertura. Los iconos vienen ya rasterizados de `menu_icons`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::{
    menu::{CheckMenuItem, IconMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Wry,
};

use crate::lock::lock;
use crate::menu_icons::{dark_menu, image, Icon};
use crate::overlay::{NowPlaying, Overlay};
use crate::panel::{self, Change, Key};
use crate::settings::Player;
use crate::settings::{self, Settings};
use crate::{autostart, awake, on_top, player, screen, show, timer, updater};

/// Máximo de caracteres de la cabecera, contando la elipsis: no más ancha
/// que "Show other apps in widget", para que el menú no se ensanche.
const HEADER_MAX: usize = 28;
const NOTHING: &str = "Nothing playing";

/// Lo que muestra la cabecera: lo mismo que el widget. Primero MusicRG; si
/// no suena, otra app (solo con "Show other apps" activo).
pub fn header_text(own: &NowPlaying, ext: &NowPlaying, others_on: bool) -> String {
    let np = if own.playing {
        own
    } else if ext.playing && others_on {
        ext
    } else {
        return NOTHING.into();
    };
    let full = if np.artist.is_empty() { np.title.clone() } else { format!("{} — {}", np.title, np.artist) };
    let text: String = if full.chars().count() > HEADER_MAX {
        full.chars().take(HEADER_MAX - 1).chain(['…']).collect()
    } else {
        full
    };
    // Windows toma `&` como marca de atajo de teclado.
    text.replace('&', "&&")
}

#[derive(Default)]
struct HeaderState {
    own: NowPlaying,
    ext: NowPlaying,
    others_on: bool,
    shown: String,
    /// Sube con cada texto nuevo: así se descarta uno viejo que llegue tarde.
    generation: u64,
}

/// Recibe `(generación, texto)` cada vez que la cabecera cambia.
type Sink = Box<dyn Fn(u64, String) + Send + Sync>;

/// Primera línea del menú: lo que suena. Se crea al arrancar (antes que el
/// menú) para no perder lo que llegue mientras tanto.
pub struct Header {
    sink: OnceLock<Sink>,
    state: Mutex<HeaderState>,
}

impl Header {
    pub fn new(others_on: bool) -> Self {
        Self { sink: OnceLock::new(), state: Mutex::new(HeaderState { others_on, ..Default::default() }) }
    }

    pub fn set_own(&self, np: &NowPlaying) {
        self.update(|st| st.own = np.clone());
    }

    pub fn set_ext(&self, np: &NowPlaying) {
        self.update(|st| st.ext = np.clone());
    }

    fn set_others(&self, on: bool) {
        self.update(|st| st.others_on = on);
    }

    /// Engancha el item del menú. Escribir en él espera al hilo principal,
    /// así que se encola allí sin esperar; el hilo principal aplica los
    /// textos en orden y se salta los que ya no son el último.
    fn attach(&self, app: &AppHandle, item: IconMenuItem<Wry>) {
        let app = app.clone();
        self.attach_sink(Box::new(move |generation, text| {
            let (handle, item) = (app.clone(), item.clone());
            let _ = app.run_on_main_thread(move || {
                if handle.state::<Header>().is_latest(generation) {
                    let _ = item.set_text(&text);
                }
            });
        }));
    }

    fn attach_sink(&self, sink: Sink) {
        let _ = self.sink.set(sink);
        self.update(|st| st.shown.clear()); // fuerza el primer texto
    }

    fn is_latest(&self, generation: u64) -> bool {
        lock(&self.state).generation == generation
    }

    /// Calcula el texto con el candado y lo publica ya sin él: el que recibe
    /// el texto puede esperar a otro hilo que también quiera el candado.
    fn update(&self, f: impl FnOnce(&mut HeaderState)) {
        let Some(sink) = self.sink.get() else {
            f(&mut lock(&self.state));
            return;
        };
        let pending = {
            let mut st = lock(&self.state);
            f(&mut st);
            let text = header_text(&st.own, &st.ext, st.others_on);
            (text != st.shown).then(|| {
                st.shown.clone_from(&text);
                st.generation += 1;
                (st.generation, text)
            })
        };
        if let Some((generation, text)) = pending {
            sink(generation, text);
        }
    }
}

/// Items con icono, para cambiar la tinta cuando Windows cambia de tema.
struct Icons {
    items: Vec<(IconMenuItem<Wry>, Icon)>,
    subs: Vec<(Submenu<Wry>, Icon)>,
    dark: AtomicBool,
}

impl Icons {
    /// Se llama justo antes de abrir el menú: si el tema cambió, cambia la tinta.
    fn refresh(&self) {
        let dark = dark_menu();
        if self.dark.swap(dark, Ordering::Relaxed) == dark {
            return;
        }
        for (item, icon) in &self.items {
            let _ = item.set_icon(Some(image(*icon, dark)));
        }
        for (sub, icon) in &self.subs {
            let _ = sub.set_icon(Some(image(*icon, dark)));
        }
    }
}

/// Submenú "Pinned" de la bandeja: lo que el usuario tiene en Destacar.
pub struct Pins {
    pub menu: Submenu<Wry>,
    pub state: Mutex<PinsState>,
}

#[derive(Default)]
pub struct PinsState {
    /// Nombres en el orden del menú.
    pub names: Vec<String>,
    /// La lista viene de la página abierta ahora (no de la caché): sus
    /// posiciones coinciden con las de shell.js.
    pub live: bool,
    /// Pedido desde la bandeja con el reproductor aún cargando.
    pub pending: Option<String>,
}

/// Rehace el submenú con esta lista.
pub fn fill_pins(app: &AppHandle, sub: &Submenu<Wry>, names: &[String]) {
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
    let mut st = lock(&pins.state);
    let Some(name) = st.names.get(i).cloned() else { return };
    match app.get_webview_window("main") {
        Some(win) if st.live => {
            let _ = win.eval(format!("window.__rg&&window.__rg.playPin({i})"));
        }
        win => {
            st.pending = Some(name);
            if win.is_none() {
                crate::open_player(app, false);
            }
        }
    }
}

/// Texto de la casilla de "Screen care", con sus minutos.
fn screen_label() -> String {
    format!("Screen care ({} min)", settings::screen_off_minutes())
}

/// Casillas de ajustes del menú de la bandeja.
struct Toggles {
    widget: CheckMenuItem<Wry>,
    others: CheckMenuItem<Wry>,
    flat: CheckMenuItem<Wry>,
    caps: CheckMenuItem<Wry>,
    keep_awake: CheckMenuItem<Wry>,
    screen_care: CheckMenuItem<Wry>,
    on_top: CheckMenuItem<Wry>,
}

impl Toggles {
    fn new(app: &App, prefs: Settings) -> tauri::Result<Self> {
        let check = |id: &str, text: &str, on: bool| CheckMenuItem::with_id(app, id, text, true, on, None::<&str>);
        Ok(Self {
            widget: check("widget", "Taskbar widget", prefs.widget)?,
            others: check("others", "Show other apps in widget", prefs.others)?,
            flat: check("flat", "Flat audio bars", prefs.flat)?,
            caps: check("caps", "Caps Lock toast", prefs.caps)?,
            keep_awake: check("keep_awake", "Keep PC awake", prefs.keep_awake)?,
            screen_care: check("screen_care", &screen_label(), prefs.screen_care)?,
            on_top: check("on_top", "Always on top", prefs.on_top)?,
        })
    }

    /// Lo que marcan ahora las casillas; lo que no está en el menú (el
    /// timer) se queda como en `base`.
    fn read(&self, base: Settings) -> Settings {
        let on = |item: &CheckMenuItem<Wry>, default: bool| item.is_checked().unwrap_or(default);
        Settings {
            widget: on(&self.widget, base.widget),
            caps: on(&self.caps, base.caps),
            others: on(&self.others, base.others),
            flat: on(&self.flat, base.flat),
            keep_awake: on(&self.keep_awake, base.keep_awake),
            screen_care: on(&self.screen_care, base.screen_care),
            on_top: on(&self.on_top, base.on_top),
            ..base
        }
    }

    /// Pone las casillas como dicen los ajustes (tras un cambio en la ventana).
    fn show(&self, s: Settings) {
        for (item, on) in [
            (&self.widget, s.widget),
            (&self.others, s.others),
            (&self.flat, s.flat),
            (&self.caps, s.caps),
            (&self.keep_awake, s.keep_awake),
            (&self.screen_care, s.screen_care),
            (&self.on_top, s.on_top),
        ] {
            let _ = item.set_checked(on);
        }
        let _ = self.screen_care.set_text(screen_label());
    }
}

/// Ajustes vigentes: los comparten el menú de la bandeja y la ventana de ajustes.
pub struct Prefs {
    cur: Mutex<Settings>,
    dir: Option<PathBuf>,
}

impl Prefs {
    pub fn new(prefs: Settings, dir: Option<PathBuf>) -> Self {
        Self { cur: Mutex::new(prefs), dir }
    }
}

/// Los ajustes vigentes.
pub fn current(app: &AppHandle) -> Settings {
    app.try_state::<Prefs>().map_or_else(Settings::default, |p| *lock(&p.cur))
}

/// Casillas de la bandeja, para reflejar lo que se cambie en la ventana.
struct Checks {
    toggles: Toggles,
    autostart: CheckMenuItem<Wry>,
}

/// Aplica unos ajustes nuevos, los guarda y los deja como vigentes. Se puede
/// llamar desde cualquier hilo: nada de aquí espera al hilo principal.
fn apply_settings(app: &AppHandle, prefs: Settings) {
    let state = app.state::<Prefs>();
    *lock(&state.cur) = prefs;
    let overlay = app.state::<Overlay>();
    overlay.set_widget(prefs.widget);
    overlay.set_caps(prefs.caps);
    overlay.set_others(prefs.others);
    app.state::<Header>().set_others(prefs.others);
    overlay.set_flat(prefs.flat);
    awake::set(prefs.keep_awake);
    screen::set(prefs.screen_care, settings::screen_off_minutes());
    on_top::set(app, prefs.on_top);
    timer::set_mode(prefs.timer);
    player::set_enabled(prefs.free_idle);
    if let Some(dir) = &state.dir {
        settings::save(dir, prefs);
    }
}

/// Chincheta de la barra de la ventana: cambia "Always on top" igual que la
/// casilla del menú. Devuelve cómo queda.
pub fn toggle_on_top(app: &AppHandle) -> bool {
    let Some(checks) = app.try_state::<Checks>() else { return on_top::wanted() };
    let on = !on_top::wanted();
    let _ = checks.toggles.on_top.set_checked(on);
    let mut prefs = current(app);
    prefs.on_top = on;
    apply_settings(app, prefs);
    panel::refresh();
    on
}

fn set_key(s: &mut Settings, key: Key, on: bool) {
    match key {
        Key::Widget => s.widget = on,
        Key::Others => s.others = on,
        Key::Flat => s.flat = on,
        Key::Caps => s.caps = on,
        Key::KeepAwake => s.keep_awake = on,
        Key::FreeIdle => s.free_idle = on,
        Key::Autostart | Key::Global => {}
    }
}

/// Abre la ventana de ajustes (o la trae al frente).
pub fn open_settings(app: &AppHandle) {
    let (reader, writer) = (app.clone(), app.clone());
    panel::open(panel::Hooks {
        read: Box::new(move || {
            let state = reader.state::<Prefs>();
            let prefs = *lock(&state.cur);
            let custom_url = state.dir.as_deref().and_then(settings::load_custom_url).unwrap_or_default();
            panel::Values {
                prefs,
                autostart: autostart::enabled(),
                custom_url,
                screen_minutes: settings::screen_off_minutes(),
            }
        }),
        write: Box::new(move |change| {
            let state = writer.state::<Prefs>();
            let mut prefs = *lock(&state.cur);
            match change {
                Change::Toggle(Key::Autostart, on) => autostart::set(on),
                Change::Toggle(key, on) => {
                    set_key(&mut prefs, key, on);
                    apply_settings(&writer, prefs);
                }
                Change::Timer(mode) => {
                    prefs.timer = mode;
                    apply_settings(&writer, prefs);
                }
                Change::Player(p) => {
                    if prefs.player != p {
                        prefs.player = p;
                        apply_settings(&writer, prefs);
                        player::reload(&writer);
                    }
                }
                Change::ScreenCare(on, minutes) => {
                    settings::set_screen_off_minutes(state.dir.as_deref(), minutes);
                    prefs.screen_care = on;
                    apply_settings(&writer, prefs);
                }
                Change::CustomUrl(url) => {
                    if let Some(dir) = &state.dir {
                        settings::save_custom_url(dir, &url);
                    }
                    prefs.player = Player::Custom;
                    apply_settings(&writer, prefs);
                    player::reload(&writer);
                }
            }
            // Las casillas del menú, en el hilo principal y sin esperarlo.
            let app = writer.clone();
            let _ = writer.run_on_main_thread(move || {
                if let Some(checks) = app.try_state::<Checks>() {
                    checks.toggles.show(*lock(&app.state::<Prefs>().cur));
                    let _ = checks.autostart.set_checked(autostart::enabled());
                }
            });
        }),
    });
}

/// Monta el menú y el icono de la bandeja. Orden (spec 2026-11-25):
/// cabecera, reproducción, Pinned | Widget ›, casillas | actualizar, mostrar, salir.
#[allow(clippy::too_many_lines)]
pub fn build(app: &App, prefs: Settings, config_dir: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let dark = dark_menu();
    let icon_item = |id: &str, text: &str, enabled: bool, icon: Icon| {
        IconMenuItem::with_id(app, id, text, enabled, Some(image(icon, dark)), None::<&str>)
    };
    let header = icon_item("now", NOTHING, false, Icon::Music)?;
    let play = icon_item("toggle", "Play / Pause", true, Icon::Play)?;
    let next = icon_item("next", "Next", true, Icon::Next)?;
    let prev = icon_item("prev", "Previous", true, Icon::Prev)?;
    let update_item = updater::menu_item(app.handle(), image(Icon::Update, dark))?;
    let show_item = icon_item("show", "Show MusicRG", true, Icon::Show)?;
    let quit = icon_item("quit", "Quit", true, Icon::Quit)?;

    // Primero la última lista guardada; la página la actualiza al cargar.
    let pins_menu = Submenu::with_id_and_icon(app, "pins", "Pinned", true, Some(image(Icon::Pin, dark)))?;
    let cached = config_dir.map_or_else(Vec::new, settings::load_pins);
    fill_pins(app.handle(), &pins_menu, &cached);
    app.manage(Pins { menu: pins_menu.clone(), state: Mutex::new(PinsState { names: cached, ..Default::default() }) });

    let toggles = Toggles::new(app, prefs)?;
    let widget_menu = Submenu::with_id_and_icon(app, "widget_menu", "Widget", true, Some(image(Icon::Widget, dark)))?;
    widget_menu.append_items(&[&toggles.widget, &toggles.others, &toggles.flat])?;
    let autostart_item =
        CheckMenuItem::with_id(app, "autostart", "Start with Windows", true, autostart::enabled(), None::<&str>)?;
    let settings_item = icon_item("settings", "Settings\u{2026}", true, Icon::Settings)?;

    let menu = Menu::with_items(
        app,
        &[
            &header,
            &PredefinedMenuItem::separator(app)?,
            &play,
            &next,
            &prev,
            &pins_menu,
            &PredefinedMenuItem::separator(app)?,
            &widget_menu,
            &toggles.on_top,
            &toggles.caps,
            &toggles.keep_awake,
            &toggles.screen_care,
            &autostart_item,
            &settings_item,
            &PredefinedMenuItem::separator(app)?,
            &update_item,
            &show_item,
            &quit,
        ],
    )?;
    let icons = Icons {
        items: vec![
            (header.clone(), Icon::Music),
            (play, Icon::Play),
            (next, Icon::Next),
            (prev, Icon::Prev),
            (settings_item, Icon::Settings),
            (update_item.clone(), Icon::Update),
            (show_item, Icon::Show),
            (quit, Icon::Quit),
        ],
        subs: vec![(pins_menu, Icon::Pin), (widget_menu, Icon::Widget)],
        dark: AtomicBool::new(dark),
    };
    app.state::<Header>().attach(app.handle(), header);
    app.manage(Checks { toggles, autostart: autostart_item });

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("MusicRG")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show(app),
            "quit" => app.exit(0),
            "update" => updater::on_click(app),
            "settings" => open_settings(app),
            "autostart" => {
                let checks = app.state::<Checks>();
                autostart::set(checks.autostart.is_checked().unwrap_or(false));
                // Lo que diga el registro, por si no se pudo escribir.
                let _ = checks.autostart.set_checked(autostart::enabled());
                panel::refresh();
            }
            id if id.starts_with("pin:") => {
                if let Ok(i) = id[4..].parse::<usize>() {
                    play_pin(app, i);
                }
            }
            "widget" | "caps" | "others" | "flat" | "keep_awake" | "screen_care" | "on_top" => {
                let base = *lock(&app.state::<Prefs>().cur);
                apply_settings(app, app.state::<Checks>().toggles.read(base));
                panel::refresh();
            }
            cmd @ ("toggle" | "next" | "prev") => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.eval(format!("window.__rg&&window.__rg.cmd('{cmd}')"));
                }
            }
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| match event {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                show(tray.app_handle());
            }
            // Antes de que Windows abra el menú (se abre al soltar).
            TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Down, .. } => {
                icons.refresh();
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    updater::start(app.handle(), update_item);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{header_text, Header};
    use crate::lock::lock;
    use crate::overlay::NowPlaying;

    fn np(playing: bool, title: &str, artist: &str) -> NowPlaying {
        NowPlaying { playing, title: title.into(), artist: artist.into(), art: String::new() }
    }

    #[test]
    fn own_song_with_artist() {
        assert_eq!(header_text(&np(true, "A", "B"), &np(false, "", ""), true), "A — B");
    }

    #[test]
    fn other_app_without_artist_when_allowed() {
        assert_eq!(header_text(&np(false, "Mine", "Me"), &np(true, "X", ""), true), "X");
    }

    #[test]
    fn other_app_hidden_when_disabled() {
        assert_eq!(header_text(&np(false, "", ""), &np(true, "X", ""), false), "Nothing playing");
    }

    #[test]
    fn nothing_playing() {
        assert_eq!(header_text(&np(false, "A", "B"), &np(false, "X", "Y"), true), "Nothing playing");
    }

    #[test]
    fn long_title_is_clipped_to_menu_width() {
        // 28: no más ancho que la opción más larga del menú, como antes.
        let t = header_text(&np(true, &"a".repeat(100), ""), &np(false, "", ""), true);
        assert_eq!(t.chars().count(), 28);
        assert!(t.ends_with('…'));
    }

    /// Escribir en el menú espera al hilo principal; si se hiciera con el
    /// candado tomado y el hilo principal quisiera el candado, la app se
    /// congelaría. El escritor tiene que encontrarlo libre.
    #[test]
    fn writes_menu_text_without_holding_the_lock() {
        let header = Arc::new(Header::new(true));
        let free = Arc::new(AtomicBool::new(false));
        let (h, f) = (Arc::downgrade(&header), free.clone());
        header.attach_sink(Box::new(move |_, _| {
            if let Some(h) = h.upgrade() {
                f.store(h.state.try_lock().is_ok(), Ordering::SeqCst);
            }
        }));
        header.set_own(&np(true, "A", "B"));
        assert!(free.load(Ordering::SeqCst));
    }

    /// Dos hilos pueden publicar textos casi a la vez: solo el último vale.
    #[test]
    fn only_latest_text_is_applied() {
        let header = Header::new(true);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        header.attach_sink(Box::new(move |generation, text| lock(&s).push((generation, text))));
        header.set_own(&np(true, "A", ""));
        header.set_own(&np(true, "B", ""));
        let seen = lock(&seen).clone();
        let (old, new) = (seen[seen.len() - 2].0, seen[seen.len() - 1].0);
        assert!(!header.is_latest(old));
        assert!(header.is_latest(new));
        assert_eq!(seen[seen.len() - 1].1, "B");
    }

    #[test]
    fn ampersand_is_escaped() {
        assert_eq!(header_text(&np(true, "Tom & Jerry", ""), &np(false, "", ""), true), "Tom && Jerry");
    }
}
