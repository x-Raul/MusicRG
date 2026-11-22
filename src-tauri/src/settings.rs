//! Ajustes en `%APPDATA%\MusicRG\settings.json`. Se cambian desde el menú de
//! la bandeja; `widget_offset` solo a mano, por si el widget choca con algo.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub struct Settings {
    /// Widget "sonando ahora" sobre la barra de tareas.
    pub widget: bool,
    /// Aviso al pulsar Bloq Mayús.
    pub caps: bool,
    /// El widget muestra también lo que suena en otras apps (navegador...).
    pub others: bool,
    /// Barras del widget apoyadas abajo en vez de centradas.
    pub flat: bool,
}

static OFFSET: OnceLock<Option<f32>> = OnceLock::new();

/// Distancia (en píxeles a escala 100 %) desde el borde izquierdo de la barra.
pub fn widget_offset() -> Option<f32> {
    OFFSET.get().copied().flatten()
}

fn path(dir: &std::path::Path) -> PathBuf {
    dir.join("MusicRG").join("settings.json")
}

fn read(dir: &std::path::Path) -> Value {
    std::fs::read_to_string(path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

pub fn load(dir: &std::path::Path) -> Settings {
    let v = read(dir);
    let _ = OFFSET.set(v["widget_offset"].as_f64().map(|f| f as f32));
    Settings {
        widget: v["widget"].as_bool().unwrap_or(true),
        caps: v["caps_lock_toast"].as_bool().unwrap_or(true),
        others: v["other_apps"].as_bool().unwrap_or(true),
        flat: v["flat_bars"].as_bool().unwrap_or(false),
    }
}

pub fn save(dir: &std::path::Path, s: Settings) {
    update(dir, |v| {
        v["widget"] = json!(s.widget);
        v["caps_lock_toast"] = json!(s.caps);
        v["other_apps"] = json!(s.others);
        v["flat_bars"] = json!(s.flat);
    });
}

/// Última lista de Destacar que mandó la página: así el menú de la bandeja
/// la tiene aunque el reproductor aún no se haya abierto.
pub fn load_pins(dir: &std::path::Path) -> Vec<String> {
    read(dir)["pins"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
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
