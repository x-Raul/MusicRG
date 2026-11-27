//! "Always on top": la ventana del reproductor se queda por encima de las
//! demás, para usarla de minirreproductor mientras trabajas.
//!
//! Se activa desde la bandeja o con la chincheta de la barra de la ventana
//! (`shell.js`); las dos cosas muestran siempre el mismo estado. Mientras la
//! ventana está maximizada no se aplica: taparía todo lo demás. Al
//! restaurarla vuelve sola.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tauri::{AppHandle, Manager, WebviewWindow};

/// Lo que ha pedido el usuario.
static WANTED: AtomicBool = AtomicBool::new(false);

/// Lo último aplicado a la ventana (0 no, 1 sí, 2 sin saber): así arrastrar
/// o redimensionar no llama a Windows en cada evento.
static APPLIED: AtomicU8 = AtomicU8::new(UNKNOWN);
const UNKNOWN: u8 = 2;

pub fn wanted() -> bool {
    WANTED.load(Ordering::Relaxed)
}

/// Al arrancar, antes de crear la ventana.
pub fn init(on: bool) {
    WANTED.store(on, Ordering::Relaxed);
}

/// La ventana se acaba de crear con `always_on_top(wanted())`.
pub fn created() {
    APPLIED.store(u8::from(wanted()), Ordering::Relaxed);
}

/// Cambia la opción, la aplica y lo refleja en la chincheta de la página.
pub fn set(app: &AppHandle, on: bool) {
    WANTED.store(on, Ordering::Relaxed);
    if let Some(win) = app.get_webview_window("main") {
        apply(&win);
        let _ = win.eval(format!("window.__rg&&window.__rg.setOnTop({on})"));
    }
}

/// Lo que toca según la opción y si la ventana está maximizada. Se llama
/// también cuando la ventana cambia de tamaño.
pub fn apply(win: &WebviewWindow) {
    let on = effective(wanted(), win.is_maximized().unwrap_or(false));
    if APPLIED.swap(u8::from(on), Ordering::Relaxed) != u8::from(on) {
        let _ = win.set_always_on_top(on);
    }
}

fn effective(wanted: bool, maximized: bool) -> bool {
    wanted && !maximized
}

#[cfg(test)]
mod tests {
    use super::effective;

    #[test]
    fn only_when_wanted_and_not_maximized() {
        assert!(effective(true, false));
        assert!(!effective(true, true));
        assert!(!effective(false, false));
        assert!(!effective(false, true));
    }
}
