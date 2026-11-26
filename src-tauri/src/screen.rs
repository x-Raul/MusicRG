//! "Screen care": si no tocas ratón ni teclado durante unos minutos (10 por
//! defecto), apaga la pantalla para que no se queme (OLED, LED, plasma). La
//! música sigue sonando y el PC no se suspende por esto.
//!
//! Es lo mismo que pasa al pulsar el botón del monitor: se enciende al mover
//! el ratón o pulsar una tecla. Funciona aunque "Keep PC awake" esté activo,
//! así que juntas sirven para dejar música puesta con la pantalla apagada.
//!
//! No apaga nada si hay algo a pantalla completa (vídeo, juego,
//! presentación): ahí es normal pasar rato sin tocar nada.
//!
//! Un hilo mira cada pocos segundos cuánto llevas sin tocar nada
//! (`GetLastInputInfo`); no hay hooks de teclado ni ratón.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Once;
use std::time::Duration;

/// Cada cuánto se mira el tiempo sin actividad. Como mucho se apaga este
/// tiempo más tarde de lo pedido.
const CHECK: Duration = Duration::from_secs(15);

static ENABLED: AtomicBool = AtomicBool::new(false);
static MINUTES: AtomicU32 = AtomicU32::new(crate::settings::SCREEN_OFF_DEFAULT);
static START: Once = Once::new();

/// Enciende o apaga la opción. El hilo se crea la primera vez que se activa.
pub fn set(on: bool, minutes: u32) {
    MINUTES.store(minutes, Ordering::Relaxed);
    ENABLED.store(on, Ordering::Relaxed);
    if on {
        START.call_once(|| {
            let _ = std::thread::Builder::new().name("screen-care".into()).spawn(watch);
        });
    }
}

/// ¿Toca apagar ahora? `done` = ya se apagó en esta racha sin actividad
/// (no se repite hasta que vuelvas a tocar algo).
fn should_turn_off(idle_ms: u32, limit_ms: u32, done: bool, fullscreen: bool) -> bool {
    !done && !fullscreen && idle_ms >= limit_ms
}

fn watch() {
    // Momento (tick) de la última actividad cuando se apagó la pantalla.
    let mut done_for: Option<u32> = None;
    loop {
        std::thread::sleep(CHECK);
        if !ENABLED.load(Ordering::Relaxed) {
            done_for = None;
            continue;
        }
        let Some((last, idle_ms)) = imp::idle() else { continue };
        let limit_ms = MINUTES.load(Ordering::Relaxed).saturating_mul(60_000);
        let done = done_for == Some(last);
        if should_turn_off(idle_ms, limit_ms, done, imp::fullscreen()) {
            imp::monitor_off();
            done_for = Some(last);
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_PRESENTATION_MODE};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowRect, PostMessageW, HWND_BROADCAST, SC_MONITORPOWER, WM_SYSCOMMAND,
    };

    /// `(tick de la última actividad, ms desde entonces)`.
    pub fn idle() -> Option<(u32, u32)> {
        let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        // SAFETY: `li` es una variable local con `cbSize` correcto.
        if !unsafe { GetLastInputInfo(&raw mut li) }.as_bool() {
            return None;
        }
        // SAFETY: sin argumentos ni efectos.
        let now = unsafe { GetTickCount() };
        // Los dos son ticks de 32 bits: `wrapping_sub` aguanta la vuelta a 0 (cada 49 días).
        Some((li.dwTime, now.wrapping_sub(li.dwTime)))
    }

    /// Modo presentación, o la ventana activa tapa todo su monitor (vídeo,
    /// juego...). Como en el widget, no se usa "D3D a pantalla completa":
    /// salta con juegos u overlays en segundo plano y no apagaría nunca.
    pub fn fullscreen() -> bool {
        // SAFETY: llamadas de solo lectura; `buf`, `mi` y `r` son locales y
        // viven toda la llamada.
        unsafe {
            if matches!(SHQueryUserNotificationState(), Ok(QUNS_PRESENTATION_MODE)) {
                return true;
            }
            let fg = GetForegroundWindow();
            if fg.is_invalid() {
                return false;
            }
            let mut buf = [0u16; 64];
            let n = GetClassNameW(fg, &mut buf).max(0) as usize;
            let class = String::from_utf16_lossy(&buf[..n]);
            // El escritorio también "tapa" el monitor entero.
            if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
                return false;
            }
            let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let mut r = RECT::default();
            if !GetMonitorInfoW(mon, &raw mut mi).as_bool() || GetWindowRect(fg, &raw mut r).is_err() {
                return false;
            }
            let m = mi.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
    }

    /// 2 = apagar (1 sería "ahorro", -1 encender). `PostMessage` no espera a
    /// que respondan las ventanas: una colgada no bloquea este hilo.
    pub fn monitor_off() {
        // SAFETY: mensaje estándar del sistema, sin punteros.
        let _ =
            unsafe { PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2)) };
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn idle() -> Option<(u32, u32)> {
        None
    }
    pub fn fullscreen() -> bool {
        false
    }
    pub fn monitor_off() {}
}

#[cfg(test)]
mod tests {
    use super::should_turn_off;

    const TEN_MIN: u32 = 10 * 60_000;

    #[test]
    fn waits_for_the_limit() {
        assert!(!should_turn_off(TEN_MIN - 1, TEN_MIN, false, false));
        assert!(should_turn_off(TEN_MIN, TEN_MIN, false, false));
    }

    #[test]
    fn only_once_per_idle_stretch() {
        assert!(!should_turn_off(TEN_MIN * 3, TEN_MIN, true, false));
    }

    #[test]
    fn never_over_fullscreen() {
        assert!(!should_turn_off(TEN_MIN * 3, TEN_MIN, false, true));
    }
}
