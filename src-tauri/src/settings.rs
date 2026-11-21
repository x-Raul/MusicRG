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
    }
}

pub fn save(dir: &std::path::Path, s: Settings) {
    let mut v = read(dir);
    if !v.is_object() {
        v = json!({});
    }
    v["widget"] = json!(s.widget);
    v["caps_lock_toast"] = json!(s.caps);
    let p = path(dir);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(p, text);
    }
}
