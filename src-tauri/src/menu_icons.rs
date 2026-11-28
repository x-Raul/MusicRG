//! Iconos del menú de la bandeja (`icons/menu/*.svg`, dibujados para MusicRG):
//! solo líneas rectas de 1 px sobre la rejilla de 16 px, para que salgan nítidos.
//!
//! `build.rs` rasteriza cada SVG a 16×16 RGBA en dos tintas; aquí solo se
//! incluyen esos bytes. En ejecución no se decodifica nada.

use tauri::image::Image;
use windows::core::w;

use crate::overlay::{reg_dword, PERSONALIZE};

const PX: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Music,
    Play,
    Next,
    Prev,
    Pin,
    Widget,
    Power,
    Settings,
    Update,
    Show,
    Quit,
}

/// Los bytes que dejó `build.rs` para un SVG, en las dos tintas.
macro_rules! tints {
    ($name:literal) => {
        (
            include_bytes!(concat!(env!("OUT_DIR"), "/menu_", $name, "_dark.rgba")).as_slice(),
            include_bytes!(concat!(env!("OUT_DIR"), "/menu_", $name, "_light.rgba")).as_slice(),
        )
    };
}

impl Icon {
    #[cfg(test)]
    pub const ALL: [Self; 11] = [
        Self::Music,
        Self::Play,
        Self::Next,
        Self::Prev,
        Self::Pin,
        Self::Widget,
        Self::Power,
        Self::Settings,
        Self::Update,
        Self::Show,
        Self::Quit,
    ];

    /// (tinta para menú oscuro, tinta para menú claro)
    fn bytes(self) -> (&'static [u8], &'static [u8]) {
        match self {
            Self::Music => tints!("now"),
            Self::Play => tints!("play"),
            Self::Next => tints!("next"),
            Self::Prev => tints!("prev"),
            Self::Pin => tints!("pin"),
            Self::Widget => tints!("widget"),
            Self::Power => tints!("power"),
            Self::Settings => tints!("settings"),
            Self::Update => tints!("update"),
            Self::Show => tints!("show"),
            Self::Quit => tints!("quit"),
        }
    }
}

pub fn image(icon: Icon, dark_menu: bool) -> Image<'static> {
    let (dark, light) = icon.bytes();
    Image::new(if dark_menu { dark } else { light }, PX, PX)
}

/// ¿Los menús de Windows se ven oscuros? Siguen el modo de las apps; si el
/// valor no existe (Windows antiguo), se asume oscuro, como el resto de MusicRG.
pub fn dark_menu() -> bool {
    reg_dword(PERSONALIZE, w!("AppsUseLightTheme")) != Some(1)
}

#[cfg(test)]
mod tests {
    use super::{image, Icon};

    #[test]
    fn every_icon_is_16px_and_visible() {
        for icon in Icon::ALL {
            for dark in [true, false] {
                let img = image(icon, dark);
                assert_eq!((img.width(), img.height()), (16, 16), "{icon:?}");
                assert_eq!(img.rgba().len(), 16 * 16 * 4, "{icon:?}");
                let opaque = img.rgba().as_chunks::<4>().0.iter().filter(|p| p[3] > 128).count();
                assert!(opaque >= 12, "{icon:?} dark={dark}: solo {opaque} píxeles visibles");
            }
        }
    }

    #[test]
    fn tints_differ_by_theme() {
        let lit = |dark| {
            let img = image(Icon::Play, dark);
            img.rgba().as_chunks::<4>().0.iter().filter(|p| p[3] > 128).map(|p| u32::from(p[0])).max().unwrap_or(0)
        };
        assert!(lit(true) > 200, "el menú oscuro necesita tinta clara");
        assert!(lit(false) < 60, "el menú claro necesita tinta oscura");
    }
}
