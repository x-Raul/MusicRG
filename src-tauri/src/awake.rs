//! "Keep PC awake": mientras está marcada, ni el PC se suspende ni se apaga
//! la pantalla, suene algo o no.
//!
//! Usa una petición de energía de Windows (`PowerCreateRequest`), la misma
//! que usan los reproductores de vídeo. No depende de ningún hilo, Windows la
//! suelta sola si la app se cierra, y se ve con `powercfg /requests`.

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;

    use windows::core::{w, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired,
        PowerSetRequest, POWER_REQUEST_TYPE,
    };
    use windows::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};

    use crate::lock::lock;

    /// Lo que se pide mientras la opción está activa.
    const KINDS: [POWER_REQUEST_TYPE; 2] = [PowerRequestSystemRequired, PowerRequestDisplayRequired];

    /// La petición activa, guardada como número: `HANDLE` no es `Send`.
    static REQUEST: Mutex<Option<isize>> = Mutex::new(None);

    pub fn set(on: bool) {
        let mut cur = lock(&REQUEST);
        match (on, *cur) {
            (true, None) => *cur = create().map(|h| h.0 as isize),
            (false, Some(h)) => {
                release(HANDLE(h as _));
                *cur = None;
            }
            _ => {} // ya está como se pide
        }
    }

    /// ¿Hay una petición activa? (para los tests)
    #[cfg(test)]
    pub fn active() -> bool {
        lock(&REQUEST).is_some()
    }

    fn create() -> Option<HANDLE> {
        let ctx = REASON_CONTEXT {
            Version: 0, // POWER_REQUEST_CONTEXT_VERSION
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                // Windows solo lee el texto; `w!` lo deja en memoria estática.
                SimpleReasonString: PWSTR(w!("MusicRG: Keep PC awake").as_ptr().cast_mut()),
            },
        };
        // SAFETY: `ctx` es válido durante la llamada y Windows copia el texto.
        let h = unsafe { PowerCreateRequest(&raw const ctx) }.ok()?;
        for kind in KINDS {
            // SAFETY: `h` es la petición que se acaba de crear.
            if unsafe { PowerSetRequest(h, kind) }.is_err() {
                release(h);
                return None;
            }
        }
        Some(h)
    }

    fn release(h: HANDLE) {
        for kind in KINDS {
            // SAFETY: `h` es una petición abierta por `create`; quitar un tipo
            // que no estaba puesto solo devuelve error, que se ignora.
            let _ = unsafe { PowerClearRequest(h, kind) };
        }
        // SAFETY: la petición es nuestra y no se vuelve a usar después.
        let _ = unsafe { CloseHandle(h) };
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn set(_: bool) {}
    #[cfg(test)]
    pub fn active() -> bool {
        false
    }
}

pub use imp::set;

#[cfg(test)]
mod tests {
    /// Para comprobarlo a mano: mientras corre, `powercfg /requests` (como
    /// administrador) muestra "MusicRG: Keep PC awake" en DISPLAY y SYSTEM.
    #[test]
    #[ignore = "deja el PC despierto unos segundos"]
    fn request_on_and_off() {
        super::set(true);
        assert!(super::imp::active(), "Windows no aceptó la petición de energía");
        super::set(true); // dos veces no crea otra petición
        std::thread::sleep(std::time::Duration::from_secs(
            std::env::var("MUSICRG_AWAKE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(1),
        ));
        super::set(false);
        assert!(!super::imp::active());
        super::set(false);
    }
}
