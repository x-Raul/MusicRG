//! Actualizaciones automáticas desde GitHub Releases.
//!
//! Al arrancar (y luego cada 6 horas) mira `latest.json` del último release.
//! Si hay versión nueva, la entrada del menú de la bandeja pasa a
//! "Install update vX.Y.Z" y el tooltip lo avisa. No instala nada sin que el
//! usuario haga clic: instalar cierra la app y cortaría la música.
//! La firma del instalador se comprueba con la clave pública de tauri.conf.json.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{image::Image, menu::IconMenuItem, AppHandle, Manager, Wry};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::lock::lock;

const CHECK_TEXT: &str = "Check for updates";
const FIRST_CHECK: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_hours(6);

pub struct Updater {
    item: IconMenuItem<Wry>,
    pending: Mutex<Option<Update>>,
    busy: Mutex<bool>,
}

pub fn menu_item(app: &AppHandle, icon: Image<'_>) -> tauri::Result<IconMenuItem<Wry>> {
    IconMenuItem::with_id(app, "update", CHECK_TEXT, true, Some(icon), None::<&str>)
}

/// Guarda el estado y lanza las comprobaciones periódicas en segundo plano.
pub fn start(app: &AppHandle, item: IconMenuItem<Wry>) {
    app.manage(Updater { item, pending: Mutex::new(None), busy: Mutex::new(false) });
    // En desarrollo no se comprueba solo (la versión local no es un release).
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        loop {
            tauri::async_runtime::block_on(check(&app, false));
            std::thread::sleep(EVERY);
        }
    });
}

/// Clic en la entrada del menú: instala si ya hay una versión encontrada,
/// si no, comprueba ahora mostrando el resultado.
pub fn on_click(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let update = lock(&app.state::<Updater>().pending).take();
        match update {
            Some(update) => install(&app, update).await,
            None => check(&app, true).await,
        }
    });
}

async fn check(app: &AppHandle, manual: bool) {
    let state = app.state::<Updater>();
    {
        let mut busy = lock(&state.busy);
        if *busy || lock(&state.pending).is_some() {
            return;
        }
        *busy = true;
    }
    if manual {
        set(&state.item, "Checking for updates…", false);
    }
    let result = match app.updater() {
        Ok(u) => u.check().await,
        Err(e) => Err(e),
    };
    match result {
        Ok(Some(update)) => {
            let label = format!("Install update v{}", update.version);
            set(&state.item, &label, true);
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_tooltip(Some(format!("MusicRG – update v{} available", update.version)));
            }
            *lock(&state.pending) = Some(update);
        }
        Ok(None) => set(&state.item, if manual { "Up to date ✓" } else { CHECK_TEXT }, true),
        Err(e) => {
            eprintln!("[musicrg] update check: {e}");
            set(&state.item, if manual { "Update check failed – retry" } else { CHECK_TEXT }, true);
        }
    }
    *lock(&state.busy) = false;
}

async fn install(app: &AppHandle, update: Update) {
    let state = app.state::<Updater>();
    set(&state.item, &format!("Downloading v{}…", update.version), false);
    // En Windows el instalador (modo "passive") cierra la app, se instala y
    // vuelve a abrirla; restart() es solo por si el instalador no la cierra.
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => app.restart(),
        Err(e) => {
            eprintln!("[musicrg] update install: {e}");
            set(&state.item, &format!("Install update v{} (retry)", update.version), true);
            *lock(&state.pending) = Some(update);
        }
    }
}

fn set(item: &IconMenuItem<Wry>, text: &str, enabled: bool) {
    let _ = item.set_text(text);
    let _ = item.set_enabled(enabled);
}
