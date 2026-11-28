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
//! Un hilo con una ventana invisible recibe la entrada real de ratón,
//! teclado y touchpad (Raw Input, sin hooks) y cada pocos segundos mira
//! cuánto llevas sin tocar nada. No se usa `GetLastInputInfo`: un vídeo en
//! el navegador lo pone a cero cada pocos segundos (pide a Windows que no
//! apague la pantalla) y la pantalla no se apagaría nunca.
//!
//! Gasta lo mínimo: tras la primera entrada deja de escuchar hasta la
//! siguiente comprobación, así que el hilo despierta como mucho dos veces
//! cada 15 s (una por entrada, una por el temporizador), use quien use el
//! PC. Con la opción apagada no escucha nada.

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

/// Solo en desarrollo: una línea por comprobación en %TEMP%\musicrg-screen.log.
#[cfg(debug_assertions)]
fn debug_log(line: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("musicrg-screen.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "{t} {line}");
    }
}

/// Una comprobación: `last` = tick de la última entrada real, `now` = ahora.
fn check(last: u32, now: u32, done_for: &mut Option<u32>) {
    if !ENABLED.load(Ordering::Relaxed) {
        *done_for = None;
        return;
    }
    // Ticks de 32 bits: `wrapping_sub` aguanta la vuelta a 0 (cada 49 días).
    let idle_ms = now.wrapping_sub(last);
    let limit_ms = MINUTES.load(Ordering::Relaxed).saturating_mul(60_000);
    let done = *done_for == Some(last);
    let full = imp::fullscreen();
    #[cfg(debug_assertions)]
    debug_log(&format!(
        "idle={}s windows_idle={}s limit={}s fullscreen={full} done={done} {}",
        idle_ms / 1000,
        imp::windows_idle_ms() / 1000,
        limit_ms / 1000,
        imp::foreground_info()
    ));
    if should_turn_off(idle_ms, limit_ms, done, full) {
        imp::monitor_off();
        *done_for = Some(last);
    }
}

fn watch() {
    imp::run(CHECK, check);
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::SystemInformation::GetTickCount;
    #[cfg(debug_assertions)]
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK, RIDEV_REMOVE};
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_PRESENTATION_MODE};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW,
        GetWindowRect, PostMessageW, RegisterClassW, SetTimer, HWND_BROADCAST, HWND_MESSAGE, MSG, SC_MONITORPOWER,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT, WM_SYSCOMMAND, WM_TIMER, WNDCLASSW,
    };

    /// Hilo de "Screen care": ventana solo de mensajes que recibe la
    /// entrada real (Raw Input) y un temporizador que llama a `check` cada
    /// `every`. No vuelve nunca (el hilo vive lo que la app).
    pub fn run(every: Duration, check: fn(u32, u32, &mut Option<u32>)) {
        // SAFETY: todo en este hilo; la ventana y sus registros son suyos y
        // viven tanto como el bucle de mensajes.
        unsafe {
            let Ok(hinst) = GetModuleHandleW(None) else { return };
            let class = w!("MusicRGScreenCare");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&raw const wc);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                None,
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinst.into()),
                None,
            ) else {
                return;
            };
            let mut last = GetTickCount();
            let mut touched = false;
            listen(hwnd, true);
            let mut listening = true;
            let _ = SetTimer(Some(hwnd), T_CHECK, every.as_millis() as u32, None);
            let mut done_for = None;
            let mut msg = MSG::default();
            while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
                match (msg.message, msg.wParam.0) {
                    // Con saber que hubo entrada basta: deja de escuchar
                    // hasta la próxima comprobación.
                    (WM_INPUT, _) if listening => {
                        listen(hwnd, false);
                        listening = false;
                        touched = true;
                    }
                    (WM_TIMER, T_CHECK) => {
                        // Hubo entrada en este intervalo: se cuenta como si
                        // fuera ahora (como mucho se apaga 15 s tarde, nunca
                        // antes de tiempo).
                        let now = GetTickCount();
                        if std::mem::take(&mut touched) {
                            last = now;
                        }
                        check(last, now, &mut done_for);
                        let want = super::ENABLED.load(Ordering::Relaxed);
                        if want != listening {
                            listen(hwnd, want);
                            listening = want;
                        }
                    }
                    _ => {}
                }
                DispatchMessageW(&raw const msg);
            }
        }
    }

    const T_CHECK: usize = 1;

    /// Escucha (o deja de escuchar) ratón, teclado y touchpad, aunque la
    /// ventana no tenga el foco.
    unsafe fn listen(hwnd: HWND, on: bool) {
        // (página, uso): ratón, teclado, touchpad de precisión.
        let devices = [(0x01u16, 0x02u16), (0x01, 0x06), (0x0D, 0x05)].map(|(page, usage)| RAWINPUTDEVICE {
            usUsagePage: page,
            usUsage: usage,
            dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
            hwndTarget: if on { hwnd } else { HWND::default() },
        });
        let _ = RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }

    unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        DefWindowProcW(h, m, wp, lp)
    }

    /// Solo en desarrollo: lo que dice `GetLastInputInfo`, para comparar.
    #[cfg(debug_assertions)]
    pub fn windows_idle_ms() -> u32 {
        let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        // SAFETY: `li` es una variable local con `cbSize` correcto.
        if !unsafe { GetLastInputInfo(&raw mut li) }.as_bool() {
            return 0;
        }
        // SAFETY: sin argumentos ni efectos.
        unsafe { GetTickCount() }.wrapping_sub(li.dwTime)
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

    /// Solo en desarrollo: estado de notificaciones, clase y rectángulo de
    /// la ventana activa, y el monitor.
    #[cfg(debug_assertions)]
    pub fn foreground_info() -> String {
        // SAFETY: llamadas de solo lectura sobre variables locales.
        unsafe {
            let quns = SHQueryUserNotificationState().map_or(-1, |q| q.0);
            let fg = GetForegroundWindow();
            let mut buf = [0u16; 64];
            let n = GetClassNameW(fg, &mut buf).max(0) as usize;
            let class = String::from_utf16_lossy(&buf[..n]);
            let mut r = RECT::default();
            let _ = GetWindowRect(fg, &raw mut r);
            let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &raw mut mi);
            let m = mi.rcMonitor;
            format!(
                "quns={quns} class={class} win=({},{},{},{}) mon=({},{},{},{})",
                r.left, r.top, r.right, r.bottom, m.left, m.top, m.right, m.bottom
            )
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
    pub fn run(_: std::time::Duration, _: fn(u32, u32, &mut Option<u32>)) {}
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
