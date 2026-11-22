//! "Iniciar con Windows": una entrada en `HKCU\...\Run`, sin servicios ni
//! tareas programadas. El registro es la única fuente de verdad, así que si el
//! usuario la quita desde el Administrador de tareas, el menú lo refleja.

/// Argumento con el que Windows arranca MusicRG al iniciar sesión: la ventana
/// no se abre, la app espera en la bandeja.
pub const ARG: &str = "--autostart";

#[cfg(windows)]
mod imp {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const NAME: PCWSTR = w!("MusicRG");

    fn command() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        Some(format!("\"{}\" {}", exe.display(), super::ARG))
    }

    /// Activado solo si la entrada apunta a ESTE ejecutable (una copia vieja
    /// en otra carpeta no cuenta).
    pub fn enabled() -> bool {
        let mut buf = [0u16; 1024];
        let mut n = (buf.len() * 2) as u32;
        let r = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut n))
        };
        if r != ERROR_SUCCESS {
            return false;
        }
        let len = (n as usize / 2).saturating_sub(1);
        let value = String::from_utf16_lossy(&buf[..len]);
        command().is_some_and(|c| c.eq_ignore_ascii_case(&value))
    }

    pub fn set(on: bool) {
        unsafe {
            if on {
                let Some(cmd) = command() else { return };
                let wide = HSTRING::from(cmd);
                let bytes = (wide.len() + 1) * 2;
                let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN, NAME, REG_SZ.0, Some(wide.as_ptr() as *const _), bytes as u32);
            } else {
                let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn enabled() -> bool {
        false
    }
    pub fn set(_: bool) {}
}

pub use imp::{enabled, set};

/// ¿Lo ha arrancado Windows al iniciar sesión?
pub fn launched_at_login() -> bool {
    std::env::args().any(|a| a == ARG)
}
