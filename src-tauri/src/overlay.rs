//! Ventanas nativas fuera de la app:
//! - Widget "sonando ahora" encima de la barra de tareas (carátula, título,
//!   artista y barras que siguen el audio real de MusicRG).
//! - Aviso de Bloq Mayús activado / desactivado.
//!
//! Todo vive en un hilo propio con su bucle de mensajes Win32 y Direct2D por
//! software sobre ventanas *layered*: sin otro WebView, sin GPU y sin
//! temporizadores cuando no hay nada en pantalla. Con la música en pausa este
//! hilo está dormido; solo despierta con un mensaje (canción nueva, tecla).

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CAPITAL,
};
use windows::Win32::UI::Input::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::art;
use crate::audio::{Capture, Levels, BANDS};

#[derive(Clone, Default, PartialEq, Debug)]
pub struct NowPlaying {
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub art: String,
}

enum Cmd {
    Now(NowPlaying),
    Widget(bool),
    Caps(bool),
    Art(String, Vec<u8>, u32),
    #[cfg(debug_assertions)]
    DemoOsd(bool),
}

struct Shared {
    widget: AtomicIsize,
    inbox: Mutex<Vec<Cmd>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
const WM_RG: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const T_FRAME: usize = 1;
const T_OSD: usize = 2;
const T_CAPS: usize = 3;
/// Cuánto espera en pausa antes de esconderse (al cambiar de canción Apple
/// pasa por "pausa" un instante; así no parpadea).
const HIDE_DELAY: Duration = Duration::from_millis(900);

/// Mando a distancia del hilo de las ventanas. Se puede usar desde cualquier hilo.
pub struct Overlay;

impl Overlay {
    pub fn spawn(widget: bool, caps: bool, on_click: impl Fn() + Send + 'static) -> Self {
        let _ = SHARED.set(Shared { widget: AtomicIsize::new(0), inbox: Mutex::new(Vec::new()) });
        let _ = std::thread::Builder::new()
            .name("rg-overlay".into())
            .spawn(move || unsafe { thread_main(widget, caps, Box::new(on_click)) });
        Overlay
    }
    pub fn now_playing(&self, np: NowPlaying) {
        post(Cmd::Now(np));
    }
    pub fn set_widget(&self, on: bool) {
        post(Cmd::Widget(on));
    }
    pub fn set_caps(&self, on: bool) {
        post(Cmd::Caps(on));
    }
    /// Solo en desarrollo: muestra el aviso de Bloq Mayús sin tocar la tecla.
    #[cfg(debug_assertions)]
    pub fn demo_osd(&self, on: bool) {
        post(Cmd::DemoOsd(on));
    }
}

fn post(cmd: Cmd) {
    let Some(sh) = SHARED.get() else { return };
    sh.inbox.lock().unwrap().push(cmd);
    let h = sh.widget.load(Ordering::Acquire);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as _)), WM_RG, WPARAM(0), LPARAM(0));
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static ON_CLICK: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

unsafe fn thread_main(widget_on: bool, caps_on: bool, on_click: Box<dyn Fn() + Send>) {
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let Ok(hinst) = GetModuleHandleW(None) else { return };
    let class = w!("MusicRGOverlay");
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinst.into(),
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
        ..Default::default()
    });
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    let make = |ex, title: PCWSTR| {
        CreateWindowExW(ex, class, title, WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst.into()), None)
    };
    let (Ok(wh), Ok(oh)) = (
        make(ex, w!("MusicRG - now playing")),
        make(ex | WS_EX_TRANSPARENT, w!("MusicRG - Caps Lock")),
    ) else {
        return;
    };
    let Some(gfx) = Gfx::new() else { return };
    ON_CLICK.with(|c| *c.borrow_mut() = Some(on_click));
    let mut st = State {
        widget: Widget::new(wh, &gfx),
        osd: Osd::new(oh, &gfx),
        gfx,
        capture: Capture::default(),
        theme: Theme::read(),
        widget_on,
        caps_on: false,
    };
    st.set_caps(caps_on);
    STATE.with(|s| *s.borrow_mut() = Some(st));
    SHARED.get().unwrap().widget.store(wh.0 as isize, Ordering::Release);
    with_state(|s| s.drain());

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        DispatchMessageW(&msg);
    }
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            ON_CLICK.with(|c| {
                if let Some(f) = c.borrow().as_ref() {
                    f()
                }
            });
            return LRESULT(0);
        }
        WM_RG | WM_TIMER | WM_INPUT | WM_MOUSEMOVE | WM_MOUSELEAVE | WM_SETTINGCHANGE
        | WM_DISPLAYCHANGE => {
            with_state(|s| s.on_msg(h, m, wp, lp));
        }
        _ => {}
    }
    // WM_INPUT también necesita DefWindowProc para liberar el bloque de datos.
    DefWindowProcW(h, m, wp, lp)
}

struct State {
    gfx: Gfx,
    widget: Widget,
    osd: Osd,
    capture: Capture,
    theme: Theme,
    widget_on: bool,
    caps_on: bool,
}

impl State {
    unsafe fn on_msg(&mut self, h: HWND, m: u32, wp: WPARAM, lp: LPARAM) {
        match m {
            WM_RG => self.drain(),
            WM_TIMER => match wp.0 {
                T_FRAME => {
                    if self.widget.tick(&self.gfx, &self.theme, self.capture.levels()) {
                        self.capture.stop();
                    }
                }
                T_OSD => self.osd.tick(),
                T_CAPS => {
                    let _ = KillTimer(Some(h), T_CAPS);
                    let on = GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0;
                    self.osd.show(&self.gfx, &self.theme, on);
                }
                _ => {}
            },
            WM_INPUT => {
                if self.caps_on && caps_released(lp) {
                    // El estado del conmutador se actualiza justo después.
                    SetTimer(Some(self.widget.surf.hwnd), T_CAPS, 30, None);
                }
            }
            WM_MOUSEMOVE if h == self.widget.surf.hwnd => {
                if !self.widget.hover {
                    self.widget.hover = true;
                    self.widget.dirty = true;
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: h,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut t);
                }
            }
            WM_MOUSELEAVE if h == self.widget.surf.hwnd => {
                self.widget.hover = false;
                self.widget.dirty = true;
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
                self.theme = Theme::read();
                self.widget.relayout = true;
            }
            _ => {}
        }
    }

    unsafe fn drain(&mut self) {
        let cmds = std::mem::take(&mut *SHARED.get().unwrap().inbox.lock().unwrap());
        for cmd in cmds {
            match cmd {
                Cmd::Now(np) => {
                    let show = np.playing && self.widget_on;
                    self.widget.set_track(np);
                    if show {
                        self.widget.show(&self.gfx, &self.theme);
                        self.capture.start();
                    } else {
                        self.widget.hide_later();
                    }
                }
                Cmd::Widget(on) => {
                    self.widget_on = on;
                    if on && self.widget.np.playing {
                        self.widget.show(&self.gfx, &self.theme);
                        self.capture.start();
                    } else if !on {
                        // Se desvanece; el audio se para al terminar (tick).
                        self.widget.hide_now();
                    }
                }
                Cmd::Caps(on) => self.set_caps(on),
                Cmd::Art(url, px, size) => self.widget.set_art(&url, &px, size),
                #[cfg(debug_assertions)]
                Cmd::DemoOsd(on) => self.osd.show(&self.gfx, &self.theme, on),
            }
        }
    }

    unsafe fn set_caps(&mut self, on: bool) {
        if on == self.caps_on {
            return;
        }
        self.caps_on = on;
        // Raw Input en modo "sink": Windows nos avisa de las teclas aunque la
        // ventana no tenga foco, sin hook global (no añade latencia a nadie).
        let dev = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x06,
            dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
            hwndTarget: if on { self.widget.surf.hwnd } else { HWND::default() },
        };
        let _ = RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }
}

unsafe fn caps_released(lp: LPARAM) -> bool {
    let mut ri = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let n = GetRawInputData(
        HRAWINPUT(lp.0 as _),
        RID_INPUT,
        Some(&mut ri as *mut _ as *mut _),
        &mut size,
        std::mem::size_of::<RAWINPUTHEADER>() as u32,
    );
    if n == u32::MAX || ri.header.dwType != RIM_TYPEKEYBOARD.0 {
        return false;
    }
    let kb = ri.data.keyboard;
    kb.VKey == VK_CAPITAL.0 && kb.Flags as u32 & RI_KEY_BREAK != 0
}

// ---------------------------------------------------------------------------
// Dibujo

struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    ui: HSTRING,
}

impl Gfx {
    unsafe fn new() -> Option<Self> {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let mut coll = None;
        dw.GetSystemFontCollection(&mut coll, false).ok()?;
        let coll = coll?;
        let has = |name: &HSTRING| {
            let (mut i, mut ok) = (0u32, BOOL(0));
            coll.FindFamilyName(name, &mut i, &mut ok).is_ok() && ok.as_bool()
        };
        let pick = |a: &str, b: &str| {
            let a = HSTRING::from(a);
            if has(&a) { a } else { HSTRING::from(b) }
        };
        Some(Self {
            ui: pick("Segoe UI Variable Text", "Segoe UI"),
            d2d,
            dw,
        })
    }

    unsafe fn layout(
        &self,
        text: &str,
        font: &HSTRING,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        max_w: f32,
    ) -> Option<IDWriteTextLayout> {
        let fmt = self
            .dw
            .CreateTextFormat(
                font,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("es-es"),
            )
            .ok()?;
        let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&fmt) {
            let trim = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let _ = fmt.SetTrimming(&trim, &sign);
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        self.dw.CreateTextLayout(&wide, &fmt, max_w, size * 2.0).ok()
    }
}

fn text_width(l: &IDWriteTextLayout) -> f32 {
    let mut m = DWRITE_TEXT_METRICS::default();
    unsafe {
        let _ = l.GetMetrics(&mut m);
    }
    m.widthIncludingTrailingWhitespace
}

fn rgba(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a,
    }
}

fn rrect(l: f32, t: f32, r: f32, b: f32, rad: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: l, top: t, right: r, bottom: b },
        radiusX: rad,
        radiusY: rad,
    }
}

/// Ventana layered + mapa de bits en memoria + render target de Direct2D.
struct Surface {
    hwnd: HWND,
    w: i32,
    h: i32,
    dc: HDC,
    bmp: HBITMAP,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
}

impl Surface {
    unsafe fn new(hwnd: HWND, gfx: &Gfx) -> Self {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let rt = gfx.d2d.CreateDCRenderTarget(&props).expect("d2d");
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let brush = rt.CreateSolidColorBrush(&rgba(0xffffff, 1.0), None).expect("brush");
        Self { hwnd, w: 0, h: 0, dc: CreateCompatibleDC(None), bmp: HBITMAP::default(), rt, brush }
    }

    unsafe fn resize(&mut self, w: i32, h: i32) {
        if (w, h) == (self.w, self.h) || w <= 0 || h <= 0 {
            return;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            return;
        };
        SelectObject(self.dc, bmp.into());
        if !self.bmp.is_invalid() {
            let _ = DeleteObject(self.bmp.into());
        }
        self.bmp = bmp;
        (self.w, self.h) = (w, h);
        let _ = self.rt.BindDC(self.dc, &RECT { left: 0, top: 0, right: w, bottom: h });
    }

    unsafe fn fill(&self, r: &D2D1_ROUNDED_RECT, c: D2D1_COLOR_F) {
        self.brush.SetColor(&c);
        self.rt.FillRoundedRectangle(r, &self.brush);
    }

    unsafe fn text(&self, l: &IDWriteTextLayout, x: f32, y: f32, c: D2D1_COLOR_F) {
        self.brush.SetColor(&c);
        self.rt.DrawTextLayout(Vector2 { X: x, Y: y }, l, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
    }

    unsafe fn present(&self, x: i32, y: i32, alpha: f32) {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0) as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let r = UpdateLayeredWindow(
            self.hwnd,
            None,
            Some(&POINT { x, y }),
            Some(&SIZE { cx: self.w, cy: self.h }),
            Some(self.dc),
            Some(&POINT { x: 0, y: 0 }),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        #[cfg(debug_assertions)]
        if let Err(e) = r {
            eprintln!("[musicrg] UpdateLayeredWindow: {e}");
        }
        #[cfg(not(debug_assertions))]
        let _ = r;
    }
}

// ---------------------------------------------------------------------------
// Tema de Windows (claro/oscuro y color de acento)

struct Theme {
    light: bool,
    accent: D2D1_COLOR_F,
}

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const ACCENT: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent");
const ADVANCED: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced");

impl Theme {
    fn read() -> Self {
        let light = reg_dword(PERSONALIZE, w!("SystemUsesLightTheme")) == Some(1);
        // AccentPalette: 8 colores RGBA, de más claro a más oscuro.
        let mut pal = [0u8; 32];
        let accent = if reg_bin(ACCENT, w!("AccentPalette"), &mut pal) {
            let i = if light { 4 } else { 1 };
            let c = &pal[i * 4..i * 4 + 3];
            rgba((c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32, 1.0)
        } else if light {
            rgba(0x005fb8, 1.0)
        } else {
            rgba(0x7dd3fc, 1.0)
        };
        Self { light, accent }
    }
    fn fg(&self) -> u32 {
        if self.light { 0x000000 } else { 0xffffff }
    }
}

fn reg_dword(path: PCWSTR, name: PCWSTR) -> Option<u32> {
    let (mut v, mut n) = (0u32, 4u32);
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, path, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut _), Some(&mut n))
    };
    (r == ERROR_SUCCESS).then_some(v)
}

fn reg_bin(path: PCWSTR, name: PCWSTR, buf: &mut [u8]) -> bool {
    let mut n = buf.len() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, path, name, RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut n))
    };
    r == ERROR_SUCCESS && n as usize == buf.len()
}

// ---------------------------------------------------------------------------
// Barra de tareas

struct Taskbar {
    hwnd: HWND,
    rect: RECT,
    work: RECT,
    scale: f32,
}

unsafe fn taskbar() -> Option<Taskbar> {
    let hwnd = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
    let mut rect = RECT::default();
    GetWindowRect(hwnd, &mut rect).ok()?;
    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(mon, &mut mi).as_bool() {
        return None;
    }
    let h = rect.bottom - rect.top;
    let w = rect.right - rect.left;
    // Vertical u oculta (auto-ocultar): no hay sitio para el widget.
    let visible = rect.bottom.min(mi.rcMonitor.bottom) - rect.top.max(mi.rcMonitor.top);
    if h <= 0 || w < h * 3 || visible < h / 2 || !IsWindowVisible(hwnd).as_bool() {
        return None;
    }
    let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
    Some(Taskbar { hwnd, rect, work: mi.rcWork, scale })
}

/// Juego, vídeo o presentación a pantalla completa: mejor no pintar encima.
unsafe fn fullscreen_busy() -> bool {
    matches!(
        SHQueryUserNotificationState(),
        Ok(QUNS_BUSY) | Ok(QUNS_RUNNING_D3D_FULL_SCREEN) | Ok(QUNS_PRESENTATION_MODE)
    )
}

// ---------------------------------------------------------------------------
// Widget "sonando ahora"

const BAR_W: f32 = 3.0;
const BAR_GAP: f32 = 2.0;
const ART: f32 = 32.0;
const TEXT_W: f32 = 150.0;

struct Widget {
    surf: Surface,
    np: NowPlaying,
    title: Option<IDWriteTextLayout>,
    artist: Option<IDWriteTextLayout>,
    art: Option<(ID2D1Bitmap, u32)>,
    art_url: String,
    bars: [f32; BANDS],
    pos: (i32, i32),
    scale: f32,
    owner: HWND,
    alpha: f32,
    visible: bool,
    want: bool,
    blocked: bool,
    hide_at: Option<Instant>,
    last_frame: Instant,
    last_check: Instant,
    hover: bool,
    dirty: bool,
    relayout: bool,
}

impl Widget {
    unsafe fn new(hwnd: HWND, gfx: &Gfx) -> Self {
        Self {
            surf: Surface::new(hwnd, gfx),
            np: NowPlaying::default(),
            title: None,
            artist: None,
            art: None,
            art_url: String::new(),
            bars: [0.0; BANDS],
            pos: (0, 0),
            scale: 1.0,
            owner: HWND::default(),
            alpha: 0.0,
            visible: false,
            want: false,
            blocked: false,
            hide_at: None,
            last_frame: Instant::now(),
            last_check: Instant::now(),
            hover: false,
            dirty: true,
            relayout: true,
        }
    }

    fn set_track(&mut self, np: NowPlaying) {
        if np.title != self.np.title || np.artist != self.np.artist {
            self.title = None;
            self.artist = None;
            self.relayout = true;
        }
        if np.art != self.art_url {
            self.art_url = np.art.clone();
            self.art = None;
            self.fetch_art();
        }
        self.np = np;
        self.dirty = true;
    }

    fn fetch_art(&self) {
        if self.art_url.is_empty() {
            return;
        }
        let url = self.art_url.clone();
        let px = (ART * self.scale).round() as u32;
        let _ = std::thread::Builder::new().name("rg-art".into()).spawn(move || {
            let pixels = art::fetch(&url, px);
            #[cfg(debug_assertions)]
            eprintln!("[musicrg] art {url} -> {}", pixels.is_some());
            if let Some(pixels) = pixels {
                post(Cmd::Art(url, pixels, px));
            }
        });
    }

    unsafe fn set_art(&mut self, url: &str, px: &[u8], size: u32) {
        if url != self.art_url {
            return;
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        if let Ok(bmp) = self.surf.rt.CreateBitmap(
            D2D_SIZE_U { width: size, height: size },
            Some(px.as_ptr() as *const _),
            size * 4,
            &props,
        ) {
            self.art = Some((bmp, size));
            self.dirty = true;
        }
    }

    unsafe fn show(&mut self, gfx: &Gfx, _theme: &Theme) {
        self.hide_at = None;
        self.want = true;
        if !self.visible {
            self.relayout = true;
            self.place(gfx);
            self.visible = true;
            self.last_frame = Instant::now();
            SetTimer(Some(self.surf.hwnd), T_FRAME, 33, None);
        }
    }

    fn hide_later(&mut self) {
        if self.want && self.hide_at.is_none() {
            self.hide_at = Some(Instant::now() + HIDE_DELAY);
        }
    }

    unsafe fn hide_now(&mut self) {
        self.want = false;
        self.hide_at = None;
    }

    /// Calcula tamaño y posición encima de la barra de tareas.
    unsafe fn place(&mut self, gfx: &Gfx) -> bool {
        let Some(tb) = taskbar() else {
            self.blocked = true;
            return false;
        };
        self.blocked = fullscreen_busy();
        let s = tb.scale;
        if (s - self.scale).abs() > 0.01 {
            self.scale = s;
            self.title = None;
            self.artist = None;
            if self.art.is_some() {
                self.art = None;
                self.fetch_art();
            }
        }
        let mut changed = self.relayout;
        if self.title.is_none() {
            changed = true;
            let t = if self.np.title.is_empty() { "MusicRG" } else { &self.np.title };
            self.title = gfx.layout(t, &gfx.ui, 12.5 * s, DWRITE_FONT_WEIGHT_SEMI_BOLD, TEXT_W * s);
            self.artist = gfx.layout(&self.np.artist, &gfx.ui, 12.0 * s, DWRITE_FONT_WEIGHT_NORMAL, TEXT_W * s);
        }
        let text_w = self
            .title
            .iter()
            .chain(self.artist.iter())
            .map(text_width)
            .fold(0.0, f32::max)
            .min(TEXT_W * s);
        let bars_w = BANDS as f32 * (BAR_W + BAR_GAP) * s - BAR_GAP * s;
        let w = (8.0 * s + ART * s + 10.0 * s + text_w + 14.0 * s + bars_w + 12.0 * s).ceil() as i32;
        let h = tb.rect.bottom - tb.rect.top;
        let left_aligned = reg_dword(ADVANCED, w!("TaskbarAl")) == Some(0);
        let x = if left_aligned {
            // Iconos a la izquierda: el widget va junto al área de notificación.
            let tray = FindWindowExW(Some(tb.hwnd), None, w!("TrayNotifyWnd"), None)
                .ok()
                .and_then(|t| {
                    let mut r = RECT::default();
                    GetWindowRect(t, &mut r).ok().map(|_| r.left)
                })
                .unwrap_or(tb.rect.right - (320.0 * s) as i32);
            tray - w - (8.0 * s) as i32
        } else {
            // Iconos centrados: hueco libre a la izquierda. Si el botón de
            // Widgets (el tiempo) está activado, se deja su sitio.
            let widgets = reg_dword(ADVANCED, w!("TaskbarDa")) != Some(0);
            let offset = crate::settings::widget_offset().unwrap_or(if widgets { 168.0 } else { 6.0 });
            tb.rect.left + (offset * s) as i32
        };
        changed |= (w, h) != (self.surf.w, self.surf.h) || (x, tb.rect.top) != self.pos;
        self.surf.resize(w, h);
        self.pos = (x, tb.rect.top);
        if self.owner != tb.hwnd {
            // Ventana "propiedad" de la barra de tareas: se queda siempre
            // encima de ella, y si un juego tapa la barra, tapa también esto.
            SetWindowLongPtrW(self.surf.hwnd, GWLP_HWNDPARENT, tb.hwnd.0 as isize);
            self.owner = tb.hwnd;
        }
        self.relayout = false;
        // Solo se repinta si algo cambió: con la barra quieta, cero trabajo.
        self.dirty |= changed;
        true
    }

    /// Un fotograma. Devuelve `true` cuando el widget acaba de esconderse del todo.
    unsafe fn tick(&mut self, gfx: &Gfx, theme: &Theme, levels: &Levels) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        if self.hide_at.is_some_and(|t| now >= t) {
            self.hide_at = None;
            self.want = false;
        }
        if self.relayout || now.duration_since(self.last_check) >= Duration::from_millis(1000) {
            self.last_check = now;
            let was = self.blocked;
            self.place(gfx);
            if self.blocked != was {
                self.dirty = true;
            }
        }

        let target = if self.want && !self.blocked { 1.0 } else { 0.0 };
        let old_alpha = self.alpha;
        let step = dt / 0.18;
        if target > self.alpha {
            self.alpha = (self.alpha + step).min(1.0);
        } else if target < self.alpha {
            self.alpha = (self.alpha - step).max(0.0);
        }

        if self.alpha == 0.0 && !self.want {
            // Fuera del todo: sin temporizador ni captura hasta la próxima canción.
            let _ = KillTimer(Some(self.surf.hwnd), T_FRAME);
            let _ = ShowWindow(self.surf.hwnd, SW_HIDE);
            self.visible = false;
            self.bars = [0.0; BANDS];
            return true;
        }

        // Barras: subida rápida, bajada suave.
        let mut moved = 0f32;
        for i in 0..BANDS {
            let t = if self.np.playing { levels.get(i) } else { 0.0 };
            let k = if t > self.bars[i] { 0.6 } else { 0.2 };
            let v = self.bars[i] + (t - self.bars[i]) * k;
            moved += (v - self.bars[i]).abs();
            self.bars[i] = v;
        }

        if self.alpha == 0.0 {
            if IsWindowVisible(self.surf.hwnd).as_bool() {
                let _ = ShowWindow(self.surf.hwnd, SW_HIDE);
            }
            return false;
        }
        if !(self.dirty || moved > 0.004 || self.alpha != old_alpha) {
            return false; // nada cambió: ni un píxel que repintar
        }
        self.dirty = false;
        self.draw(theme);
        self.surf.present(self.pos.0, self.pos.1, self.alpha);
        if !IsWindowVisible(self.surf.hwnd).as_bool() {
            let _ = ShowWindow(self.surf.hwnd, SW_SHOWNOACTIVATE);
        }
        false
    }

    unsafe fn draw(&self, theme: &Theme) {
        let s = self.scale;
        let (w, h) = (self.surf.w as f32, self.surf.h as f32);
        let fg = theme.fg();
        let rt = &self.surf.rt;
        rt.BeginDraw();
        // Casi transparente pero no del todo: así todo el rectángulo recibe clics.
        rt.Clear(Some(&rgba(0, 1.0 / 255.0)));
        let pad = (h - 40.0 * s).max(0.0) / 2.0;
        if self.hover {
            self.surf.fill(&rrect(2.0 * s, pad, w - 2.0 * s, h - pad, 6.0 * s), rgba(fg, 0.08));
        }

        // Carátula
        let a = ART * s;
        let (ax, ay) = (8.0 * s, (h - a) / 2.0);
        let art_rect = rrect(ax, ay, ax + a, ay + a, 4.0 * s);
        match &self.art {
            Some((bmp, size)) => {
                if let Ok(brush) = rt.CreateBitmapBrush(bmp, None, None) {
                    let k = a / *size as f32;
                    brush.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: ax, M32: ay });
                    rt.FillRoundedRectangle(&art_rect, &brush);
                }
            }
            None => self.surf.fill(&art_rect, rgba(fg, 0.12)),
        }

        // Texto
        let tx = ax + a + 10.0 * s;
        let mut text_w = 0f32;
        if let Some(t) = &self.title {
            text_w = text_w.max(text_width(t));
            self.surf.text(t, tx, h / 2.0 - 18.0 * s, rgba(fg, 0.95));
        }
        if let Some(t) = &self.artist {
            text_w = text_w.max(text_width(t));
            self.surf.text(t, tx, h / 2.0 + 0.5 * s, rgba(fg, 0.6));
        }

        // Barras
        let bx = tx + text_w.min(TEXT_W * s) + 14.0 * s;
        let (bw, gap) = (BAR_W * s, BAR_GAP * s);
        let (min_h, max_h) = (3.0 * s, 24.0 * s);
        for (i, v) in self.bars.iter().enumerate() {
            let bh = min_h + (max_h - min_h) * v;
            let x = bx + i as f32 * (bw + gap);
            let y = (h - bh) / 2.0;
            let mut c = theme.accent;
            c.a = 0.55 + 0.45 * v;
            self.surf.fill(&rrect(x, y, x + bw, y + bh, bw / 2.0), c);
        }
        let _ = rt.EndDraw(None, None);
    }
}

// ---------------------------------------------------------------------------
// Aviso de Bloq Mayús
//
// La tarjeta entra creciendo con un pequeño rebote y el candado está dibujado
// con geometría (no es un glifo), así que el grillete puede moverse:
// - activar:    el grillete abierto gira, baja y encaja con un "clic".
// - desactivar: el grillete sube, gira hacia fuera y el candado se balancea.
// Solo se redibuja mientras algo se mueve; en la pausa el hilo duerme.

const OSD_POP: f32 = 0.34; // entrada de la tarjeta
const OSD_LOCK_DELAY: f32 = 0.10; // el candado empieza cuando la tarjeta ya se ve
const OSD_LOCK: f32 = 0.55; // animación del candado
const OSD_HOLD: f32 = 1.1; // quieto en pantalla
const OSD_OUT: f32 = 0.22; // salida
const OSD_SHOWN: f32 = OSD_LOCK_DELAY + OSD_LOCK + OSD_HOLD;

/// Grillete abierto: cuánto sube (unidades del candado) y cuánto gira (grados).
const SHACKLE_LIFT: f32 = 1.7;
const SHACKLE_ANGLE: f32 = -32.0;
/// Pie izquierdo del grillete: el eje sobre el que gira al abrirse.
const SHACKLE_X: f32 = 3.3;
const SHACKLE_FOOT: f32 = 2.5;

/// Solo en desarrollo: MUSICRG_OSD_SLOW=10 reproduce la animación 10 veces
/// más lenta para poder verla fotograma a fotograma.
#[cfg(debug_assertions)]
fn osd_slow() -> f32 {
    static SLOW: OnceLock<f32> = OnceLock::new();
    *SLOW.get_or_init(|| {
        std::env::var("MUSICRG_OSD_SLOW").ok().and_then(|v| v.parse().ok()).filter(|v: &f32| *v > 0.0).unwrap_or(1.0)
    })
}
#[cfg(not(debug_assertions))]
fn osd_slow() -> f32 {
    1.0
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn ease_out(x: f32) -> f32 {
    1.0 - (1.0 - x).powi(3)
}
fn ease_in(x: f32) -> f32 {
    x * x * x
}
fn ease_in_out(x: f32) -> f32 {
    if x < 0.5 { 4.0 * x * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(3) / 2.0 }
}
/// Se pasa un poco y vuelve: da la sensación de "pop".
fn ease_out_back(x: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}
fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, k: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: a.r + (b.r - a.r) * k,
        g: a.g + (b.g - a.g) * k,
        b: a.b + (b.b - a.b) * k,
        a: a.a + (b.a - a.a) * k,
    }
}

/// Postura del candado en un instante.
struct LockPose {
    lift: f32,
    angle: f32,
    /// Balanceo de todo el candado (grados) al abrirse.
    wiggle: f32,
    /// "Clic" al cerrarse: el candado crece un poco y vuelve.
    bump: f32,
    /// 0 = color normal, 1 = color de acento (cerrado / activado).
    accent: f32,
}

impl LockPose {
    fn at(on: bool, u: f32) -> Self {
        if on {
            // Cerrar: gira de vuelta, baja y encaja.
            let swing = ease_in_out(clamp01(u / 0.45));
            let drop = ease_in(clamp01((u - 0.4) / 0.25));
            let click = clamp01((u - 0.62) / 0.38);
            Self {
                angle: SHACKLE_ANGLE * (1.0 - swing),
                lift: SHACKLE_LIFT * (1.0 - drop),
                wiggle: 0.0,
                bump: 0.16 * (std::f32::consts::PI * click).sin(),
                accent: ease_out(click),
            }
        } else {
            // Abrir: sube, gira hacia fuera con rebote y se balancea.
            let w = clamp01((u - 0.5) / 0.5);
            Self {
                lift: SHACKLE_LIFT * ease_out(clamp01(u / 0.3)),
                angle: SHACKLE_ANGLE * ease_out_back(clamp01((u - 0.18) / 0.5)),
                wiggle: 9.0 * (w * std::f32::consts::TAU).sin() * (1.0 - w),
                bump: 0.0,
                accent: 1.0 - ease_out(clamp01(u / 0.3)),
            }
        }
    }
}

struct Osd {
    surf: Surface,
    start: Option<Instant>,
    pos: (i32, i32),
    alpha: f32,
    /// Estado nuevo de Bloq Mayús que se está mostrando.
    on: bool,
    /// Si la tarjeta entra creciendo (false = ya estaba en pantalla).
    pop: bool,
    s: f32,
    margin: f32,
    card: (f32, f32),
    text: Option<IDWriteTextLayout>,
    fg: u32,
    bg: D2D1_COLOR_F,
    accent: D2D1_COLOR_F,
    shackle: Option<ID2D1PathGeometry>,
    round: Option<ID2D1StrokeStyle>,
}

impl Osd {
    unsafe fn new(hwnd: HWND, gfx: &Gfx) -> Self {
        Self {
            surf: Surface::new(hwnd, gfx),
            start: None,
            pos: (0, 0),
            alpha: 0.0,
            on: false,
            pop: true,
            s: 1.0,
            margin: 0.0,
            card: (0.0, 0.0),
            text: None,
            fg: 0xffffff,
            bg: rgba(0x202020, 0.96),
            accent: rgba(0xffffff, 1.0),
            shackle: shackle_geometry(gfx),
            round: gfx
                .d2d
                .CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        dashCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        miterLimit: 10.0,
                        dashStyle: D2D1_DASH_STYLE_SOLID,
                        dashOffset: 0.0,
                    },
                    None,
                )
                .ok(),
        }
    }

    unsafe fn show(&mut self, gfx: &Gfx, theme: &Theme, on: bool) {
        let (s, work) = match taskbar() {
            Some(tb) => (tb.scale, tb.work),
            None => {
                let mut r = RECT::default();
                let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut _ as *mut _), Default::default());
                (1.0, r)
            }
        };
        let label = if on { "Caps Lock on" } else { "Caps Lock off" };
        let Some(text) = gfx.layout(label, &gfx.ui, 13.0 * s, DWRITE_FONT_WEIGHT_NORMAL, 400.0 * s) else {
            return;
        };
        let card_w = (14.0 + 16.0 + 9.0 + 14.0) * s + text_width(&text);
        let card_h = 44.0 * s;
        // Margen transparente para que el rebote (crece más del 100 %) no se corte.
        let margin = (10.0 * s).ceil();
        let w = (card_w + 2.0 * margin).ceil() as i32;
        let h = (card_h + 2.0 * margin).ceil() as i32;
        self.surf.resize(w, h);

        self.s = s;
        self.margin = margin;
        self.card = (card_w.ceil(), card_h);
        self.text = Some(text);
        self.fg = theme.fg();
        self.bg = if theme.light { rgba(0xf3f3f3, 0.97) } else { rgba(0x202020, 0.96) };
        self.accent = theme.accent;
        self.on = on;
        // Si ya estaba en pantalla no vuelve a entrar: solo da un pequeño salto.
        self.pop = !(self.start.is_some() && self.alpha > 0.5);
        self.pos = (
            (work.left + work.right) / 2 - w / 2,
            work.bottom - h + margin as i32 - (16.0 * s) as i32,
        );
        self.start = Some(Instant::now());
        self.alpha = -1.0; // fuerza el primer present
        let _ = SetWindowPos(self.surf.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        self.tick();
        let _ = ShowWindow(self.surf.hwnd, SW_SHOWNOACTIVATE);
    }

    unsafe fn tick(&mut self) {
        let Some(start) = self.start else { return };
        let t = start.elapsed().as_secs_f32() / osd_slow();
        if t >= OSD_SHOWN + OSD_OUT {
            self.start = None;
            self.alpha = 0.0;
            let _ = KillTimer(Some(self.surf.hwnd), T_OSD);
            let _ = ShowWindow(self.surf.hwnd, SW_HIDE);
            return;
        }

        // Tarjeta: escala, desplazamiento vertical y opacidad.
        let (mut scale, mut alpha, mut rise) = if self.pop {
            let k = clamp01(t / OSD_POP);
            (0.72 + 0.28 * ease_out_back(k), ease_out(clamp01(k * 1.8)), 8.0 * self.s * (1.0 - ease_out(k)))
        } else {
            (1.0 + 0.04 * (std::f32::consts::PI * clamp01(t / 0.2)).sin(), 1.0, 0.0)
        };
        if t > OSD_SHOWN {
            let k = clamp01((t - OSD_SHOWN) / OSD_OUT);
            scale *= 1.0 - 0.08 * ease_in(k);
            alpha *= 1.0 - k;
            rise += 4.0 * self.s * ease_in(k);
        }
        let u = clamp01((t - OSD_LOCK_DELAY) / OSD_LOCK);
        self.draw(scale, rise, &LockPose::at(self.on, u));
        self.alpha = alpha;
        self.surf.present(self.pos.0, self.pos.1, alpha);

        // Entre el final de la animación y la salida no cambia nada: a dormir.
        let still_from = OSD_LOCK_DELAY + OSD_LOCK;
        let next = if t >= still_from && t < OSD_SHOWN {
            (((OSD_SHOWN - t) * osd_slow() * 1000.0) as u32).max(1)
        } else {
            16
        };
        SetTimer(Some(self.surf.hwnd), T_OSD, next, None);
    }

    unsafe fn draw(&self, scale: f32, rise: f32, pose: &LockPose) {
        let Some(text) = &self.text else { return };
        let (s, m) = (self.s, self.margin);
        let (cw, ch) = self.card;
        let rt = &self.surf.rt;
        let brush = &self.surf.brush;
        let fg = self.fg;

        rt.BeginDraw();
        rt.SetTransform(&Matrix3x2::identity());
        rt.Clear(Some(&rgba(0, 0.0)));
        let center = Vector2 { X: m + cw / 2.0, Y: m + ch / 2.0 };
        let card_tf = Matrix3x2::scale_around(scale, scale, center) * Matrix3x2::translation(0.0, rise);
        rt.SetTransform(&card_tf);

        let card = rrect(m + 0.5, m + 0.5, m + cw - 0.5, m + ch - 0.5, 8.0 * s);
        self.surf.fill(&card, self.bg);
        brush.SetColor(&rgba(fg, 0.09));
        rt.DrawRoundedRectangle(&card, brush, 1.0, None);

        let pad = 14.0 * s;
        let cy = m + ch / 2.0 - 3.0 * s;
        self.surf.text(text, m + pad + 16.0 * s + 9.0 * s, cy - 9.5 * s, rgba(fg, 0.95));

        // Pastilla de estado: se alarga y toma el color de acento al activar.
        let pill_w = (18.0 + 22.0 * pose.accent) * s;
        let px = m + (cw - pill_w) / 2.0;
        let pill = rrect(px, m + ch - 9.0 * s, px + pill_w, m + ch - 6.0 * s, 1.5 * s);
        self.surf.fill(&pill, mix(rgba(fg, 0.3), self.accent, pose.accent));

        // Candado en unidades propias (caja de ~16 x 16), escalado a la pantalla.
        let k = s * 0.92 * (1.0 + pose.bump);
        let lock_tf = Matrix3x2::scale(k, k)
            * Matrix3x2::rotation(pose.wiggle)
            * Matrix3x2::translation(m + pad + 8.0 * s, cy - 0.6 * s)
            * card_tf;
        let color = mix(rgba(fg, 0.9), self.accent, pose.accent);

        // Grillete primero: las patas quedan tapadas por el cuerpo.
        if let (Some(shackle), Some(round)) = (&self.shackle, &self.round) {
            let pivot = Vector2 { X: -SHACKLE_X, Y: SHACKLE_FOOT - pose.lift };
            rt.SetTransform(
                &(Matrix3x2::translation(0.0, -pose.lift) * Matrix3x2::rotation_around(pose.angle, pivot) * lock_tf),
            );
            brush.SetColor(&color);
            rt.DrawGeometry(shackle, brush, 1.9, round);
        }
        rt.SetTransform(&lock_tf);
        self.surf.fill(&rrect(-6.0, -1.0, 6.0, 7.0, 2.0), color);
        // Bocallave hueca, del color de la tarjeta.
        brush.SetColor(&self.bg);
        rt.FillEllipse(
            &D2D1_ELLIPSE { point: Vector2 { X: 0.0, Y: 2.4 }, radiusX: 1.15, radiusY: 1.15 },
            brush,
        );
        rt.FillRectangle(&D2D_RECT_F { left: -0.5, top: 2.4, right: 0.5, bottom: 4.6 }, brush);

        rt.SetTransform(&Matrix3x2::identity());
        let _ = rt.EndDraw(None, None);
    }
}

/// Grillete en "U" invertida: dos patas y un arco por arriba.
unsafe fn shackle_geometry(gfx: &Gfx) -> Option<ID2D1PathGeometry> {
    let geo = gfx.d2d.CreatePathGeometry().ok()?;
    let sink = geo.Open().ok()?;
    let top = -2.4; // arco bajo: el grillete asoma poco sobre el cuerpo
    sink.BeginFigure(Vector2 { X: -SHACKLE_X, Y: SHACKLE_FOOT }, D2D1_FIGURE_BEGIN_HOLLOW);
    sink.AddLine(Vector2 { X: -SHACKLE_X, Y: top });
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: Vector2 { X: SHACKLE_X, Y: top },
        size: D2D_SIZE_F { width: SHACKLE_X, height: SHACKLE_X },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.AddLine(Vector2 { X: SHACKLE_X, Y: SHACKLE_FOOT });
    sink.EndFigure(D2D1_FIGURE_END_OPEN);
    sink.Close().ok()?;
    Some(geo)
}
