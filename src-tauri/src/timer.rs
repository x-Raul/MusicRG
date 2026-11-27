//! Timer resolution: lo mismo que hace "Set Timer Resolution" (botón
//! "Maximum"), pero desde MusicRG y recordado entre arranques.
//!
//! Dos piezas, porque Windows 11 las separa:
//! - **La petición** (`set`): `NtSetTimerResolution` pide el intervalo más
//!   fino que admite el equipo (normalmente 0,5 ms). Desde Windows 10 2004 eso
//!   solo acelera los temporizadores *de MusicRG*. Además, Windows 11 ignora
//!   la petición de un proceso sin ventanas visibles, y MusicRG vive en la
//!   bandeja; por eso se le dice que no la ignore (`ProcessPowerThrottling`).
//!   Windows la suelta sola si la app se cierra.
//! - **El modo global** (`global`): el valor de registro
//!   `GlobalTimerResolutionRequests = 1` devuelve el comportamiento antiguo,
//!   en el que la petición vale para todo el sistema (juegos incluidos). Es
//!   de HKLM, así que se escribe con `reg.exe` elevado (el usuario acepta el
//!   aviso de UAC) y no cuenta hasta reiniciar.

/// Resoluciones del temporizador del sistema, en unidades de 100 ns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Res {
    /// La más gruesa (normalmente 156 250 = 15,625 ms).
    pub min: u32,
    /// La más fina (normalmente 5000 = 0,5 ms).
    pub max: u32,
    /// La que hay ahora en el sistema (la más fina que alguien pide).
    pub cur: u32,
}

/// Unidades de 100 ns a milisegundos.
pub fn ms(units: u32) -> f32 {
    units as f32 / 10_000.0
}

/// Estado del modo global en el registro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Global {
    Off,
    On,
    /// Escrito en esta sesión de Windows: falta reiniciar para que cuente.
    OnAfterRestart,
    OffAfterRestart,
}

impl Global {
    /// Lo que se verá tras reiniciar.
    pub fn wanted(self) -> bool {
        matches!(self, Self::On | Self::OnAfterRestart)
    }
}

/// Decide el estado a partir del valor y de si la clave se escribió después
/// de arrancar Windows. Puro, para los tests.
fn global_from(value: Option<u32>, written_since_boot: bool) -> Global {
    match (value == Some(1), written_since_boot) {
        (true, true) => Global::OnAfterRestart,
        (true, false) => Global::On,
        // Solo MusicRG escribe un 0 (al desactivarlo); "no existe" es lo normal.
        (false, true) if value == Some(0) => Global::OffAfterRestart,
        (false, _) => Global::Off,
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, FILETIME};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
        RRF_RT_REG_DWORD,
    };
    use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemTimeAsFileTime, GetTickCount64};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, ProcessPowerThrottling, SetProcessInformation, WaitForSingleObject,
        INFINITE, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        PROCESS_POWER_THROTTLING_STATE,
    };
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    use super::{global_from, Global, Res};

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryTimerResolution(min: *mut u32, max: *mut u32, cur: *mut u32) -> i32;
        fn NtSetTimerResolution(desired: u32, set: u8, cur: *mut u32) -> i32;
    }

    /// ¿Tiene MusicRG su petición puesta?
    static ON: AtomicBool = AtomicBool::new(false);

    pub fn query() -> Option<Res> {
        let (mut min, mut max, mut cur) = (0u32, 0u32, 0u32);
        // SAFETY: los tres punteros son variables locales vivas durante la llamada.
        let st = unsafe { NtQueryTimerResolution(&raw mut min, &raw mut max, &raw mut cur) };
        (st >= 0 && max > 0).then_some(Res { min, max, cur })
    }

    pub fn active() -> bool {
        ON.load(Ordering::Relaxed)
    }

    pub fn set(on: bool) {
        if ON.swap(on, Ordering::SeqCst) == on {
            return; // ya está como se pide
        }
        let Some(res) = query() else {
            ON.store(false, Ordering::SeqCst);
            return;
        };
        honor_when_hidden(on);
        let mut cur = 0u32;
        // SAFETY: `cur` es local. Pedir o soltar solo afecta a este proceso.
        let st = unsafe { NtSetTimerResolution(res.max, u8::from(on), &raw mut cur) };
        if on && st < 0 {
            ON.store(false, Ordering::SeqCst);
            honor_when_hidden(false);
        }
    }

    /// Windows 11 ignora la petición de un proceso sin ventanas a la vista
    /// (MusicRG en la bandeja). Con `on` se le pide que la respete siempre;
    /// sin él vuelve a decidir Windows.
    fn honor_when_hidden(on: bool) {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: if on { PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION } else { 0 },
            StateMask: 0,
        };
        // SAFETY: `state` es local y el tamaño es el suyo; el handle del
        // proceso actual es un pseudo-handle que no hace falta cerrar.
        let _ = unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&raw const state).cast(),
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        };
    }

    const KERNEL: PCWSTR = w!("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\kernel");
    const KERNEL_REG: &str = "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\kernel";
    const VALUE: PCWSTR = w!("GlobalTimerResolutionRequests");

    pub fn global() -> Global {
        let (mut v, mut n) = (0u32, 4u32);
        // SAFETY: `v` y `n` son locales; `n` es el tamaño real de `v`. Leer
        // HKLM no necesita permisos de administrador.
        let r = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                KERNEL,
                VALUE,
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut v).cast()),
                Some(&raw mut n),
            )
        };
        let value = (r == ERROR_SUCCESS).then_some(v);
        global_from(value, written_since_boot())
    }

    /// ¿La clave `kernel` cambió después de arrancar Windows? Es la forma
    /// barata de saber si el valor ya está en vigor o espera a reiniciar.
    fn written_since_boot() -> bool {
        let mut key = HKEY::default();
        // SAFETY: `key` es local; se cierra antes de salir.
        if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, KERNEL, None, KEY_READ, &raw mut key) } != ERROR_SUCCESS {
            return false;
        }
        let mut written = FILETIME::default();
        // SAFETY: `key` está abierta y `written` es local; el resto no se pide.
        let r = unsafe {
            RegQueryInfoKeyW(key, None, None, None, None, None, None, None, None, None, None, Some(&raw mut written))
        };
        // SAFETY: la clave se abrió arriba y no se usa después.
        let _ = unsafe { RegCloseKey(key) };
        if r != ERROR_SUCCESS {
            return false;
        }
        let ticks = |f: FILETIME| u64::from(f.dwHighDateTime) << 32 | u64::from(f.dwLowDateTime);
        // SAFETY: funciones sin parámetros de entrada.
        let (now, up_ms) = unsafe { (GetSystemTimeAsFileTime(), GetTickCount64()) };
        let boot = ticks(now).saturating_sub(up_ms * 10_000);
        ticks(written) > boot
    }

    /// Escribe el modo global con `reg.exe` elevado. Bloquea hasta que el
    /// usuario responde al aviso de UAC y `reg.exe` termina: llamarlo desde un
    /// hilo aparte. Devuelve `false` si el usuario lo cancela o falla.
    pub fn set_global(on: bool) -> bool {
        let mut dir = [0u16; 260];
        // SAFETY: `dir` es un búfer local de su tamaño.
        let n = unsafe { GetSystemDirectoryW(Some(&mut dir)) } as usize;
        if n == 0 || n >= dir.len() {
            return false;
        }
        // Ruta completa a System32: nada de buscar `reg.exe` en el PATH.
        let exe = HSTRING::from(format!("{}\\reg.exe", String::from_utf16_lossy(&dir[..n])));
        let args = HSTRING::from(format!(
            "add \"{KERNEL_REG}\" /v GlobalTimerResolutionRequests /t REG_DWORD /d {} /f",
            u8::from(on)
        ));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(exe.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: `info` y las cadenas que apunta viven toda la llamada.
        if unsafe { ShellExecuteExW(&raw mut info) }.is_err() || info.hProcess.is_invalid() {
            return false; // UAC cancelado
        }
        let mut code = 1u32;
        // SAFETY: `hProcess` es el proceso que se acaba de lanzar; se cierra al final.
        unsafe {
            WaitForSingleObject(info.hProcess, INFINITE);
            let _ = GetExitCodeProcess(info.hProcess, &raw mut code);
            let _ = CloseHandle(info.hProcess);
        }
        code == 0
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Global, Res};
    pub fn query() -> Option<Res> {
        None
    }
    pub fn active() -> bool {
        false
    }
    pub fn set(_: bool) {}
    pub fn global() -> Global {
        Global::Off
    }
    pub fn set_global(_: bool) -> bool {
        false
    }
}

pub use imp::{active, global, query, set, set_global};

use std::sync::atomic::{AtomicU8, Ordering};

use crate::settings::TimerMode;

/// Modo actual, como número (0 = Off, 1 = Auto, 2 = Always).
static MODE: AtomicU8 = AtomicU8::new(0);
/// Cada cuánto mira el modo Auto si hay algo a pantalla completa.
const WATCH: std::time::Duration = std::time::Duration::from_secs(2);

/// Aplica un modo. En Auto se lanza un hilo que, cada 2 s, mira si hay
/// algo a pantalla completa (lo mismo que esconde el widget) y pide o suelta
/// los 0,5 ms; el hilo termina en cuanto el modo deja de ser Auto.
pub fn set_mode(mode: TimerMode) {
    let n = mode as u8;
    let old = MODE.swap(n, Ordering::SeqCst);
    match mode {
        TimerMode::Off => set(false),
        TimerMode::Always => set(true),
        TimerMode::Auto => {
            set(crate::overlay::game_running());
            if old != n {
                let _ = std::thread::Builder::new().name("rg-gamewatch".into()).spawn(|| {
                    while MODE.load(Ordering::SeqCst) == TimerMode::Auto as u8 {
                        std::thread::sleep(WATCH);
                        if MODE.load(Ordering::SeqCst) == TimerMode::Auto as u8 {
                            set(crate::overlay::game_running());
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{global_from, ms, Global};

    #[test]
    fn units_to_ms() {
        assert!((ms(156_250) - 15.625).abs() < 1e-4);
        assert!((ms(5000) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn global_states() {
        assert_eq!(global_from(None, false), Global::Off);
        assert_eq!(global_from(None, true), Global::Off);
        assert_eq!(global_from(Some(0), false), Global::Off);
        assert_eq!(global_from(Some(0), true), Global::OffAfterRestart);
        assert_eq!(global_from(Some(1), false), Global::On);
        assert_eq!(global_from(Some(1), true), Global::OnAfterRestart);
        assert!(Global::OnAfterRestart.wanted() && !Global::OffAfterRestart.wanted());
    }

    /// Con la petición puesta, el sistema baja al intervalo más fino.
    #[test]
    fn request_reaches_maximum() {
        let Some(res) = super::query() else { return };
        super::set(true);
        let now = super::query().map_or(0, |r| r.cur);
        super::set(false);
        assert!(!super::active());
        assert!(now <= res.max + 50, "pedido {} y quedó {now}", res.max);
    }
}
