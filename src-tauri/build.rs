// En un script de compilación, abortar con un mensaje claro es lo correcto.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

/// Tinta de los iconos del menú: clara para el menú oscuro, oscura para el claro.
const TINTS: [(&str, &str); 2] = [("dark", "#E6E6E6"), ("light", "#1F1F1F")];
/// Windows pinta los iconos de menú siempre a 16×16 (ver `muda`).
const ICON_PX: u32 = 16;

fn main() {
    menu_icons();
    // La página de Apple Music (remota) solo puede llamar a estos comandos;
    // el permiso se concede en capabilities/apple-music.json.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "now_playing",
        "window_ctl",
        "pins",
    ])))
    .expect("tauri-build");
}

/// Rasteriza `icons/menu/*.svg` a RGBA de 16×16 (sin premultiplicar) en
/// `OUT_DIR/menu_<nombre>_<dark|light>.rgba`, que `menu_icons.rs` incluye.
fn menu_icons() {
    let dir = Path::new("icons/menu");
    println!("cargo:rerun-if-changed=icons/menu");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    for entry in std::fs::read_dir(dir).expect("icons/menu") {
        let path = entry.expect("icons/menu").path();
        if path.extension().is_none_or(|e| e != "svg") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).expect("nombre de icono");
        let svg = std::fs::read_to_string(&path).expect("leer SVG");
        for (theme, tint) in TINTS {
            let tree =
                resvg::usvg::Tree::from_str(&svg.replace("currentColor", tint), &resvg::usvg::Options::default())
                    .expect("SVG válido");
            let mut pix = resvg::tiny_skia::Pixmap::new(ICON_PX, ICON_PX).expect("pixmap");
            let k = ICON_PX as f32 / tree.size().width();
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pix.as_mut());
            let rgba: Vec<u8> = pix
                .pixels()
                .iter()
                .flat_map(|p| {
                    let c = p.demultiply();
                    [c.red(), c.green(), c.blue(), c.alpha()]
                })
                .collect();
            std::fs::write(out.join(format!("menu_{name}_{theme}.rgba")), rgba).expect("escribir icono");
        }
    }
}
