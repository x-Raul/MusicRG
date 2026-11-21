fn main() {
    // La página de Apple Music (remota) solo puede llamar a estos comandos;
    // el permiso se concede en capabilities/apple-music.json.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["now_playing", "window_ctl"])),
    )
    .expect("tauri-build");
}
