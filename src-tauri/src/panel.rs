//! Ventana de ajustes: nativa (Win32 + Direct2D por software), como el widget.
//!
//! No existe hasta que se abre: entonces se lanza un hilo `rg-settings` con
//! su ventana y su bucle de mensajes, y al cerrarla se destruye todo y el
//! hilo termina. Cerrada no ocupa memoria ni CPU; abierta, 1-3 MB.
//!
//! Solo pinta cuando algo cambia. Los interruptores y la aguja del dial se
//! mueven con un seguimiento exponencial a 60 Hz solo mientras se mueven;
//! quietos, el único temporizador es el que lee la resolución cada segundo.
//!
//! El dial es un instrumento de verdad: escala logarítmica en octavas de
//! 16 ms a 0,5 ms (cada marca mayor, la mitad que la anterior) y la aguja
//! marca la resolución que tiene el sistema ahora (`NtQueryTimerResolution`).
//!
//! Sobre `unsafe`: todo lo `unsafe` de este módulo envuelve Win32, Direct2D y
//! DirectWrite, y solo se llama desde el hilo `rg-settings`, con los objetos
//! que posee `Ui`, que viven tanto como la ventana.

use std::cell::RefCell;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN,
    VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::settings::{self, Player, Settings, TimerMode, SCREEN_MINUTES};
use crate::timer::{self, Global, Res};

/// Lo que se puede cambiar desde la ventana.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Widget,
    Others,
    Flat,
    Caps,
    KeepAwake,
    Autostart,
    /// Descargar el reproductor cuando no se usa.
    FreeIdle,
    /// Modo global del timer (registro, con UAC). Lo gestiona la ventana.
    Global,
}

/// Lo que el usuario cambió.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Toggle(Key, bool),
    Timer(TimerMode),
    Player(Player),
    /// Dirección propia ya comprobada (`settings::clean_url`).
    CustomUrl(String),
    /// "Screen care": encendido y minutos sin actividad.
    ScreenCare(bool, u32),
}

/// Valores que la ventana enseña (los da quien la abre).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Values {
    pub prefs: Settings,
    pub autostart: bool,
    pub custom_url: String,
    /// Minutos de "Screen care".
    pub screen_minutes: u32,
}

/// Cómo habla la ventana con el resto de la app. Se llaman en el hilo de la
/// ventana: no deben esperar al hilo principal.
pub struct Hooks {
    pub read: Box<dyn Fn() -> Values + Send>,
    pub write: Box<dyn Fn(Change) + Send>,
}

static HWND_OPEN: AtomicIsize = AtomicIsize::new(0);
const WM_REFRESH: u32 = WM_APP + 1;
/// `wparam` = 1 si el registro se escribió.
const WM_GLOBAL_DONE: u32 = WM_APP + 2;
const T_ANIM: usize = 1;
const T_POLL: usize = 2;

/// Abre la ventana, o la trae al frente si ya está abierta.
pub fn open(hooks: Hooks) {
    let h = HWND_OPEN.load(Ordering::Acquire);
    if h != 0 {
        let hwnd = HWND(h as _);
        // SAFETY: si la ventana ya no existe, estas llamadas solo fallan.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    let _ = std::thread::Builder::new()
        .name("rg-settings".into())
        // SAFETY: este hilo nuevo es el hilo `rg-settings`.
        .spawn(move || unsafe { thread_main(hooks) });
}

/// Algo cambió fuera (menú de la bandeja): la ventana, si está abierta, relee.
pub fn refresh() {
    let h = HWND_OPEN.load(Ordering::Acquire);
    if h != 0 {
        // SAFETY: sin punteros; si la ventana ya no existe solo falla.
        let _ = unsafe { PostMessageW(Some(HWND(h as _)), WM_REFRESH, WPARAM(0), LPARAM(0)) };
    }
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

// ---------------------------------------------------------------------------
// Paleta: la de MusicRG (theme.css). Jerarquía de tonos: fondo y placas
// bajos, estructura media, texto alto y un solo acento encendido.

const BG: u32 = 0x000b_0b0f;
const CARD: u32 = 0x0013_1318;
const CARD_EDGE: u32 = 0x0022_222a;
const HOVER: u32 = 0x001a_1a21;
const RULE: u32 = 0x0020_2028;
const FACE: u32 = 0x000e_0e12;
const BEZEL: u32 = 0x002c_2c35;
const LO: u32 = 0x007e_7e8a;
const FAINT: u32 = 0x003a_3a44;
const MID: u32 = 0x00b4_b4bf;
const HI: u32 = 0x00f2_f2f5;
const ACCENT: u32 = 0x00ff_4d6d;

fn rgb(hex: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn mix(a: u32, b: u32, k: f32) -> D2D1_COLOR_F {
    lerp(rgb(a), rgb(b), k)
}

fn lerp(a: D2D1_COLOR_F, b: D2D1_COLOR_F, k: f32) -> D2D1_COLOR_F {
    let l = |x: f32, y: f32| x + (y - x) * k;
    D2D1_COLOR_F { r: l(a.r, b.r), g: l(a.g, b.g), b: l(a.b, b.b), a: 1.0 }
}

/// COLORREF de DWM: 0x00BBGGRR.
fn colorref(hex: u32) -> u32 {
    (hex & 0xff) << 16 | (hex & 0xff00) | (hex >> 16) & 0xff
}

// ---------------------------------------------------------------------------
// Iconos: rejilla de 16, líneas rectas de 1 px como los del menú.

#[derive(Clone, Copy)]
enum Sh {
    /// Línea (x1, y1, x2, y2).
    L(f32, f32, f32, f32),
    /// Polilínea abierta.
    P(&'static [(f32, f32)]),
    /// Rectángulo de líneas (izq, arriba, der, abajo).
    R(f32, f32, f32, f32),
    /// Rectángulo relleno (x, y, ancho, alto).
    F(f32, f32, f32, f32),
    /// Círculo de línea (cx, cy, radio).
    C(f32, f32, f32),
}
use Sh::{C, F, L, P, R};

const IC_WIDGET: &[Sh] = &[R(1.5, 2.5, 14.5, 13.5), L(1.5, 10.5, 14.5, 10.5), F(10.0, 11.0, 3.0, 2.0)];
const IC_OTHERS: &[Sh] =
    &[R(1.5, 5.5, 10.5, 14.5), P(&[(5.5, 5.5), (5.5, 1.5), (14.5, 1.5), (14.5, 10.5), (10.5, 10.5)])];
const IC_FLAT: &[Sh] = &[
    L(1.5, 14.5, 14.5, 14.5),
    F(2.0, 9.0, 2.0, 5.0),
    F(5.5, 4.0, 2.0, 10.0),
    F(9.0, 7.0, 2.0, 7.0),
    F(12.5, 10.0, 2.0, 4.0),
];
const IC_CAPS: &[Sh] =
    &[R(3.5, 7.5, 12.5, 14.5), P(&[(5.5, 7.5), (5.5, 2.5), (10.5, 2.5), (10.5, 7.5)]), F(7.5, 10.0, 1.0, 2.5)];
const IC_AWAKE: &[Sh] = &[
    R(2.5, 6.5, 10.5, 14.5),
    P(&[(10.5, 8.5), (13.5, 8.5), (13.5, 11.5), (10.5, 11.5)]),
    L(4.5, 1.5, 4.5, 3.5),
    L(6.5, 2.5, 6.5, 4.5),
    L(8.5, 1.5, 8.5, 3.5),
];
const IC_START: &[Sh] = &[
    P(&[(8.5, 1.5), (14.5, 1.5), (14.5, 14.5), (8.5, 14.5)]),
    L(1.5, 8.0, 10.5, 8.0),
    P(&[(7.5, 5.0), (10.5, 8.0), (7.5, 11.0)]),
];
const IC_TIMER: &[Sh] =
    &[C(8.0, 9.5, 5.5), L(6.0, 1.5, 10.0, 1.5), L(8.0, 1.5, 8.0, 4.0), L(8.0, 9.5, 8.0, 6.5), L(12.0, 4.0, 13.5, 2.5)];
const IC_GAMES: &[Sh] = &[
    R(1.5, 4.5, 14.5, 12.5),
    L(3.5, 8.5, 7.5, 8.5),
    L(5.5, 6.5, 5.5, 10.5),
    F(10.0, 6.5, 1.5, 1.5),
    F(12.0, 9.0, 1.5, 1.5),
];

const IC_MEMORY: &[Sh] = &[
    R(1.5, 4.5, 14.5, 11.5),
    F(3.5, 6.5, 3.0, 3.0),
    F(9.5, 6.5, 3.0, 3.0),
    L(3.5, 11.5, 3.5, 13.5),
    L(6.5, 11.5, 6.5, 13.5),
    L(9.5, 11.5, 9.5, 13.5),
    L(12.5, 11.5, 12.5, 13.5),
];
/// Monitor con su pie y una media luna: la pantalla que se duerme.
const IC_SCREEN: &[Sh] = &[
    R(1.5, 2.5, 14.5, 11.5),
    L(8.0, 11.5, 8.0, 14.5),
    L(5.0, 14.5, 11.0, 14.5),
    P(&[(9.5, 4.5), (7.5, 5.0), (6.5, 7.0), (7.5, 9.0), (9.5, 9.5)]),
];
const IC_MUSIC: &[Sh] = &[
    L(5.5, 2.5, 13.5, 2.5),
    L(5.5, 2.5, 5.5, 11.5),
    L(13.5, 2.5, 13.5, 10.5),
    F(2.5, 10.0, 3.5, 3.0),
    F(10.5, 9.0, 3.5, 3.0),
];

// ---------------------------------------------------------------------------
// Contenido y medidas (en DIP: 1 = 1 px a escala 100 %)

const W: f32 = 460.0;
const PAD: f32 = 20.0;
/// Margen de arriba: sin título (ya lo dice la barra de la ventana).
const HEADER: f32 = 4.0;
const SECTION: f32 = 28.0;
const ROW: f32 = 48.0;
const GAUGE: f32 = 124.0;
const GAP: f32 = 10.0;
/// "Advanced settings": una regla fina con el rótulo en medio, sin placa.
const EXPANDER: f32 = 30.0;
/// "Where settings are saved": un enlace pequeño, sin placa.
const LINK: f32 = 24.0;
const NOTE: f32 = 22.0;
const BOTTOM: f32 = 14.0;
const ICON: f32 = 20.0;
const TEXT_X: f32 = PAD + 16.0 + ICON + 14.0;
const SW_W: f32 = 44.0;
const SW_H: f32 = 20.0;
/// Fila con selector de varias opciones (texto arriba, segmentos debajo).
const CHOICE: f32 = 78.0;
const SEG_H: f32 = 26.0;
/// Fila con el campo de la dirección propia.
const FIELD: f32 = 52.0;
const FIELD_H: f32 = 28.0;

/// Partes que se pliegan: empiezan cerradas cada vez que se abre la ventana.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fold {
    /// Widget y System, bajo "Advanced settings".
    Advanced,
    /// Dónde se guardan los ajustes.
    Saved,
}

enum Item {
    Section(&'static str),
    Toggle(Key, &'static str, &'static [Sh]),
    Gauge,
    /// Cabecera que pliega o despliega lo que viene después.
    Expander(Fold, &'static str),
    /// Texto que solo se ve con `Saved` abierto.
    Note,
    /// Selector de varias opciones.
    Choice(Pick, &'static str, &'static [Sh]),
    /// Campo de texto para la dirección propia (solo con "Custom").
    UrlField,
}

/// Posición de "Custom" en el selector de servicio.
const CUSTOM: usize = 3;

/// Qué elige cada selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Timer,
    Player,
    /// "Screen care": apagado o minutos.
    Screen,
}

const PICKS: [Pick; 3] = [Pick::Timer, Pick::Player, Pick::Screen];

impl Pick {
    fn labels(self) -> &'static [&'static str] {
        match self {
            Self::Timer => &["Off", "Auto", "Always"],
            Self::Player => &["Apple", "Spotify", "YT Music", "Custom", "Off"],
            Self::Screen => &["Off", "5 min", "10 min", "15 min", "30 min", "60 min"],
        }
    }
}

/// Lo principal arriba (rendimiento); el resto, plegado.
const ITEMS: &[Item] = &[
    Item::Section("PERFORMANCE"),
    Item::Gauge,
    Item::Choice(Pick::Timer, "Timer resolution 0.5 ms", IC_TIMER),
    Item::Toggle(Key::Global, "Apply to games", IC_GAMES),
    Item::Expander(Fold::Advanced, "Advanced settings"),
    Item::Section("PLAYER"),
    Item::Choice(Pick::Player, "Music service", IC_MUSIC),
    Item::UrlField,
    Item::Toggle(Key::FreeIdle, "Free memory when idle", IC_MEMORY),
    Item::Section("WIDGET"),
    Item::Toggle(Key::Widget, "Taskbar widget", IC_WIDGET),
    Item::Toggle(Key::Others, "Show other apps", IC_OTHERS),
    Item::Toggle(Key::Flat, "Flat audio bars", IC_FLAT),
    Item::Section("SYSTEM"),
    Item::Toggle(Key::Caps, "Caps Lock toast", IC_CAPS),
    Item::Toggle(Key::KeepAwake, "Keep PC awake", IC_AWAKE),
    Item::Choice(Pick::Screen, "Screen care", IC_SCREEN),
    Item::Toggle(Key::Autostart, "Start with Windows", IC_START),
    Item::Expander(Fold::Saved, "Where settings are saved"),
    Item::Note,
];

/// Cuántos interruptores hay (para las animaciones).
const TOGGLES: usize = 8;

/// Qué está desplegado.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Open {
    advanced: bool,
    saved: bool,
    /// El servicio elegido es "Custom" (se ve el campo de la dirección).
    custom: bool,
}

impl Open {
    fn get(self, f: Fold) -> bool {
        match f {
            Fold::Advanced => self.advanced,
            Fold::Saved => self.saved,
        }
    }
}

/// ¿Se ve el elemento `idx`? Los plegables, siempre; lo que va tras uno
/// (hasta el siguiente) solo si está abierto.
fn visible(idx: usize, open: Open) -> bool {
    // "Advanced settings" siempre; los demás plegables, solo con él abierto.
    if matches!(ITEMS[idx], Item::Expander(Fold::Advanced, _)) {
        return true;
    }
    if matches!(ITEMS[idx], Item::Expander(..)) {
        return open.advanced;
    }
    if matches!(ITEMS[idx], Item::UrlField) && !open.custom {
        return false;
    }
    let fold = ITEMS[..idx].iter().rev().find_map(|it| if let Item::Expander(f, _) = it { Some(*f) } else { None });
    // Los plegables van todos dentro de "Advanced settings".
    fold.is_none_or(|f| open.get(f) && (f == Fold::Advanced || open.advanced))
}

/// Posición vertical de cada elemento: (arriba, abajo); los ocultos miden 0.
/// Devuelve también el alto total.
fn layout(open: Open) -> (Vec<(f32, f32)>, f32) {
    let mut y = HEADER;
    let mut out = Vec::with_capacity(ITEMS.len());
    for (i, it) in ITEMS.iter().enumerate() {
        if !visible(i, open) {
            out.push((y, y));
            continue;
        }
        let h = match it {
            Item::Section(_) | Item::Expander(..) => {
                if i > 0 {
                    y += GAP;
                }
                match it {
                    Item::Expander(Fold::Advanced, _) => EXPANDER,
                    Item::Expander(Fold::Saved, _) => LINK,
                    _ => SECTION,
                }
            }
            Item::Toggle(..) => ROW,
            Item::Gauge => GAUGE,
            Item::Note => NOTE,
            Item::Choice(..) => CHOICE,
            Item::UrlField => FIELD,
        };
        out.push((y, y + h));
        y += h;
    }
    (out, y + BOTTOM)
}

/// ¿Se puede pulsar (interruptor, selector o plegable)?
fn interactive(idx: usize) -> bool {
    matches!(ITEMS[idx], Item::Toggle(..) | Item::Expander(..) | Item::Choice(..))
}

/// ¿Va dentro de una placa (fila de una sección)?
fn in_plate(it: &Item) -> bool {
    matches!(it, Item::Toggle(..) | Item::Gauge | Item::Choice(..) | Item::UrlField)
}

/// Caja del selector de una fila que empieza en `t`: (izq, arriba, der, abajo).
fn seg_box(t: f32) -> (f32, f32, f32, f32) {
    (TEXT_X, t + 45.0, W - PAD - 16.0, t + 45.0 + SEG_H)
}

/// Segmento bajo `x` en un selector de `n` opciones.
fn seg_at(x: f32, n: usize) -> Option<usize> {
    let (l, _, r, _) = seg_box(0.0);
    (l..r).contains(&x).then(|| (((x - l) / ((r - l) / n as f32)) as usize).min(n - 1))
}

/// Caja del campo de texto de una fila que empieza en `t`.
fn field_box(t: f32) -> (f32, f32, f32, f32) {
    (TEXT_X, t + 4.0, W - PAD - 16.0, t + 4.0 + FIELD_H)
}

/// Posición del interruptor `idx` entre los interruptores (para `anim`).
fn slot(idx: usize) -> Option<usize> {
    matches!(ITEMS[idx], Item::Toggle(..))
        .then(|| ITEMS[..idx].iter().filter(|it| matches!(it, Item::Toggle(..))).count())
}

fn subtitle(key: Key, ui: &Ui) -> String {
    match key {
        Key::Widget => "Now playing above the taskbar".into(),
        Key::Others => "Browser, Spotify\u{2026} when MusicRG is idle".into(),
        Key::Flat => "Bars rest on a baseline".into(),
        Key::Caps => "Shows when Caps Lock turns on or off".into(),
        // Screen care sigue apagando la pantalla aunque esto esté activo.
        Key::KeepAwake if ui.vals.prefs.screen_care => "No sleep; Screen care still turns the screen off".into(),
        Key::KeepAwake => "No sleep, the screen stays on".into(),
        Key::Autostart => "Only the widget and tray at sign-in".into(),
        Key::FreeIdle => "Unload the player after 30 min closed and silent".into(),
        Key::Global => {
            if ui.global_busy {
                "Waiting for administrator approval\u{2026}".into()
            } else if ui.global_failed {
                "Not changed: administrator approval needed".into()
            } else {
                match ui.global {
                    Global::Off => "Off: only MusicRG gets it (admin)".into(),
                    Global::On => "On: every app and game shares it".into(),
                    Global::OnAfterRestart => "On after you restart Windows".into(),
                    Global::OffAfterRestart => "Off after you restart Windows".into(),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------

struct Fmts {
    title: IDWriteTextFormat,
    sub: IDWriteTextFormat,
    section: IDWriteTextFormat,
    big: IDWriteTextFormat,
    unit: IDWriteTextFormat,
    small: IDWriteTextFormat,
    small_r: IDWriteTextFormat,
    tiny: IDWriteTextFormat,
    /// Texto de los segmentos, centrado.
    seg: IDWriteTextFormat,
}

#[allow(clippy::struct_excessive_bools)]
struct Ui {
    hwnd: HWND,
    hooks: Hooks,
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    square: ID2D1StrokeStyle,
    fmts: Fmts,
    dpi: u32,
    rows: Vec<(f32, f32)>,
    open: Open,
    vals: Values,
    global: Global,
    global_busy: bool,
    global_failed: bool,
    res: Option<Res>,
    /// 0 = apagado, 1 = encendido, por interruptor (orden de `ITEMS`).
    anim: [f32; TOGGLES],
    /// Ángulo actual de la aguja (radianes).
    needle: f32,
    /// Por plegable (`Fold as usize`): 0 = plegado, 1 = abierto (gira la flecha).
    fold_anim: [f32; 2],
    /// Por plegable: 0 = en reposo, 1 = ratón o foco encima (la línea se
    /// enciende y corre).
    glow: [f32; 2],
    /// Por interruptor: 0 = en reposo, 1 = ratón encima.
    sw_hover: [f32; TOGGLES],
    /// Por selector (`Pick as usize`): posición del carro, en segmentos.
    seg_pos: [f32; 3],
    /// Por selector: 0 = en "Off", 1 = encendido (testigo del carro).
    seg_lit: [f32; 3],
    /// Índices de `ITEMS` (interruptores y plegables).
    hover: Option<usize>,
    pressed: Option<usize>,
    focus: usize,
    /// El foco se dibuja solo si se usa el teclado.
    keyboard: bool,
    still: bool,
    animating: bool,
    /// Cuándo se dio el último paso de animación: los pasos miden el tiempo
    /// real (el temporizador de 16 ms llega a menudo cada 31 ms).
    last_step: std::time::Instant,
    tracking: bool,
    /// Desplazamiento vertical del contenido (DIP).
    scroll: f32,
    /// Última x del ratón (DIP), para el segmento bajo el cursor.
    mouse_x: f32,
    /// Campo de texto nativo (EDIT) para la dirección propia.
    edit: HWND,
    edit_font: HFONT,
    edit_brush: HBRUSH,
    /// La dirección escrita no vale (no es `https://`).
    url_error: bool,
}

/// # Safety
/// Solo desde el hilo `rg-settings`.
unsafe fn thread_main(hooks: Hooks) {
    let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
    if let Some(hwnd) = create_window() {
        if let Some(ui) = Ui::new(hwnd, hooks) {
            UI.with(|u| *u.borrow_mut() = Some(ui));
            HWND_OPEN.store(hwnd.0 as isize, Ordering::Release);
            with_ui(|ui| ui.place());
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            SetTimer(Some(hwnd), T_POLL, 1000, None);
            let mut msg = MSG::default();
            while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
                // Enter en el campo de la dirección: se aplica (el EDIT de una
                // línea no avisa de Enter por sí solo).
                if msg.message == WM_KEYDOWN && msg.wParam.0 == usize::from(VK_RETURN.0) {
                    let applied = with_ui(|ui| {
                        (msg.hwnd == ui.edit).then(|| {
                            ui.apply_url();
                        })
                    });
                    if applied.flatten().is_some() {
                        continue;
                    }
                }
                let _ = TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        } else {
            #[cfg(debug_assertions)]
            eprintln!("[musicrg] settings: Direct2D/DirectWrite setup failed");
            let _ = DestroyWindow(hwnd);
        }
    } else {
        #[cfg(debug_assertions)]
        eprintln!("[musicrg] settings: window creation failed");
    }
    HWND_OPEN.store(0, Ordering::Release);
    // Suelta Direct2D/DirectWrite antes de cerrar COM.
    UI.with(|u| drop(u.borrow_mut().take()));
    if com {
        CoUninitialize();
    }
}

const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

unsafe fn create_window() -> Option<HWND> {
    let hinst = GetModuleHandleW(None).ok()?;
    let class = w!("MusicRGSettings");
    // El icono de la app (recurso 32512, el que pone tauri-build).
    let icon = LoadIconW(Some(hinst.into()), PCWSTR(32512 as _)).unwrap_or_default();
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst.into(),
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hIcon: icon,
        hIconSm: icon,
        ..Default::default()
    };
    RegisterClassExW(&raw const wc); // la segunda vez falla y vale la primera
                                     // Se crea escondida en el monitor del ratón (el de la bandeja) y `place`
                                     // la mide y la centra con el DPI de ese monitor.
    let mut pt = POINT::default();
    let _ = GetCursorPos(&raw mut pt);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        class,
        w!("MusicRG Settings"),
        STYLE,
        pt.x,
        pt.y,
        1,
        1,
        None,
        None,
        Some(hinst.into()),
        None,
    )
    .ok()?;
    let on = BOOL(1);
    let attr = |a, v: *const std::ffi::c_void, n: usize| {
        let _ = DwmSetWindowAttribute(hwnd, a, v, n as u32);
    };
    attr(DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const on).cast(), 4);
    // Barra de título del color de la ventana (Windows 11).
    for (a, c) in [(DWMWA_CAPTION_COLOR, BG), (DWMWA_BORDER_COLOR, CARD_EDGE), (DWMWA_TEXT_COLOR, MID)] {
        let c = colorref(c);
        attr(a, (&raw const c).cast(), 4);
    }
    Some(hwnd)
}

fn reduced_motion() -> bool {
    let mut on = BOOL(1);
    // SAFETY: `on` es local y del tamaño que pide la acción.
    let _ = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    !on.as_bool()
}

impl Ui {
    unsafe fn new(hwnd: HWND, hooks: Hooks) -> Option<Self> {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            // Por software: ni GPU ni los DLL de Direct3D en memoria.
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
            ..Default::default()
        };
        let rt = d2d.CreateDCRenderTarget(&raw const props).ok()?;
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
        let brush = rt.CreateSolidColorBrush(&rgb(HI), None).ok()?;
        let square = d2d
            .CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_SQUARE,
                    endCap: D2D1_CAP_STYLE_SQUARE,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_MITER,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )
            .ok()?;
        let fmts = Fmts::new(&dw)?;
        let vals = (hooks.read)();
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            &HSTRING::from(vals.custom_url.as_str()),
            WINDOW_STYLE(WS_CHILD.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32),
            0,
            0,
            1,
            1,
            Some(hwnd),
            None,
            GetModuleHandleW(None).ok().map(Into::into),
            None,
        )
        .ok()?;
        let _ = SendMessageW(edit, 0x00C5 /* EM_LIMITTEXT */, Some(WPARAM(2000)), None);
        let edit_brush = CreateSolidBrush(COLORREF(colorref(FACE)));
        let (rows, _) = layout(Open::default());
        let mut ui = Self {
            hwnd,
            hooks,
            dw,
            rt,
            brush,
            square,
            fmts,
            dpi: GetDpiForWindow(hwnd).max(96),
            rows,
            open: Open { custom: vals.prefs.player == Player::Custom, ..Open::default() },
            vals,
            global: timer::global(),
            global_busy: false,
            global_failed: false,
            res: timer::query(),
            anim: [0.0; TOGGLES],
            needle: 0.0,
            fold_anim: [0.0; 2],
            glow: [0.0; 2],
            sw_hover: [0.0; TOGGLES],
            seg_pos: [0.0; 3],
            seg_lit: [0.0; 3],
            hover: None,
            pressed: None,
            focus: (0..ITEMS.len()).find(|&i| interactive(i)).unwrap_or(0),
            keyboard: false,
            still: reduced_motion(),
            animating: false,
            last_step: std::time::Instant::now(),
            tracking: false,
            mouse_x: 0.0,
            scroll: 0.0,
            edit,
            edit_font: HFONT::default(),
            edit_brush,
            url_error: false,
        };
        // Arranca ya en su sitio: sin animación al abrir.
        for (i, k) in toggle_keys().enumerate() {
            ui.anim[i] = f32::from(u8::from(ui.value(k)));
        }
        ui.needle = ui.needle_target();
        ui.snap();
        ui.rows = layout(ui.open).0;
        ui.set_edit_font();
        Some(ui)
    }

    fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    /// Tamaño y posición: centrada en la zona de trabajo de su monitor.
    unsafe fn place(&mut self) {
        let (w, h) = self.outer_size();
        let work = self.work_area();
        let x = work.left + ((work.right - work.left) - w).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - h).max(0) / 2;
        let _ = SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }

    /// Tamaño exterior (con la barra de título) para lo que hay desplegado.
    /// Si no cabe en la pantalla, se queda en el alto de la pantalla y el
    /// contenido se desplaza con la rueda.
    unsafe fn outer_size(&self) -> (i32, i32) {
        let (_, h) = layout(self.open);
        let s = self.scale();
        let mut r = RECT { left: 0, top: 0, right: (W * s).round() as i32, bottom: (h * s).round() as i32 };
        let _ = AdjustWindowRectExForDpi(&raw mut r, STYLE, false, WINDOW_EX_STYLE::default(), self.dpi);
        let work = self.work_area();
        let max_h = work.bottom - work.top;
        let h = if max_h > 0 { (r.bottom - r.top).min(max_h) } else { r.bottom - r.top };
        (r.right - r.left, h)
    }

    /// Cuánto se puede desplazar el contenido (DIP): lo que no cabe.
    unsafe fn max_scroll(&self) -> f32 {
        let mut cr = RECT::default();
        let _ = GetClientRect(self.hwnd, &raw mut cr);
        let visible_h = (cr.bottom - cr.top) as f32 / self.scale();
        (layout(self.open).1 - visible_h).max(0.0)
    }

    unsafe fn scroll_by(&mut self, dy: f32) {
        let s = (self.scroll + dy).clamp(0.0, self.max_scroll());
        if (s - self.scroll).abs() > 0.01 {
            self.scroll = s;
            self.place_edit();
            self.invalidate();
        }
    }

    unsafe fn work_area(&self) -> RECT {
        let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &raw mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT::default()
        }
    }

    /// Pliega o despliega: la ventana crece o encoge hacia abajo y, si se
    /// saldría de la pantalla, sube lo justo.
    unsafe fn fold(&mut self, f: Fold) {
        match f {
            Fold::Advanced => self.open.advanced = !self.open.advanced,
            Fold::Saved => self.open.saved = !self.open.saved,
        }
        self.relayout();
        self.animate();
    }

    /// Recalcula las filas y ajusta el alto de la ventana (crece hacia abajo;
    /// si se saldría de la pantalla, sube lo justo).
    unsafe fn relayout(&mut self) {
        self.rows = layout(self.open).0;
        let (w, h) = self.outer_size();
        let mut r = RECT::default();
        let _ = GetWindowRect(self.hwnd, &raw mut r);
        let work = self.work_area();
        let top = r.top.min(work.bottom - h).max(work.top);
        let _ = SetWindowPos(self.hwnd, None, r.left, top, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        self.scroll = self.scroll.min(self.max_scroll());
        self.place_edit();
        self.invalidate();
    }

    fn value(&self, k: Key) -> bool {
        let p = self.vals.prefs;
        match k {
            Key::Widget => p.widget,
            Key::Others => p.others,
            Key::Flat => p.flat,
            Key::Caps => p.caps,
            Key::KeepAwake => p.keep_awake,
            Key::Autostart => self.vals.autostart,
            Key::FreeIdle => p.free_idle,
            Key::Global => self.global.wanted(),
        }
    }

    fn set_value(&mut self, k: Key, on: bool) {
        let p = &mut self.vals.prefs;
        match k {
            Key::Widget => p.widget = on,
            Key::Others => p.others = on,
            Key::Flat => p.flat = on,
            Key::Caps => p.caps = on,
            Key::KeepAwake => p.keep_awake = on,
            Key::Autostart => self.vals.autostart = on,
            Key::FreeIdle => p.free_idle = on,
            Key::Global => {}
        }
    }

    /// Ángulo de la aguja para la resolución actual.
    fn needle_target(&self) -> f32 {
        let cur = self.res.map_or(156_250, |r| r.cur);
        dial_angle(timer::ms(cur))
    }

    /// Pulsar el elemento `idx` de `ITEMS`. `x` (en DIP) dice qué segmento
    /// de un selector; sin él, el selector pasa a la opción siguiente.
    unsafe fn activate(&mut self, idx: usize, x: Option<f32>) {
        let k = match ITEMS[idx] {
            Item::Toggle(k, ..) => k,
            Item::Expander(f, _) => return self.fold(f),
            Item::Choice(pick, ..) => {
                let n = pick.labels().len();
                let i = match x {
                    Some(x) => match seg_at(x, n) {
                        Some(i) => i,
                        None => return,
                    },
                    None => (self.choice(pick) + 1) % n,
                };
                return self.choose(pick, i);
            }
            _ => return,
        };
        if k == Key::Global {
            self.toggle_global();
            return;
        }
        let on = !self.value(k);
        self.set_value(k, on);
        (self.hooks.write)(Change::Toggle(k, on));
        self.animate();
    }

    /// Opción marcada en un selector.
    fn choice(&self, pick: Pick) -> usize {
        match pick {
            Pick::Timer => TimerMode::ALL.iter().position(|m| *m == self.vals.prefs.timer).unwrap_or(0),
            // "Custom" se marca en cuanto se elige, aunque falte la dirección.
            Pick::Player if self.open.custom => CUSTOM,
            Pick::Player => Player::ALL.iter().position(|p| *p == self.vals.prefs.player).unwrap_or(0),
            Pick::Screen if !self.vals.prefs.screen_care => 0,
            // Minutos puestos a mano en el JSON: se marca el más cercano.
            Pick::Screen => settings::nearest_screen_minutes(self.vals.screen_minutes) + 1,
        }
    }

    unsafe fn choose(&mut self, pick: Pick, i: usize) {
        if i == self.choice(pick) {
            return;
        }
        match pick {
            Pick::Timer => {
                let mode = TimerMode::ALL[i];
                self.vals.prefs.timer = mode;
                (self.hooks.write)(Change::Timer(mode));
                // La petición se aplica al momento; la aguja la sigue ya.
                self.res = timer::query();
            }
            Pick::Player => {
                let p = Player::ALL[i];
                self.open.custom = p == Player::Custom;
                self.url_error = false;
                // "Custom" sin dirección aún: solo se enseña el campo; se
                // aplica al escribirla.
                if p != Player::Custom || !self.vals.custom_url.is_empty() {
                    self.vals.prefs.player = p;
                    (self.hooks.write)(Change::Player(p));
                }
                self.relayout();
                if p == Player::Custom {
                    let _ = SetFocus(Some(self.edit));
                }
            }
            Pick::Screen => {
                let on = i > 0;
                if on {
                    self.vals.screen_minutes = SCREEN_MINUTES[i - 1];
                }
                self.vals.prefs.screen_care = on;
                (self.hooks.write)(Change::ScreenCare(on, self.vals.screen_minutes));
            }
        }
        self.animate();
    }

    /// Lee el campo de la dirección y, si vale y cambió, la aplica.
    unsafe fn apply_url(&mut self) {
        let mut buf = [0u16; 2048];
        let n = GetWindowTextW(self.edit, &mut buf).max(0) as usize;
        let text = String::from_utf16_lossy(&buf[..n]);
        if text.trim().is_empty() {
            return;
        }
        match settings::clean_url(&text) {
            Some(url) => {
                self.url_error = false;
                let _ = SetWindowTextW(self.edit, &HSTRING::from(url.as_str()));
                if url != self.vals.custom_url || self.vals.prefs.player != Player::Custom {
                    self.vals.custom_url.clone_from(&url);
                    self.vals.prefs.player = Player::Custom;
                    (self.hooks.write)(Change::CustomUrl(url));
                }
            }
            None => self.url_error = true,
        }
        self.invalidate();
    }

    /// Fuente del campo de texto al DPI actual (la del resto de la ventana).
    unsafe fn set_edit_font(&mut self) {
        let px = -((13.0 * self.scale()).round() as i32);
        let font = CreateFontW(
            px,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Segoe UI"),
        );
        let _ = SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        if !self.edit_font.is_invalid() {
            let _ = DeleteObject(self.edit_font.into());
        }
        self.edit_font = font;
    }

    /// Coloca (o esconde) el campo de texto nativo sobre su fila.
    unsafe fn place_edit(&self) {
        let Some(idx) = ITEMS.iter().position(|it| matches!(it, Item::UrlField)) else { return };
        if !visible(idx, self.open) {
            let _ = ShowWindow(self.edit, SW_HIDE);
            return;
        }
        let s = self.scale();
        let (l, t, r, b) = field_box(self.rows[idx].0 - self.scroll);
        // El texto va dentro del marco que se dibuja, con su margen.
        let (x, y) = (((l + 8.0) * s).round() as i32, ((t + 6.0) * s).round() as i32);
        let (w, h) = (((r - l - 16.0) * s).round() as i32, ((b - t - 10.0) * s).round() as i32);
        let _ = SetWindowPos(self.edit, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = ShowWindow(self.edit, SW_SHOW);
    }

    /// El modo global pide UAC: se hace en otro hilo para no congelar la
    /// ventana, que se entera al terminar con `WM_GLOBAL_DONE`.
    unsafe fn toggle_global(&mut self) {
        if self.global_busy {
            return;
        }
        let want = !self.global.wanted();
        self.global_busy = true;
        self.global_failed = false;
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let ok = timer::set_global(want);
            // SAFETY: sin punteros; si la ventana ya se cerró solo falla.
            let _ = unsafe { PostMessageW(Some(HWND(hwnd as _)), WM_GLOBAL_DONE, WPARAM(usize::from(ok)), LPARAM(0)) };
        });
        self.invalidate();
    }

    unsafe fn invalidate(&self) {
        let _ = InvalidateRect(Some(self.hwnd), None, false);
    }

    /// Pone en marcha el temporizador de animación si hace falta.
    unsafe fn animate(&mut self) {
        if self.still {
            self.snap();
        } else if !self.animating {
            self.animating = true;
            self.last_step = std::time::Instant::now();
            SetTimer(Some(self.hwnd), T_ANIM, 16, None);
        }
        self.invalidate();
    }

    fn snap(&mut self) {
        for (i, k) in toggle_keys().enumerate() {
            self.anim[i] = f32::from(u8::from(self.value(k)));
        }
        self.needle = self.needle_target();
        for f in FOLDS {
            self.fold_anim[f as usize] = self.fold_target(f);
            self.glow[f as usize] = self.glow_target(f);
        }
        for (i, idx) in toggle_idxs().enumerate() {
            self.sw_hover[i] = self.lit(idx);
        }
        for pick in PICKS {
            self.seg_pos[pick as usize] = self.choice(pick) as f32;
            self.seg_lit[pick as usize] = self.choice_lit(pick);
        }
    }

    /// ¿Está el selector en algo distinto de "Off"? (1 o 0)
    fn choice_lit(&self, pick: Pick) -> f32 {
        let cur = self.choice(pick);
        let on = match pick {
            Pick::Timer | Pick::Screen => cur != 0,
            Pick::Player => cur != Player::ALL.len() - 1,
        };
        f32::from(u8::from(on))
    }

    fn fold_target(&self, f: Fold) -> f32 {
        f32::from(u8::from(self.open.get(f)))
    }

    /// ¿Tiene el elemento `idx` el ratón o el foco de teclado encima?
    fn lit(&self, idx: usize) -> f32 {
        let on = self.hover == Some(idx) || (self.keyboard && self.focus == idx && self.is_active());
        f32::from(u8::from(on))
    }

    /// La línea de un plegable se enciende con el ratón o el foco.
    fn glow_target(&self, f: Fold) -> f32 {
        self.lit(fold_idx(f))
    }

    /// Un paso de `dt` segundos hacia el destino. Devuelve `true` si ya
    /// llegó todo.
    fn step(&mut self, dt: f32) -> bool {
        // Seguimiento exponencial: ~90 % del camino en 120 ms.
        let k = 1.0 - (-dt / 0.052).exp();
        let mut done = true;
        let keys: Vec<Key> = toggle_keys().collect();
        for (i, key) in keys.into_iter().enumerate() {
            let t = f32::from(u8::from(self.value(key)));
            let d = t - self.anim[i];
            if d.abs() < 0.002 {
                self.anim[i] = t;
            } else {
                self.anim[i] += d * k;
                done = false;
            }
        }
        let t = self.needle_target();
        let d = t - self.needle;
        if d.abs() < 0.001 {
            self.needle = t;
        } else {
            self.needle += d * k * 0.7;
            done = false;
        }
        // La flecha gira algo más despacio que los interruptores; la regla,
        // más despacio aún, para que se vea correr hacia los bordes.
        let mut goals: Vec<(f32, f32)> = Vec::new();
        for f in FOLDS {
            goals.push((self.fold_target(f), 0.8));
            goals.push((self.glow_target(f), 0.45));
        }
        for idx in toggle_idxs() {
            goals.push((self.lit(idx), 0.9));
        }
        for pick in PICKS {
            goals.push((self.choice(pick) as f32, 1.3));
            goals.push((self.choice_lit(pick), 0.6));
        }
        let [f0, f1] = &mut self.fold_anim;
        let [g0, g1] = &mut self.glow;
        let [p0, p1, p2] = &mut self.seg_pos;
        let [l0, l1, l2] = &mut self.seg_lit;
        let vals = [f0, g0, f1, g1].into_iter().chain(self.sw_hover.iter_mut()).chain([p0, l0, p1, l1, p2, l2]);
        for (v, (t, rate)) in vals.zip(goals) {
            let d = t - *v;
            if d.abs() < 0.002 {
                *v = t;
            } else {
                *v += d * k * rate;
                done = false;
            }
        }
        done
    }

    /// Elemento pulsable (índice de `ITEMS`) bajo un punto en DIP.
    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        if !(PAD..=W - PAD).contains(&x) {
            return None;
        }
        (0..ITEMS.len()).find(|&i| {
            let (t, b) = self.rows[i];
            interactive(i) && visible(i, self.open) && (t..b).contains(&y)
        })
    }

    /// Siguiente (o anterior) elemento pulsable visible, en círculo.
    fn next_focus(&self, back: bool) -> usize {
        let n = ITEMS.len();
        let mut i = self.focus;
        for _ in 0..n {
            i = if back { (i + n - 1) % n } else { (i + 1) % n };
            if interactive(i) && visible(i, self.open) {
                return i;
            }
        }
        self.focus
    }

    fn dip(&self, lp: LPARAM) -> (f32, f32) {
        let x = f32::from((lp.0 & 0xffff) as u16 as i16);
        let y = f32::from(((lp.0 >> 16) & 0xffff) as u16 as i16);
        (x / self.scale(), y / self.scale() + self.scroll)
    }

    #[allow(clippy::too_many_lines)]
    unsafe fn on_msg(&mut self, m: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match m {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(self.hwnd, &raw mut ps);
                self.paint(dc);
                let _ = EndPaint(self.hwnd, &raw const ps);
            }
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_TIMER => match wp.0 {
                T_ANIM => {
                    let now = std::time::Instant::now();
                    let dt = now.duration_since(self.last_step).as_secs_f32().clamp(0.004, 0.05);
                    self.last_step = now;
                    if self.step(dt) {
                        let _ = KillTimer(Some(self.hwnd), T_ANIM);
                        self.animating = false;
                    }
                    self.invalidate();
                }
                T_POLL => {
                    let res = timer::query();
                    if res != self.res {
                        self.res = res;
                        self.animate();
                    }
                }
                _ => {}
            },
            WM_REFRESH => {
                self.vals = (self.hooks.read)();
                self.res = timer::query();
                self.animate();
            }
            WM_COMMAND => {
                // El campo de la dirección perdió el foco: se aplica.
                const EN_SETFOCUS: u32 = 0x0100;
                const EN_KILLFOCUS: u32 = 0x0200;
                if lp.0 == self.edit.0 as isize {
                    match (wp.0 >> 16) as u32 {
                        EN_KILLFOCUS => self.apply_url(),
                        EN_SETFOCUS => self.invalidate(),
                        _ => {}
                    }
                }
            }
            WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as _);
                SetTextColor(dc, COLORREF(colorref(HI)));
                SetBkColor(dc, COLORREF(colorref(FACE)));
                return Some(LRESULT(self.edit_brush.0 as isize));
            }
            WM_GLOBAL_DONE => {
                self.global_busy = false;
                self.global_failed = wp.0 == 0;
                self.global = timer::global();
                self.animate();
            }
            WM_MOUSEMOVE => {
                let (x, y) = self.dip(lp);
                let hover = self.hit(x, y);
                let on_choice = hover.is_some_and(|i| matches!(ITEMS[i], Item::Choice(..)));
                if hover != self.hover || (on_choice && (x - self.mouse_x).abs() > 0.5) {
                    self.hover = hover;
                    self.animate();
                }
                self.mouse_x = x;
                if !self.tracking {
                    self.tracking = true;
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&raw mut t);
                }
            }
            0x02A3 /* WM_MOUSELEAVE */ => {
                self.tracking = false;
                if self.hover.take().is_some() {
                    self.animate();
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = self.dip(lp);
                self.pressed = self.hit(x, y);
                self.keyboard = false;
                // Clic fuera del campo de texto: le quita el foco (y se aplica).
                let _ = SetFocus(Some(self.hwnd));
                SetCapture(self.hwnd);
                self.invalidate();
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let (x, y) = self.dip(lp);
                if let Some(i) = self.pressed.take() {
                    if self.hit(x, y) == Some(i) {
                        self.focus = i;
                        self.activate(i, Some(x));
                    }
                }
                self.invalidate();
            }
            WM_KEYDOWN => {
                let vk = wp.0 as u16;
                let back = GetKeyState(i32::from(VK_SHIFT.0)) < 0;
                match vk {
                    v if v == VK_ESCAPE.0 => {
                        let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    v if v == VK_SPACE.0 || v == VK_RETURN.0 => {
                        self.keyboard = true;
                        self.activate(self.focus, None);
                    }
                    v if v == VK_LEFT.0 || v == VK_RIGHT.0 => {
                        if let Item::Choice(pick, ..) = ITEMS[self.focus] {
                            self.keyboard = true;
                            let n = pick.labels().len();
                            let cur = self.choice(pick);
                            let i = if v == VK_LEFT.0 { cur.saturating_sub(1) } else { (cur + 1).min(n - 1) };
                            self.choose(pick, i);
                        }
                    }
                    v if v == VK_UP.0 || (v == VK_TAB.0 && back) => {
                        self.keyboard = true;
                        self.focus = self.next_focus(true);
                        self.animate();
                    }
                    v if v == VK_DOWN.0 || v == VK_TAB.0 => {
                        self.keyboard = true;
                        self.focus = self.next_focus(false);
                        self.animate();
                    }
                    _ => {}
                }
            }
            WM_DPICHANGED => {
                self.dpi = u32::from((wp.0 & 0xffff) as u16).max(96);
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                self.set_edit_font();
                self.place_edit();
                self.invalidate();
            }
            WM_SETTINGCHANGE => {
                self.still = reduced_motion();
            }
            WM_ACTIVATE => self.animate(),
            WM_MOUSEWHEEL => {
                let delta = f32::from(((wp.0 >> 16) & 0xffff) as u16 as i16);
                self.scroll_by(-delta / 120.0 * 56.0);
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        // SAFETY: la fuente y la brocha son de esta ventana, que ya no las usa.
        unsafe {
            if !self.edit_font.is_invalid() {
                let _ = DeleteObject(self.edit_font.into());
            }
            let _ = DeleteObject(self.edit_brush.into());
        }
    }
}

const FOLDS: [Fold; 2] = [Fold::Advanced, Fold::Saved];

/// Índice del plegable `f` en `ITEMS`.
fn fold_idx(f: Fold) -> usize {
    ITEMS.iter().position(|it| matches!(it, Item::Expander(g, _) if *g == f)).unwrap_or(0)
}

/// Índices de `ITEMS` de los interruptores, en orden.
fn toggle_idxs() -> impl Iterator<Item = usize> {
    (0..ITEMS.len()).filter(|&i| matches!(ITEMS[i], Item::Toggle(..)))
}

fn toggle_keys() -> impl Iterator<Item = Key> {
    ITEMS.iter().filter_map(|it| if let Item::Toggle(k, ..) = it { Some(*k) } else { None })
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if m == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    if let Some(r) = with_ui(|ui| if ui.hwnd == h { ui.on_msg(m, wp, lp) } else { None }).flatten() {
        return r;
    }
    DefWindowProcW(h, m, wp, lp)
}

// ---------------------------------------------------------------------------
// El dial: escala logarítmica de 16 ms a 0,5 ms (cinco octavas) en 240°.

const DIAL_FROM: f32 = 16.0;
const OCTAVES: f32 = 5.0;
const SWEEP: f32 = 240.0 * PI / 180.0;
/// Empieza abajo a la izquierda (210° en sentido horario desde las 3).
const START: f32 = 150.0 * PI / 180.0;

/// Ángulo (en coordenadas de pantalla: y hacia abajo) para `ms` milisegundos.
fn dial_angle(ms: f32) -> f32 {
    let t = ((DIAL_FROM / ms.max(0.01)).log2() / OCTAVES).clamp(0.0, 1.0);
    START + SWEEP * t
}

// ---------------------------------------------------------------------------
// Dibujo

impl Fmts {
    unsafe fn new(dw: &IDWriteFactory) -> Option<Self> {
        let mut coll = None;
        dw.GetSystemFontCollection(&raw mut coll, false).ok()?;
        let has = |name: &HSTRING| {
            let (mut i, mut ok) = (0u32, BOOL(0));
            coll.as_ref().is_some_and(|c| c.FindFamilyName(name, &raw mut i, &raw mut ok).is_ok() && ok.as_bool())
        };
        let ui = {
            let v = HSTRING::from("Segoe UI Variable Text");
            if has(&v) {
                v
            } else {
                HSTRING::from("Segoe UI")
            }
        };
        let f = |size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> Option<IDWriteTextFormat> {
            let fmt = dw
                .CreateTextFormat(
                    &ui,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
                .ok()?;
            let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = fmt.SetTextAlignment(align);
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
            if let Ok(sign) = dw.CreateEllipsisTrimmingSign(&fmt) {
                let trim = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = fmt.SetTrimming(&raw const trim, &sign);
            }
            Some(fmt)
        };
        let (l, r, c) = (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_ALIGNMENT_CENTER);
        Some(Self {
            title: f(14.0, DWRITE_FONT_WEIGHT_NORMAL, l)?,
            sub: f(12.0, DWRITE_FONT_WEIGHT_NORMAL, l)?,
            section: f(11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, l)?,
            big: f(30.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, l)?,
            unit: f(13.0, DWRITE_FONT_WEIGHT_NORMAL, l)?,
            small: f(12.0, DWRITE_FONT_WEIGHT_NORMAL, l)?,
            small_r: f(12.0, DWRITE_FONT_WEIGHT_NORMAL, r)?,
            tiny: f(8.5, DWRITE_FONT_WEIGHT_NORMAL, c)?,
            seg: f(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, c)?,
        })
    }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

fn rrect(l: f32, t: f32, r: f32, b: f32, rad: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(l, t, r, b), radiusX: rad, radiusY: rad }
}

fn circle(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE {
    D2D1_ELLIPSE { point: pt(x, y), radiusX: r, radiusY: r }
}

impl Ui {
    unsafe fn color(&self, c: D2D1_COLOR_F) -> &ID2D1SolidColorBrush {
        self.brush.SetColor(&raw const c);
        &self.brush
    }

    unsafe fn text(&self, s: &str, fmt: &IDWriteTextFormat, x: f32, y: f32, w: f32, c: u32) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let b = self.color(rgb(c));
        self.rt.DrawText(
            &wide,
            fmt,
            &rect(x, y, x + w, y + 60.0),
            b,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    unsafe fn text_width(&self, s: &str, fmt: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let Ok(l) = self.dw.CreateTextLayout(&wide, fmt, 1000.0, 100.0) else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = l.GetMetrics(&raw mut m);
        m.widthIncludingTrailingWhitespace
    }

    unsafe fn line(&self, a: Vector2, b: Vector2, w: f32, c: D2D1_COLOR_F) {
        let br = self.color(c);
        self.rt.DrawLine(a, b, br, w, None);
    }

    unsafe fn icon(&self, shapes: &[Sh], x: f32, y: f32, c: u32) {
        let k = ICON / 16.0;
        let p = |a: f32, b: f32| pt(x + a * k, y + b * k);
        let sw = 1.15;
        let b = self.color(rgb(c));
        for sh in shapes {
            match *sh {
                L(x1, y1, x2, y2) => self.rt.DrawLine(p(x1, y1), p(x2, y2), b, sw, &self.square),
                P(pts) => {
                    for w in pts.windows(2) {
                        self.rt.DrawLine(p(w[0].0, w[0].1), p(w[1].0, w[1].1), b, sw, &self.square);
                    }
                }
                R(l, t, r, bt) => {
                    let (a, z) = (p(l, t), p(r, bt));
                    self.rt.DrawRectangle(&rect(a.X, a.Y, z.X, z.Y), b, sw, &self.square);
                }
                F(fx, fy, fw, fh) => {
                    let (a, z) = (p(fx, fy), p(fx + fw, fy + fh));
                    self.rt.FillRectangle(&rect(a.X, a.Y, z.X, z.Y), b);
                }
                C(cx, cy, r) => {
                    let c = p(cx, cy);
                    self.rt.DrawEllipse(&circle(c.X, c.Y, r * k), b, sw, None);
                }
            }
        }
    }

    unsafe fn paint(&mut self, dc: HDC) {
        let mut cr = RECT::default();
        let _ = GetClientRect(self.hwnd, &raw mut cr);
        if self.rt.BindDC(dc, &raw const cr).is_err() {
            return;
        }
        let dpi = self.dpi as f32;
        self.rt.SetDpi(dpi, dpi);
        self.rt.BeginDraw();
        self.rt.Clear(Some(&rgb(BG)));
        self.rt.SetTransform(&Matrix3x2::translation(0.0, -self.scroll));
        self.paint_items();
        self.rt.SetTransform(&Matrix3x2::identity());
        self.paint_scrollbar(&cr);
        let _ = self.rt.EndDraw(None, None);
    }

    /// Indicador fino a la derecha, solo si hay algo que desplazar.
    unsafe fn paint_scrollbar(&self, cr: &RECT) {
        let max = self.max_scroll();
        if max <= 0.0 {
            return;
        }
        let view = (cr.bottom - cr.top) as f32 / self.scale();
        let total = view + max;
        let h = (view * view / total).max(24.0);
        let y = (view - h) * self.scroll / max;
        let bar = rrect(W - 6.0, y + 2.0, W - 3.0, y + h - 2.0, 1.5);
        self.rt.FillRoundedRectangle(&raw const bar, self.color(rgb(FAINT)));
    }

    unsafe fn paint_items(&self) {
        // Primero las placas: cada sección, de su primera a su última fila.
        for (i, it) in ITEMS.iter().enumerate() {
            if !matches!(it, Item::Section(_)) || !visible(i, self.open) {
                continue;
            }
            let rows = i + 1..ITEMS.len();
            let last = rows.take_while(|&j| in_plate(&ITEMS[j])).last().unwrap_or(i);
            let (t, b) = (self.rows[i + 1].0, self.rows[last].1);
            self.plate(t, b);
        }
        for (idx, it) in ITEMS.iter().enumerate() {
            if !visible(idx, self.open) {
                continue;
            }
            let (t, b) = self.rows[idx];
            match it {
                Item::Section(name) => self.paint_section(name, t, b),
                Item::Toggle(key, title, icon) => {
                    // Divisoria con la fila de encima de la misma placa: va
                    // del texto al borde de la placa.
                    self.divider(idx, t);
                    self.paint_toggle(idx, *key, title, icon, t, b);
                }
                Item::Choice(pick, title, icon) => {
                    self.divider(idx, t);
                    self.paint_choice(idx, *pick, title, icon, t, b);
                }
                Item::UrlField => self.paint_field(t),
                Item::Gauge => self.paint_gauge(t, b),
                Item::Expander(f, title) => self.paint_expander(idx, *f, title, t, b),
                Item::Note => self.paint_note(t),
            }
        }
    }

    /// Divisoria con la fila de encima de la misma placa: va del texto al
    /// borde de la placa.
    unsafe fn divider(&self, idx: usize, t: f32) {
        if idx > 0 && in_plate(&ITEMS[idx - 1]) && !matches!(ITEMS[idx - 1], Item::UrlField) {
            self.line(pt(TEXT_X, t + 0.5), pt(W - PAD - 1.0, t + 0.5), 1.0, rgb(RULE));
        }
    }

    /// Selector: título y explicación arriba, segmentos debajo. El elegido,
    /// en el acento; el que tiene el ratón encima, con la tinta más alta.
    unsafe fn paint_choice(&self, idx: usize, pick: Pick, title: &str, icon: &[Sh], t: f32, b: f32) {
        let hovered = self.row_state(idx, t, b, 4.0);
        let cur = self.choice(pick);
        let lit = self.seg_lit[pick as usize];
        self.icon(icon, PAD + 16.0, (t + 12.0).round(), if lit > 0.5 { HI } else { MID });
        let tw = W - TEXT_X - PAD - 16.0;
        self.text(title, &self.fmts.title, TEXT_X, t + 6.0, tw, HI);
        self.text(&self.choice_note(pick, cur), &self.fmts.sub, TEXT_X, t + 25.0, tw, LO);
        let labels = pick.labels();
        let n = labels.len();
        let (l, st, r, sb) = seg_box(t);
        let h = if hovered { 1.0 } else { 0.0 };
        self.slot(l, st, r, sb, mix(BEZEL, MID, 0.35 * h));
        let sw = (r - l) / n as f32;
        // Marcas de regla en cada límite entre segmentos, colgando del borde
        // de abajo de la ranura (las tapa el carro al pasar).
        for i in 1..n {
            let x = (l + sw * i as f32).round() + 0.5;
            self.line(pt(x, sb - 3.0), pt(x, sb - 7.0), 1.0, rgb(BEZEL));
            self.line(pt(x, st + 3.0), pt(x, st + 5.0), 1.0, rgb(BEZEL));
        }
        // Carro: se desliza de segmento en segmento.
        let pos = self.seg_pos[pick as usize];
        let pressed = hovered && self.pressed == Some(idx);
        let (cl, cr) = (l + sw * pos + 2.0, l + sw * (pos + 1.0) - 2.0);
        let ct = st + 2.0 + if pressed { 0.5 } else { 0.0 };
        self.carriage(cl, ct, cr, sb - 2.0 + if pressed { 0.5 } else { 0.0 }, 0.0, 3.5);
        // Testigo: una raya de acento al pie del carro, encendida salvo en "Off".
        let ly = (sb - 5.0).round() + 0.5;
        self.line(pt(cl + 8.0, ly), pt(cr - 8.0, ly), 1.25, mix(BEZEL, ACCENT, lit));
        let over = if hovered { seg_at(self.mouse_x, n) } else { None };
        for (i, label) in labels.iter().enumerate() {
            let x = l + sw * i as f32;
            // Bajo el carro, la tinta alta; fuera, media (alta con el ratón).
            let under = (1.0 - (pos - i as f32).abs()).clamp(0.0, 1.0);
            let base = if over == Some(i) { rgb(HI) } else { mix(LO, MID, 0.7) };
            let c = lerp(base, rgb(HI), under);
            let wide: Vec<u16> = label.encode_utf16().collect();
            self.rt.DrawText(
                &wide,
                &self.fmts.seg,
                &rect(x, st + 4.0, x + sw, sb),
                self.color(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// Ranura fresada: cara hundida, sombra arriba, luz abajo y su canto.
    unsafe fn slot(&self, l: f32, t: f32, r: f32, b: f32, edge_c: D2D1_COLOR_F) {
        let face = rrect(l, t, r, b, 4.0);
        self.rt.FillRoundedRectangle(&raw const face, self.color(rgb(FACE)));
        self.line(pt(l + 3.0, t + 1.5), pt(r - 3.0, t + 1.5), 1.0, rgb(BG));
        self.line(pt(l + 3.0, b - 1.5), pt(r - 3.0, b - 1.5), 1.0, mix(FACE, CARD_EDGE, 0.7));
        let edge = rrect(l + 0.5, t + 0.5, r - 0.5, b - 0.5, 3.5);
        self.rt.DrawRoundedRectangle(&raw const edge, self.color(edge_c), 1.0, None);
    }

    /// Carro de máquina: sombra de contacto, canto inferior oscuro, cara
    /// superior más clara, borde y bisel de luz. `h` = ratón encima.
    unsafe fn carriage(&self, l: f32, t: f32, r: f32, b: f32, h: f32, rad: f32) {
        let shadow = rrect(l - 0.5, t + 0.5, r + 0.5, b + 1.0, rad);
        self.rt.FillRoundedRectangle(&raw const shadow, self.color(rgb(BG)));
        let body = rrect(l, t, r, b, rad);
        self.rt.FillRoundedRectangle(&raw const body, self.color(mix(FACE, BEZEL, 0.9)));
        let top = rrect(l, t, r, b - 2.5, rad);
        self.rt.FillRoundedRectangle(&raw const top, self.color(mix(BEZEL, LO, 0.35 + 0.25 * h)));
        let rim = rrect(l + 0.5, t + 0.5, r - 0.5, b - 0.5, rad - 0.5);
        self.rt.DrawRoundedRectangle(&raw const rim, self.color(mix(FAINT, MID, 0.4 * h)), 1.0, None);
        self.line(pt(l + rad, t + 1.5), pt(r - rad, t + 1.5), 1.0, mix(LO, MID, 0.5 + 0.5 * h));
    }

    fn choice_note(&self, pick: Pick, cur: usize) -> String {
        match (pick, cur) {
            (Pick::Timer, 0) => "Windows decides (up to 15.6 ms)".into(),
            (Pick::Timer, 1) => "0.5 ms only while a game is full screen".into(),
            (Pick::Timer, _) => "0.5 ms all the time, a little more power".into(),
            (Pick::Player, CUSTOM) => "Any https:// music site; type it below".into(),
            (Pick::Player, 4) => "No player: no browser processes at all".into(),
            (Pick::Player, _) => "The player reloads when you change it".into(),
            (Pick::Screen, 0) => "Turn the screen off when you step away".into(),
            (Pick::Screen, _) => {
                format!("Screen off after {} min idle; music keeps playing", self.vals.screen_minutes)
            }
        }
    }

    /// Marco del campo de la dirección (el texto lo pinta el EDIT nativo).
    unsafe fn paint_field(&self, t: f32) {
        let (l, ft, r, fb) = field_box(t);
        let focused = GetFocus() == self.edit;
        let edge_c = if self.url_error {
            ACCENT
        } else if focused {
            MID
        } else {
            CARD_EDGE
        };
        let edge_c = if edge_c == CARD_EDGE { BEZEL } else { edge_c };
        self.slot(l, ft, r, fb, rgb(edge_c));
        let (note, c) = if self.url_error {
            ("Use an https:// address, e.g. https://tidal.com", ACCENT)
        } else if self.vals.custom_url.is_empty() {
            ("Press Enter to open it", LO)
        } else {
            ("Press Enter to apply changes", LO)
        };
        self.text(note, &self.fmts.sub, l + 2.0, fb + 2.0, r - l, c);
        if self.vals.custom_url.is_empty() && !focused && GetWindowTextLengthW(self.edit) == 0 {
            self.text("https://", &self.fmts.sub, l + 9.0, ft + 6.0, r - l - 16.0, FAINT);
        }
    }

    unsafe fn plate(&self, t: f32, b: f32) {
        let card = rrect(PAD, t, W - PAD, b, 8.0);
        self.rt.FillRoundedRectangle(&raw const card, self.color(rgb(CARD)));
        let edge = rrect(PAD + 0.5, t + 0.5, W - PAD - 0.5, b - 0.5, 7.5);
        self.rt.DrawRoundedRectangle(&raw const edge, self.color(rgb(CARD_EDGE)), 1.0, None);
        // Luz en el canto de arriba y un tornillo en cada esquina.
        self.line(pt(PAD + 8.0, t + 1.5), pt(W - PAD - 8.0, t + 1.5), 1.0, mix(CARD, CARD_EDGE, 0.6));
        for (x, y) in [(PAD + 6.0, t + 6.0), (W - PAD - 6.0, t + 6.0), (PAD + 6.0, b - 6.0), (W - PAD - 6.0, b - 6.0)] {
            self.rt.FillEllipse(&circle(x, y, 1.3), self.color(rgb(CARD_EDGE)));
            self.rt.FillEllipse(&circle(x, y, 0.5), self.color(rgb(BEZEL)));
        }
    }

    /// Fondo de fila al pasar el ratón o pulsar, y el anillo de foco.
    unsafe fn row_state(&self, idx: usize, t: f32, b: f32, inset: f32) -> bool {
        let hovered = self.hover == Some(idx);
        if hovered {
            let pressed = self.pressed == Some(idx);
            let r = rrect(PAD + inset, t + inset, W - PAD - inset, b - inset, 6.0);
            let c = if pressed { mix(HOVER, CARD_EDGE, 0.6) } else { rgb(HOVER) };
            self.rt.FillRoundedRectangle(&raw const r, self.color(c));
        }
        if self.keyboard && self.focus == idx && self.is_active() {
            let r = rrect(PAD + inset - 1.0, t + inset - 1.0, W - PAD - inset + 1.0, b - inset + 1.0, 6.0);
            self.rt.DrawRoundedRectangle(&raw const r, self.color(rgb(HI)), 1.5, None);
        }
        hovered
    }

    /// Flecha de 1 px girada `a` radianes (0 = hacia abajo).
    unsafe fn chevron_at(&self, cx: f32, cy: f32, a: f32, c: D2D1_COLOR_F) {
        let (ca, sa) = (a.cos(), a.sin());
        let rot = |x: f32, y: f32| pt(cx + x * ca - y * sa, cy + x * sa + y * ca);
        let pts = [rot(-3.5, -1.75), rot(0.0, 1.75), rot(3.5, -1.75)];
        let b = self.color(c);
        self.rt.DrawLine(pts[0], pts[1], b, 1.25, None);
        self.rt.DrawLine(pts[1], pts[2], b, 1.25, None);
    }

    unsafe fn paint_expander(&self, idx: usize, f: Fold, title: &str, t: f32, b: f32) {
        let mid = f32::midpoint(t, b);
        match f {
            Fold::Advanced => self.paint_fold_rule(idx, title, mid),
            Fold::Saved => self.paint_fold_link(idx, title, mid),
        }
    }

    /// "Where settings are saved" como un enlace pequeño: `› Where settings…`.
    /// Con el ratón encima, un subrayado de 1 px corre de izquierda a
    /// derecha; al abrir, la flecha gira un cuarto de vuelta y una línea en
    /// escuadra baja hasta la ruta, que aparece fundiéndose.
    unsafe fn paint_fold_link(&self, idx: usize, title: &str, mid: f32) {
        let i = Fold::Saved as usize;
        let (g, f) = (self.glow[i], self.fold_anim[i]);
        let pressed = self.pressed == Some(idx) && self.hover == Some(idx);
        let ink = if pressed { rgb(HI) } else { mix(LO, MID, g.max(f * 0.6)) };
        let cx = PAD + 8.0;
        // De "derecha" (-90°) a "abajo" (0°).
        self.chevron_at(cx, mid + 0.5, -PI / 2.0 * (1.0 - f), ink);
        let x = PAD + 18.0;
        let tw = self.text_width(title, &self.fmts.sub);
        let wide: Vec<u16> = title.encode_utf16().collect();
        self.rt.DrawText(
            &wide,
            &self.fmts.sub,
            &rect(x, mid - 8.5, x + tw + 1.0, mid + 12.0),
            self.color(ink),
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        let uy = (mid + 8.0).round() + 0.5;
        if g > 0.01 {
            let e = 1.0 - (1.0 - g) * (1.0 - g);
            self.line(pt(x, uy), pt(x + tw * e, uy), 1.0, mix(BG, LO, g));
        }
        // La versión, al pie y a la derecha (antes iba en la cabecera).
        let v = concat!("v", env!("CARGO_PKG_VERSION"));
        self.text(v, &self.fmts.small_r, W - PAD - 120.0, mid - 8.0, 120.0, FAINT);
        if self.keyboard && self.focus == idx && self.is_active() {
            let r = rrect(PAD - 2.0, mid - 10.0, x + tw + 8.0, mid + 10.0, 5.0);
            self.rt.DrawRoundedRectangle(&raw const r, self.color(rgb(HI)), 1.5, None);
        }
    }

    /// Ruta de los ajustes, colgando del enlace por una escuadra de 1 px.
    unsafe fn paint_note(&self, t: f32) {
        let f = self.fold_anim[Fold::Saved as usize];
        let (lx, x) = (PAD + 8.5, PAD + 18.0);
        let y = (t + 9.0).round() + 0.5;
        let c = mix(BG, FAINT, f);
        // Baja primero y luego corre hacia la ruta.
        let down = (f * 2.0).min(1.0);
        let across = (f * 2.0 - 1.0).clamp(0.0, 1.0);
        self.line(pt(lx, t - 4.0), pt(lx, t - 4.0 + (y - t + 4.0) * down), 1.0, c);
        if across > 0.0 {
            self.line(pt(lx, y), pt(lx + (x - 4.0 - lx) * across, y), 1.0, c);
        }
        let s = r"%APPDATA%\MusicRG\settings.json";
        let wide: Vec<u16> = s.encode_utf16().collect();
        self.rt.DrawText(
            &wide,
            &self.fmts.sub,
            &rect(x, t + 1.0, W - PAD, t + 20.0),
            self.color(mix(BG, MID, f)),
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    /// "Advanced settings" como una regla fina: `── Advanced settings ⌄ ──`.
    /// Con el ratón encima, un trazo más claro sale del rótulo y corre hacia
    /// los dos bordes; al abrir, la flecha gira media vuelta y los extremos
    /// de la regla se doblan hacia abajo, abrazando lo que se despliega.
    unsafe fn paint_fold_rule(&self, idx: usize, title: &str, mid: f32) {
        let g = self.glow[Fold::Advanced as usize];
        let f = self.fold_anim[Fold::Advanced as usize];
        let pressed = self.pressed == Some(idx) && self.hover == Some(idx);
        let tw = self.text_width(title, &self.fmts.sub);
        let (chev, gap, pad) = (8.0, 7.0, 12.0);
        let group = tw + gap + chev;
        let gl = (W - group) / 2.0;
        let gr = gl + group;
        let y = mid.round() + 0.5;
        let ink = if pressed { rgb(HI) } else { mix(LO, HI, g) };

        // Regla de base, partida por el rótulo.
        let (l0, r0) = (PAD + 1.0, W - PAD - 1.0);
        let (la, rb) = (gl - pad, gr + pad);
        self.line(pt(l0, y), pt(la, y), 1.0, rgb(RULE));
        self.line(pt(rb, y), pt(r0, y), 1.0, rgb(RULE));
        // Trazo encendido: crece del rótulo hacia fuera.
        if g > 0.01 {
            let e = 1.0 - (1.0 - g) * (1.0 - g);
            let c = mix(RULE, MID, 0.55 * g);
            self.line(pt(la - (la - l0) * e, y), pt(la, y), 1.0, c);
            self.line(pt(rb, y), pt(rb + (r0 - rb) * e, y), 1.0, c);
        }
        // Abierto: los extremos bajan como corchetes (6 px).
        if f > 0.01 {
            let d = 6.0 * f;
            let c = mix(RULE, LO, 0.35 * f);
            self.line(pt(l0, y), pt(l0, y + d), 1.0, c);
            self.line(pt(r0, y), pt(r0, y + d), 1.0, c);
            // Un punto de acento en cada extremo del rótulo.
            let a = mix(BG, ACCENT, f);
            self.rt.FillEllipse(&circle(la, y, 1.4 * f), self.color(a));
            self.rt.FillEllipse(&circle(rb, y, 1.4 * f), self.color(a));
        }

        // Rótulo y flecha (gira de "abajo" a "arriba").
        let wide: Vec<u16> = title.encode_utf16().collect();
        self.rt.DrawText(
            &wide,
            &self.fmts.sub,
            &rect(gl, mid - 8.5, gl + tw + 1.0, mid + 12.0),
            self.color(ink),
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        self.chevron_at(gr - chev / 2.0, mid + 0.5, PI * f, ink);

        // Foco de teclado: anillo ceñido al rótulo.
        if self.keyboard && self.focus == idx && self.is_active() {
            let r = rrect(gl - 8.0, mid - 10.0, gr + 8.0, mid + 10.0, 5.0);
            self.rt.DrawRoundedRectangle(&raw const r, self.color(rgb(HI)), 1.5, None);
        }
    }

    unsafe fn paint_section(&self, name: &str, t: f32, b: f32) {
        let y = b - 21.0;
        self.text(name, &self.fmts.section, PAD + 2.0, y, 200.0, LO);
        // Regla grabada que sale del rótulo y muere en el borde de la placa.
        let x = PAD + 2.0 + self.text_width(name, &self.fmts.section) + 10.0;
        let ry = (y + 8.0).round() + 0.5;
        self.line(pt(x, ry), pt(W - PAD, ry), 1.0, rgb(RULE));
        let _ = t;
    }

    unsafe fn paint_toggle(&self, idx: usize, key: Key, title: &str, icon: &[Sh], t: f32, b: f32) {
        let busy = key == Key::Global && self.global_busy;
        let hovered = self.row_state(idx, t, b, 4.0);
        let p = slot(idx).map_or(0.0, |i| self.anim[i]);
        let mid = f32::midpoint(t, b);
        // El icono se enciende con su interruptor (tinta media -> alta).
        self.icon(icon, PAD + 16.0, (mid - ICON / 2.0).round(), if p > 0.5 { HI } else { MID });
        self.text(title, &self.fmts.title, TEXT_X, t + 6.0, W - TEXT_X - PAD - SW_W - 28.0, HI);
        let sub = subtitle(key, self);
        let sub_c = if key == Key::Global && self.global_failed { ACCENT } else { LO };
        self.text(&sub, &self.fmts.sub, TEXT_X, t + 25.0, W - TEXT_X - PAD - SW_W - 28.0, sub_c);
        let h = slot(idx).map_or(0.0, |i| self.sw_hover[i]);
        let pressed = hovered && self.pressed == Some(idx);
        self.paint_switch(W - PAD - 16.0 - SW_W, mid - SW_H / 2.0, p, h, pressed, busy);
    }

    /// Interruptor dibujado como una pieza de máquina: una ranura fresada
    /// con su regla de marcas y dos tornillos en los extremos, y un carro
    /// moleteado que se desliza por ella. Lo único encendido es el testigo
    /// del carro y la línea de la ranura que deja atrás (el acento). Con el
    /// ratón encima (`h`) se aclaran el moleteado y los cantos; al pulsar,
    /// el carro se hunde medio píxel en la ranura.
    unsafe fn paint_switch(&self, x: f32, y: f32, p: f32, h: f32, pressed: bool, busy: bool) {
        let (r, b) = (x + SW_W, y + SW_H);
        let cy = y + SW_H / 2.0;
        // Ranura: cara hundida con sombra arriba y luz abajo.
        self.slot(x, y, r, b, mix(BEZEL, MID, 0.35 * h));
        // Regla: una marca cada 4 px, mayores en los extremos y en medio.
        let (t0, t1) = (x + 6.0, r - 6.0);
        let n = ((t1 - t0) / 4.0).round() as usize;
        for k in 0..=n {
            let tx = (t0 + (t1 - t0) * k as f32 / n as f32).round() + 0.5;
            let major = k == 0 || k == n || k == n / 2;
            let len = if major { 4.0 } else { 2.0 };
            self.line(pt(tx, b - 4.0 - len), pt(tx, b - 4.0), 1.0, rgb(if major { FAINT } else { BEZEL }));
        }
        // Tornillos de los extremos.
        for sx in [x + 3.0, r - 3.0] {
            self.rt.FillEllipse(&circle(sx, y + 4.0, 0.9), self.color(rgb(BEZEL)));
        }
        // Carro: 18 x 16, con cara superior, canto inferior y moleteado.
        let (cw, ch) = (18.0, SW_H - 4.0);
        let travel = SW_W - 4.0 - cw;
        let e = p * p * (3.0 - 2.0 * p);
        let cl = x + 2.0 + travel * p;
        let ct = y + 2.0 + if pressed { 0.5 } else { 0.0 };
        // Línea encendida: de la izquierda de la ranura hasta el carro.
        if e > 0.01 {
            let ly = cy.round() + 0.5;
            self.line(pt(x + 4.0, ly), pt(cl, ly), 1.25, mix(FACE, ACCENT, e));
        }
        // Halo del carro sobre la ranura (sombra de contacto).
        let shadow = rrect(cl - 0.5, ct + 0.5, cl + cw + 0.5, ct + ch + 1.0, 3.0);
        self.rt.FillRoundedRectangle(&raw const shadow, self.color(rgb(BG)));
        let body_c = if busy { rgb(BEZEL) } else { mix(BEZEL, LO, 0.35 + 0.25 * h) };
        let body = rrect(cl, ct, cl + cw, ct + ch, 3.0);
        self.rt.FillRoundedRectangle(&raw const body, self.color(mix(FACE, BEZEL, 0.9)));
        // Cara superior (más clara) sobre el canto inferior (más oscuro).
        let top = rrect(cl, ct, cl + cw, ct + ch - 2.5, 3.0);
        self.rt.FillRoundedRectangle(&raw const top, self.color(body_c));
        let rim = rrect(cl + 0.5, ct + 0.5, cl + cw - 0.5, ct + ch - 0.5, 2.5);
        self.rt.DrawRoundedRectangle(&raw const rim, self.color(mix(FAINT, MID, 0.4 * h)), 1.0, None);
        // Bisel: luz en el canto de arriba.
        self.line(pt(cl + 3.0, ct + 1.5), pt(cl + cw - 3.0, ct + 1.5), 1.0, mix(LO, MID, 0.5 + 0.5 * h));
        // Moleteado: cinco rayas que mueren en el bisel y en el canto.
        let kc = mix(BEZEL, FACE, 0.5);
        for k in 0..5 {
            let kx = (cl + 5.0 + 2.0 * k as f32).round() + 0.5;
            self.line(pt(kx, ct + 4.0), pt(kx, ct + ch - 5.0), 1.0, kc);
        }
        // Testigo: apagado, un punto gris; encendido, el acento.
        let lx = cl + cw - 3.5;
        let lamp = circle(lx, ct + ch / 2.0 - 1.0, 1.6);
        self.rt.FillEllipse(&raw const lamp, self.color(mix(FACE, ACCENT, e)));
        self.rt.DrawEllipse(&raw const lamp, self.color(mix(BEZEL, ACCENT, e)), 0.75, None);
    }

    fn is_active(&self) -> bool {
        // SAFETY: consulta sin efectos.
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    /// El instrumento: dial con escala de octavas, aguja, tornillos en las
    /// esquinas de su placa y las lecturas a la derecha.
    unsafe fn paint_gauge(&self, t: f32, b: f32) {
        let r = 50.0;
        let (cx, cy) = (PAD + 18.0 + r, f32::midpoint(t, b) + 4.0);
        // Placa del dial con sus cuatro tornillos.
        let plate = rrect(cx - r - 8.0, cy - r - 12.0, cx + r + 8.0, cy + r + 4.0, 6.0);
        self.rt.FillRoundedRectangle(&raw const plate, self.color(mix(CARD, BG, 0.45)));
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let (x, y) = (cx + sx * (r + 3.0), if sy < 0.0 { cy - r - 7.0 } else { cy + r - 1.0 });
            self.rt.FillEllipse(&circle(x, y, 1.6), self.color(rgb(BEZEL)));
            self.rt.FillEllipse(&circle(x, y, 0.6), self.color(rgb(FAINT)));
        }
        // Bisel y esfera.
        self.rt.FillEllipse(&circle(cx, cy, r), self.color(rgb(BEZEL)));
        self.rt.FillEllipse(&circle(cx, cy, r - 2.5), self.color(rgb(FACE)));
        self.rt.DrawEllipse(&circle(cx, cy, r - 4.5), self.color(rgb(CARD_EDGE)), 0.75, None);
        // Marcas: cada octava (mayor) partida en cuartos de octava.
        let labels = ["16", "8", "4", "2", "1", ".5"];
        let steps = (OCTAVES as usize) * 4;
        for s in 0..=steps {
            let a = START + SWEEP * s as f32 / steps as f32;
            let (ca, sa) = (a.cos(), a.sin());
            let major = s % 4 == 0;
            let last = s == steps;
            let (r0, r1) = if major { (r - 13.0, r - 6.0) } else { (r - 10.0, r - 6.0) };
            let c = if last {
                rgb(HI)
            } else if major {
                rgb(MID)
            } else {
                rgb(LO)
            };
            let w = if major { 1.4 } else { 0.9 };
            self.line(pt(cx + ca * r0, cy + sa * r0), pt(cx + ca * r1, cy + sa * r1), w, c);
            if major {
                let lr = r - 21.0;
                let (lx, ly) = (cx + ca * lr, cy + sa * lr);
                self.text(labels[s / 4], &self.fmts.tiny, lx - 9.0, ly - 6.0, 18.0, if last { HI } else { LO });
            }
        }
        // Arco de la escala, de la primera a la última marca.
        self.paint_arc(cx, cy, r - 6.0, START, START + SWEEP, rgb(FAINT));
        // Tramo encendido: de 16 ms a la lectura actual.
        if self.needle > START + 0.01 {
            self.paint_arc(cx, cy, r - 6.0, START, self.needle, rgb(ACCENT));
        }
        self.text("ms", &self.fmts.tiny, cx - 9.0, cy + 16.0, 18.0, LO);
        // Aguja con cola, eje y tornillo.
        let (ca, sa) = (self.needle.cos(), self.needle.sin());
        self.line(pt(cx - ca * 9.0, cy - sa * 9.0), pt(cx + ca * (r - 8.0), cy + sa * (r - 8.0)), 1.6, rgb(ACCENT));
        self.rt.FillEllipse(&circle(cx, cy, 4.2), self.color(rgb(BEZEL)));
        self.rt.DrawEllipse(&circle(cx, cy, 4.2), self.color(rgb(ACCENT)), 1.0, None);
        self.rt.FillEllipse(&circle(cx, cy, 1.2), self.color(rgb(HI)));

        // Lecturas.
        let x = cx + r + 26.0;
        let wr = W - PAD - 16.0 - x;
        let (lamp, lamp_c) =
            if timer::active() { ("MusicRG request on", ACCENT) } else { ("No request from MusicRG", FAINT) };
        self.rt.FillEllipse(&circle(x + 3.0, t + 21.0, 3.0), self.color(rgb(lamp_c)));
        self.text(lamp, &self.fmts.sub, x + 12.0, t + 13.0, wr - 12.0, LO);
        let (cur, min, max) = self
            .res
            .map_or(("—".into(), "—".into(), "—".into()), |r| (fmt_ms(r.cur), fmt_ms(r.min), fmt_ms(r.max)));
        self.text(&cur, &self.fmts.big, x - 1.0, t + 28.0, wr, HI);
        let cw = self.text_width(&cur, &self.fmts.big);
        self.text("ms current", &self.fmts.unit, x + cw + 5.0, t + 44.0, wr - cw - 5.0, LO);
        for (k, (name, v)) in [("Minimum", &min), ("Maximum", &max)].into_iter().enumerate() {
            let y = t + 76.0 + 22.0 * k as f32;
            self.line(pt(x, y - 3.5), pt(W - PAD - 1.0, y - 3.5), 1.0, rgb(RULE));
            self.text(name, &self.fmts.small, x, y, wr, LO);
            self.text(&format!("{v} ms"), &self.fmts.small_r, x, y, wr, MID);
        }
    }

    unsafe fn paint_arc(&self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, c: D2D1_COLOR_F) {
        // Segmentos cortos: barato y sin geometrías que crear.
        let n = (((a1 - a0) / (PI / 60.0)).ceil() as usize).max(1);
        let b = self.color(c);
        let mut prev = pt(cx + a0.cos() * r, cy + a0.sin() * r);
        for s in 1..=n {
            let a = a0 + (a1 - a0) * s as f32 / n as f32;
            let p = pt(cx + a.cos() * r, cy + a.sin() * r);
            self.rt.DrawLine(prev, p, b, 1.0, None);
            prev = p;
        }
    }
}

fn fmt_ms(units: u32) -> String {
    format!("{:.3}", timer::ms(units))
}

#[cfg(test)]
mod tests {
    use super::{
        dial_angle, layout, slot, toggle_keys, visible, Item, Key, Open, ITEMS, OCTAVES, START, SWEEP, TOGGLES,
    };

    #[test]
    fn dial_is_logarithmic_in_octaves() {
        let a = |ms| dial_angle(ms);
        assert!((a(16.0) - START).abs() < 1e-5);
        assert!((a(0.5) - (START + SWEEP)).abs() < 1e-5);
        // Cada mitad del intervalo avanza lo mismo.
        let step = SWEEP / OCTAVES;
        assert!((a(1.0) - a(2.0) - step).abs() < 1e-4);
        assert!((a(4.0) - a(8.0) - step).abs() < 1e-4);
        // 15,625 ms cae justo después de la primera marca, nunca fuera.
        assert!(a(15.625) > START && a(15.625) < START + step / 4.0);
        assert!((a(0.1) - (START + SWEEP)).abs() < 1e-5);
    }

    #[test]
    fn every_toggle_has_a_slot() {
        assert_eq!(toggle_keys().count(), TOGGLES);
        assert_eq!(toggle_keys().filter(|k| *k == Key::Global).count(), 1);
        for (i, it) in ITEMS.iter().enumerate() {
            assert_eq!(slot(i).is_some(), matches!(it, Item::Toggle(..)));
        }
        assert_eq!((0..ITEMS.len()).filter_map(slot).max(), Some(TOGGLES - 1));
        // Todo desplegado puede no caber en pantallas bajas: entonces la
        // ventana se queda en el alto de la pantalla y se desplaza con la
        // rueda. Aun así, que no pase de unos 1150 px.
        let all = Open { advanced: true, saved: true, custom: true };
        let (rows, h) = layout(all);
        assert_eq!(rows.len(), ITEMS.len());
        assert!(rows.windows(2).all(|w| w[0].1 <= w[1].0));
        assert!(h < 1150.0, "alto {h}");
    }

    #[test]
    fn folds_start_closed_and_hide_their_items() {
        let closed = Open::default();
        let (rows, h_closed) = layout(closed);
        // Rendimiento siempre a la vista; Widget y System, plegados.
        assert!(matches!(ITEMS[0], Item::Section("PERFORMANCE")));
        // Todo lo que va tras "Advanced settings" está oculto, también el
        // plegable de dónde se guardan los ajustes.
        let first_fold = ITEMS.iter().position(|it| matches!(it, Item::Expander(..))).unwrap_or(0);
        for (i, row) in rows.iter().enumerate() {
            let hidden = i > first_fold;
            assert_eq!(!visible(i, closed), hidden, "elemento {i}");
            assert_eq!(row.1 - row.0 < 0.5, hidden, "elemento {i}");
        }
        // Plegada (como se abre) cabe en cualquier pantalla de 768 px.
        assert!(h_closed < 450.0, "alto plegado {h_closed}");
        let (_, h_adv) = layout(Open { advanced: true, saved: false, custom: false });
        let (_, h_saved) = layout(Open { advanced: true, saved: true, custom: false });
        let (_, h_saved_only) = layout(Open { advanced: false, saved: true, custom: false });
        assert!(h_adv > h_closed + 300.0 && h_saved > h_adv);
        assert!((h_saved_only - h_closed).abs() < 0.5);
    }
}
