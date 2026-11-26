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
use crate::settings::{self, Settings};
use crate::{autostart, awake, show, updater};

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

/// Casillas de ajustes del menú de la bandeja.
struct Toggles {
    widget: CheckMenuItem<Wry>,
    others: CheckMenuItem<Wry>,
    flat: CheckMenuItem<Wry>,
    caps: CheckMenuItem<Wry>,
    keep_awake: CheckMenuItem<Wry>,
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
        })
    }

    /// Lo que marcan ahora las casillas.
    fn read(&self) -> Settings {
        let d = Settings::default();
        let on = |item: &CheckMenuItem<Wry>, default: bool| item.is_checked().unwrap_or(default);
        Settings {
            widget: on(&self.widget, d.widget),
            caps: on(&self.caps, d.caps),
            others: on(&self.others, d.others),
            flat: on(&self.flat, d.flat),
            keep_awake: on(&self.keep_awake, d.keep_awake),
        }
    }
}

/// Aplica los ajustes que acaban de cambiar en el menú y los guarda.
fn apply_settings(app: &AppHandle, prefs: Settings, config_dir: Option<&Path>) {
    let overlay = app.state::<Overlay>();
    overlay.set_widget(prefs.widget);
    overlay.set_caps(prefs.caps);
    overlay.set_others(prefs.others);
    app.state::<Header>().set_others(prefs.others);
    overlay.set_flat(prefs.flat);
    awake::set(prefs.keep_awake);
    if let Some(dir) = config_dir {
        settings::save(dir, prefs);
    }
}

/// Monta el menú y el icono de la bandeja. Orden (spec 2026-11-25):
/// cabecera, reproducción, Pinned | Widget ›, casillas | actualizar, mostrar, salir.
pub fn build(app: &App, prefs: Settings, config_dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
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
    let cached = config_dir.as_deref().map_or_else(Vec::new, settings::load_pins);
    fill_pins(app.handle(), &pins_menu, &cached);
    app.manage(Pins { menu: pins_menu.clone(), state: Mutex::new(PinsState { names: cached, ..Default::default() }) });

    let toggles = Toggles::new(app, prefs)?;
    let widget_menu = Submenu::with_id_and_icon(app, "widget_menu", "Widget", true, Some(image(Icon::Widget, dark)))?;
    widget_menu.append_items(&[&toggles.widget, &toggles.others, &toggles.flat])?;
    let autostart_item =
        CheckMenuItem::with_id(app, "autostart", "Start with Windows", true, autostart::enabled(), None::<&str>)?;

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
            &toggles.caps,
            &toggles.keep_awake,
            &autostart_item,
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
            (update_item.clone(), Icon::Update),
            (show_item, Icon::Show),
            (quit, Icon::Quit),
        ],
        subs: vec![(pins_menu, Icon::Pin), (widget_menu, Icon::Widget)],
        dark: AtomicBool::new(dark),
    };
    app.state::<Header>().attach(app.handle(), header);

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("MusicRG")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show(app),
            "quit" => app.exit(0),
            "update" => updater::on_click(app),
            "autostart" => {
                autostart::set(autostart_item.is_checked().unwrap_or(false));
                // Lo que diga el registro, por si no se pudo escribir.
                let _ = autostart_item.set_checked(autostart::enabled());
            }
            id if id.starts_with("pin:") => {
                if let Ok(i) = id[4..].parse::<usize>() {
                    play_pin(app, i);
                }
            }
            "widget" | "caps" | "others" | "flat" | "keep_awake" => {
                apply_settings(app, toggles.read(), config_dir.as_deref());
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
