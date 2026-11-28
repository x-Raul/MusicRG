//! Ajustes en `%APPDATA%\MusicRG\settings.json`. Se cambian desde la
//! ventana de ajustes o el menú de la bandeja; `widget_offset` solo a mano,
//! por si el widget choca con algo, y `screen_off_minutes` también a mano
//! (minutos de "Screen care").

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use serde_json::{json, Value};

/// Cuándo pide MusicRG la resolución de temporizador más fina (0,5 ms).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimerMode {
    /// Nunca: lo que decida Windows.
    #[default]
    Off,
    /// Solo mientras hay un juego (o algo) a pantalla completa.
    Auto,
    /// Siempre que MusicRG esté abierta.
    Always,
}

impl TimerMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Auto, Self::Always];

    fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

/// Qué web abre el reproductor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Player {
    #[default]
    AppleMusic,
    Spotify,
    YouTubeMusic,
    /// La dirección que haya escrito el usuario (`custom_url`).
    Custom,
    /// Sin reproductor: MusicRG no abre ningún proceso de navegador
    /// (WebView2); quedan el widget, el aviso y la bandeja.
    Off,
}

impl Player {
    pub const ALL: [Self; 5] = [Self::AppleMusic, Self::Spotify, Self::YouTubeMusic, Self::Custom, Self::Off];

    fn key(self) -> &'static str {
        match self {
            Self::AppleMusic => "apple_music",
            Self::Spotify => "spotify",
            Self::YouTubeMusic => "youtube_music",
            Self::Custom => "custom",
            Self::Off => "off",
        }
    }

    /// Dirección fija de cada servicio (`None` para la propia y para Off).
    pub fn url(self) -> Option<&'static str> {
        match self {
            Self::AppleMusic => Some("https://music.apple.com/"),
            Self::Spotify => Some("https://open.spotify.com/"),
            Self::YouTubeMusic => Some("https://music.youtube.com/"),
            Self::Custom | Self::Off => None,
        }
    }
}

/// Cada campo es una opción independiente de la ventana de ajustes.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Widget "sonando ahora" sobre la barra de tareas.
    pub widget: bool,
    /// Aviso al pulsar Bloq Mayús.
    pub caps: bool,
    /// El widget muestra también lo que suena en otras apps (navegador...).
    pub others: bool,
    /// Barras del widget apoyadas abajo en vez de centradas.
    pub flat: bool,
    /// Impide que el PC se suspenda y que se apague la pantalla.
    pub keep_awake: bool,
    /// Apaga la pantalla tras unos minutos sin tocar ratón ni teclado.
    pub screen_care: bool,
    /// Ventana del reproductor siempre por encima de las demás.
    pub on_top: bool,
    pub timer: TimerMode,
    /// Con el reproductor cerrado y nada sonando un rato, se descarga entero.
    pub free_idle: bool,
    pub player: Player,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            widget: true,
            caps: true,
            others: true,
            flat: false,
            keep_awake: false,
            screen_care: false,
            on_top: false,
            timer: TimerMode::Off,
            free_idle: true,
            player: Player::AppleMusic,
        }
    }
}

static OFFSET: OnceLock<Option<f32>> = OnceLock::new();
static SCREEN_OFF: AtomicU32 = AtomicU32::new(SCREEN_OFF_DEFAULT);

/// Minutos que se ofrecen para "Screen care" (en la ventana y en la bandeja).
pub const SCREEN_MINUTES: [u32; 5] = [5, 10, 15, 30, 60];

/// Posición en `SCREEN_MINUTES` del valor más cercano a `m` (puede venir
/// otro a mano en el JSON).
pub fn nearest_screen_minutes(m: u32) -> usize {
    (0..SCREEN_MINUTES.len()).min_by_key(|&i| SCREEN_MINUTES[i].abs_diff(m)).unwrap_or(1)
}

/// Minutos sin actividad antes de apagar la pantalla, si no se cambian.
pub const SCREEN_OFF_DEFAULT: u32 = 10;

/// Distancia (en píxeles a escala 100 %) desde el borde izquierdo de la barra.
pub fn widget_offset() -> Option<f32> {
    OFFSET.get().copied().flatten()
}

/// Minutos de "Screen care" (`screen_off_minutes`, de 1 a 240).
pub fn screen_off_minutes() -> u32 {
    SCREEN_OFF.load(Ordering::Relaxed)
}

/// Cambia los minutos de "Screen care" (de 1 a 240) y los guarda.
pub fn set_screen_off_minutes(dir: Option<&std::path::Path>, minutes: u32) {
    let m = minutes.clamp(1, 240);
    SCREEN_OFF.store(m, Ordering::Relaxed);
    if let Some(dir) = dir {
        update(dir, |v| v["screen_off_minutes"] = json!(m));
    }
}

fn minutes_from_json(v: &Value) -> u32 {
    v["screen_off_minutes"].as_u64().map_or(SCREEN_OFF_DEFAULT, |m| m.clamp(1, 240) as u32)
}

fn path(dir: &std::path::Path) -> PathBuf {
    dir.join("MusicRG").join("settings.json")
}

fn read(dir: &std::path::Path) -> Value {
    std::fs::read_to_string(path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

pub fn load(dir: &std::path::Path) -> Settings {
    let v = read(dir);
    let _ = OFFSET.set(v["widget_offset"].as_f64().map(|f| f as f32));
    SCREEN_OFF.store(minutes_from_json(&v), Ordering::Relaxed);
    from_json(&v)
}

/// Lo que falte o no tenga el tipo esperado se queda con el valor por defecto.
fn from_json(v: &Value) -> Settings {
    let d = Settings::default();
    let flag = |key: &str, default: bool| v[key].as_bool().unwrap_or(default);
    // Antes era una casilla (`timer_resolution`): marcada equivale a "siempre".
    let timer = match v["timer_mode"].as_str() {
        Some(s) => TimerMode::ALL.into_iter().find(|m| m.key() == s).unwrap_or(d.timer),
        None if v["timer_resolution"].as_bool() == Some(true) => TimerMode::Always,
        None => d.timer,
    };
    let player = v["player"].as_str().and_then(|s| Player::ALL.into_iter().find(|p| p.key() == s)).unwrap_or(d.player);
    Settings {
        widget: flag("widget", d.widget),
        caps: flag("caps_lock_toast", d.caps),
        others: flag("other_apps", d.others),
        flat: flag("flat_bars", d.flat),
        keep_awake: flag("keep_awake", d.keep_awake),
        screen_care: flag("screen_care", d.screen_care),
        on_top: flag("always_on_top", d.on_top),
        timer,
        free_idle: flag("free_memory_when_idle", d.free_idle),
        player,
    }
}

pub fn save(dir: &std::path::Path, s: Settings) {
    update(dir, |v| {
        v["widget"] = json!(s.widget);
        v["caps_lock_toast"] = json!(s.caps);
        v["other_apps"] = json!(s.others);
        v["flat_bars"] = json!(s.flat);
        v["keep_awake"] = json!(s.keep_awake);
        v["screen_care"] = json!(s.screen_care);
        v["always_on_top"] = json!(s.on_top);
        v["timer_mode"] = json!(s.timer.key());
        v["free_memory_when_idle"] = json!(s.free_idle);
        v["player"] = json!(s.player.key());
        if let Some(o) = v.as_object_mut() {
            o.remove("timer_resolution");
        }
    });
}

/// Dirección propia del reproductor ("custom"), ya comprobada.
pub fn load_custom_url(dir: &std::path::Path) -> Option<String> {
    read(dir)["custom_url"].as_str().and_then(clean_url)
}

pub fn save_custom_url(dir: &std::path::Path, url: &str) {
    update(dir, |v| v["custom_url"] = json!(url));
}

/// Acepta solo `https://` con un host; añade el esquema si falta. Devuelve
/// la dirección tal como se va a abrir.
pub fn clean_url(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() || s.len() > 2000 || s.chars().any(char::is_whitespace) {
        return None;
    }
    let s = if s.contains("://") { s.to_string() } else { format!("https://{s}") };
    let url = tauri::Url::parse(&s).ok()?;
    (url.scheme() == "https" && url.host_str().is_some_and(|h| h.contains('.'))).then(|| url.to_string())
}

/// Última lista de Destacar que mandó la página: así el menú de la bandeja
/// la tiene aunque el reproductor aún no se haya abierto.
pub fn load_pins(dir: &std::path::Path) -> Vec<String> {
    read(dir)["pins"].as_array().map_or_default(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
}

pub fn save_pins(dir: &std::path::Path, names: &[String]) {
    update(dir, |v| v["pins"] = json!(names));
}

fn update(dir: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read(dir);
    if !v.is_object() {
        v = json!({});
    }
    f(&mut v);
    let p = path(dir);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(p, text);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{clean_url, from_json, minutes_from_json, Player, Settings, TimerMode, SCREEN_OFF_DEFAULT};

    #[test]
    fn missing_keys_use_defaults() {
        assert_eq!(from_json(&serde_json::Value::Null), Settings::default());
        assert_eq!(from_json(&json!({ "widget": "yes", "timer_mode": "fast" })), Settings::default());
    }

    #[test]
    fn reads_every_option() {
        let v = json!({
            "widget": false, "caps_lock_toast": false, "other_apps": false,
            "flat_bars": true, "keep_awake": true, "screen_care": true,
            "always_on_top": true, "timer_mode": "auto",
            "free_memory_when_idle": false, "player": "off",
        });
        let s = from_json(&v);
        assert!(
            !s.widget && !s.caps && !s.others && s.flat && s.keep_awake && s.screen_care && s.on_top && !s.free_idle
        );
        assert_eq!(s.timer, TimerMode::Auto);
        assert_eq!(s.player, Player::Off);
    }

    #[test]
    fn screen_off_minutes_are_clamped() {
        assert_eq!(minutes_from_json(&serde_json::Value::Null), SCREEN_OFF_DEFAULT);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": 5 })), 5);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": 0 })), 1);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": 9999 })), 240);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": "x" })), SCREEN_OFF_DEFAULT);
    }

    #[test]
    fn old_timer_checkbox_means_always() {
        assert_eq!(from_json(&json!({ "timer_resolution": true })).timer, TimerMode::Always);
        assert_eq!(from_json(&json!({ "timer_resolution": false })).timer, TimerMode::Off);
        assert_eq!(from_json(&json!({ "timer_resolution": true, "timer_mode": "off" })).timer, TimerMode::Off);
    }

    #[test]
    fn urls_are_https_only() {
        assert_eq!(clean_url("music.youtube.com").as_deref(), Some("https://music.youtube.com/"));
        assert_eq!(clean_url(" https://tidal.com/browse ").as_deref(), Some("https://tidal.com/browse"));
        assert_eq!(clean_url("http://example.com"), None);
        assert_eq!(clean_url("file:///C:/x.html"), None);
        assert_eq!(clean_url("javascript:alert(1)"), None);
        assert_eq!(clean_url("localhost"), None);
        assert_eq!(clean_url("a b.com"), None);
        assert_eq!(clean_url(""), None);
    }
}
