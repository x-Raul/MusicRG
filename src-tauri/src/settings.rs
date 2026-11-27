//! Ajustes en `%APPDATA%\MusicRG\settings.json`. Se cambian desde el menú de
//! la bandeja; `widget_offset` solo a mano, por si el widget choca con algo, y
//! `screen_off_minutes` también a mano (minutos de "Screen care").

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Value};

/// Cada campo es una casilla independiente del menú de la bandeja.
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
        }
    }
}

static OFFSET: OnceLock<Option<f32>> = OnceLock::new();
static SCREEN_OFF: OnceLock<u32> = OnceLock::new();

/// Minutos sin actividad antes de apagar la pantalla, si no se cambian.
pub const SCREEN_OFF_DEFAULT: u32 = 10;

/// Distancia (en píxeles a escala 100 %) desde el borde izquierdo de la barra.
pub fn widget_offset() -> Option<f32> {
    OFFSET.get().copied().flatten()
}

/// Minutos de "Screen care" (`screen_off_minutes`, de 1 a 240).
pub fn screen_off_minutes() -> u32 {
    SCREEN_OFF.get().copied().unwrap_or(SCREEN_OFF_DEFAULT)
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
    let _ = SCREEN_OFF.set(minutes_from_json(&v));
    from_json(&v)
}

/// Lo que falte o no sea `bool` se queda con el valor por defecto.
fn from_json(v: &Value) -> Settings {
    let d = Settings::default();
    let flag = |key: &str, default: bool| v[key].as_bool().unwrap_or(default);
    Settings {
        widget: flag("widget", d.widget),
        caps: flag("caps_lock_toast", d.caps),
        others: flag("other_apps", d.others),
        flat: flag("flat_bars", d.flat),
        keep_awake: flag("keep_awake", d.keep_awake),
        screen_care: flag("screen_care", d.screen_care),
        on_top: flag("always_on_top", d.on_top),
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
    });
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

    use super::{from_json, minutes_from_json, Settings, SCREEN_OFF_DEFAULT};

    #[test]
    fn missing_keys_use_defaults() {
        assert_eq!(from_json(&serde_json::Value::Null), Settings::default());
        assert_eq!(from_json(&json!({ "widget": "yes" })), Settings::default());
    }

    #[test]
    fn reads_every_flag() {
        let v = json!({
            "widget": false, "caps_lock_toast": false, "other_apps": false,
            "flat_bars": true, "keep_awake": true, "screen_care": true,
            "always_on_top": true,
        });
        let s = from_json(&v);
        assert!(!s.widget && !s.caps && !s.others && s.flat && s.keep_awake && s.screen_care && s.on_top);
    }

    #[test]
    fn screen_off_minutes_are_clamped() {
        assert_eq!(minutes_from_json(&serde_json::Value::Null), SCREEN_OFF_DEFAULT);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": 5 })), 5);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": 0 })), 1);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": 9999 })), 240);
        assert_eq!(minutes_from_json(&json!({ "screen_off_minutes": "x" })), SCREEN_OFF_DEFAULT);
    }
}
