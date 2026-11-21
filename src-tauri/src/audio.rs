//! Captura el audio que suena en MusicRG y lo convierte en niveles por banda
//! para las barras del widget.
//!
//! En Windows 11 (build 20348+) usa *process loopback*: solo oye el árbol de
//! procesos de WebView2 de MusicRG, no el resto del PC. En sistemas más
//! viejos cae al loopback de la salida por defecto (todo lo que suena).
//!
//! El hilo solo existe mientras el widget está en pantalla. Trabaja ~33 veces
//! por segundo con una FFT de 1024 puntos: unos microsegundos por pasada.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{implement, Interface, Ref, HRESULT};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IAgileObject,
    IAgileObject_Impl, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, SetEvent, WaitForSingleObject,
};
use windows::Win32::System::Variant::VT_BLOB;

pub const BANDS: usize = 14;
const N: usize = 1024;
const FRAME: Duration = Duration::from_millis(33);

#[derive(Default)]
pub struct Levels([AtomicU32; BANDS]);

impl Levels {
    pub fn get(&self, i: usize) -> f32 {
        f32::from_bits(self.0[i].load(Ordering::Relaxed))
    }
    fn set(&self, i: usize, v: f32) {
        self.0[i].store(v.to_bits(), Ordering::Relaxed);
    }
    fn clear(&self) {
        for i in 0..BANDS {
            self.set(i, 0.0);
        }
    }
}

/// Cada arranque tiene su propia bandera: un hilo que está terminando nunca
/// puede apagar al siguiente.
#[derive(Default)]
pub struct Capture {
    levels: Arc<Levels>,
    current: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
}

impl Capture {
    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    pub fn start(&self) {
        let mut cur = self.current.lock().unwrap();
        if let Some((run, h)) = cur.as_ref() {
            if run.load(Ordering::SeqCst) && !h.is_finished() {
                return; // ya está capturando
            }
        }
        if let Some((run, h)) = cur.take() {
            run.store(false, Ordering::SeqCst);
            let _ = h.join(); // sale en <=30 ms
        }
        let run = Arc::new(AtomicBool::new(true));
        let (levels, flag) = (self.levels.clone(), run.clone());
        if let Ok(h) = std::thread::Builder::new().name("rg-audio".into()).spawn(move || {
            if let Err(e) = unsafe { capture(&levels, &flag) } {
                log(&format!("audio: {e}"));
            }
            levels.clear();
        }) {
            *cur = Some((run, h));
        }
    }

    pub fn stop(&self) {
        if let Some((run, _)) = self.current.lock().unwrap().as_ref() {
            run.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(debug_assertions)]
fn log(s: &str) {
    eprintln!("[musicrg] {s}");
}
#[cfg(not(debug_assertions))]
fn log(_: &str) {}

struct Format {
    channels: usize,
    float: bool,
    rate: f32,
}

unsafe fn capture(levels: &Levels, run: &AtomicBool) -> windows::core::Result<()> {
    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    let out = (|| {
        let event = CreateEventW(None, false, false, None)?;
        let (client, fmt) = match process_loopback(event) {
            Ok(v) => {
                log("audio: process loopback");
                v
            }
            Err(e) => {
                log(&format!("audio: process loopback no disponible ({e}), uso la salida"));
                endpoint_loopback()?
            }
        };
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        let mut ring = vec![0f32; N];
        let mut pos = 0usize;
        let mut dsp = Dsp::new(fmt.rate);
        let mut last = Instant::now();
        while run.load(Ordering::Relaxed) {
            // Con process loopback el evento llega cada ~10 ms; con la salida
            // no hay evento y esto hace de sondeo cada 30 ms.
            let _ = WaitForSingleObject(event, 30) == WAIT_OBJECT_0;
            loop {
                let n = cap.GetNextPacketSize()?;
                if n == 0 {
                    break;
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                for f in 0..frames as usize {
                    let mut s = 0f32;
                    if !silent {
                        for c in 0..fmt.channels {
                            let i = f * fmt.channels + c;
                            s += if fmt.float {
                                *(data as *const f32).add(i)
                            } else {
                                *(data as *const i16).add(i) as f32 / 32768.0
                            };
                        }
                        s /= fmt.channels as f32;
                    }
                    ring[pos] = s;
                    pos = (pos + 1) % N;
                }
                cap.ReleaseBuffer(frames)?;
            }
            if last.elapsed() >= FRAME {
                last = Instant::now();
                dsp.analyze(&ring, pos, levels);
            }
        }
        let _ = client.Stop();
        let _ = CloseHandle(event);
        Ok(())
    })();
    CoUninitialize();
    out
}

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Done(HANDLE);

impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
    fn ActivateCompleted(
        &self,
        _: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        unsafe { SetEvent(self.0) }
    }
}
impl IAgileObject_Impl for Done_Impl {}

/// PID del proceso navegador de WebView2 (lo rellena main.rs). Windows no
/// cuenta ese proceso dentro del árbol de MusicRG para el process loopback,
/// así que se captura su árbol directamente: sigue siendo solo audio de MusicRG.
pub static BROWSER_PID: AtomicU32 = AtomicU32::new(0);

fn target_pid() -> u32 {
    match BROWSER_PID.load(Ordering::Relaxed) {
        0 => unsafe { GetCurrentProcessId() },
        pid => pid,
    }
}

unsafe fn process_loopback(event: HANDLE) -> windows::core::Result<(IAudioClient, Format)> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: target_pid(),
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // ManuallyDrop: el Drop de PROPVARIANT llama a PropVariantClear, que
    // liberaría `params` (está en la pila) => corrupción del heap.
    let pv = std::mem::ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: std::mem::size_of_val(&params) as u32,
                        pBlobData: &params as *const _ as *mut u8,
                    },
                },
            }),
        },
    });
    let done = CreateEventW(None, false, false, None)?;
    let handler: IActivateAudioInterfaceCompletionHandler = Done(done).into();
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&*pv as *const PROPVARIANT),
        &handler,
    );
    let op = match op {
        Ok(op) => op,
        Err(e) => {
            let _ = CloseHandle(done);
            return Err(e);
        }
    };
    let _ = WaitForSingleObject(done, 5000);
    let _ = CloseHandle(done);
    let mut hr = HRESULT(0);
    let mut unk = None;
    op.GetActivateResult(&mut hr, &mut unk)?;
    hr.ok()?;
    let client: IAudioClient = unk.ok_or_else(windows::core::Error::empty)?.cast()?;

    // Process loopback no admite GetMixFormat: se pide un formato fijo.
    let fmt = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 2,
        nSamplesPerSec: 44100,
        wBitsPerSample: 16,
        nBlockAlign: 4,
        nAvgBytesPerSec: 44100 * 4,
        cbSize: 0,
    };
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        2_000_000,
        0,
        &fmt,
        None,
    )?;
    client.SetEventHandle(event)?;
    Ok((client, Format { channels: 2, float: false, rate: 44100.0 }))
}

unsafe fn endpoint_loopback() -> windows::core::Result<(IAudioClient, Format)> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let pf = client.GetMixFormat()?;
    let f = *pf;
    let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, pf, None);
    CoTaskMemFree(Some(pf as *const _));
    r?;
    Ok((
        client,
        Format {
            channels: f.nChannels.max(1) as usize,
            float: f.wBitsPerSample == 32,
            rate: f.nSamplesPerSec as f32,
        },
    ))
}

/// FFT de 1024 puntos y agrupación en bandas logarítmicas (50 Hz – 14 kHz).
struct Dsp {
    window: Vec<f32>,
    edges: [usize; BANDS + 1],
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Dsp {
    fn new(rate: f32) -> Self {
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (N as f32 - 1.0)).cos())
            .collect();
        let mut edges = [0usize; BANDS + 1];
        let (lo, hi) = (50f32, 14000f32.min(rate / 2.0 - 1.0));
        for (i, e) in edges.iter_mut().enumerate() {
            let f = lo * (hi / lo).powf(i as f32 / BANDS as f32);
            *e = ((f / rate * N as f32).round() as usize).clamp(1, N / 2);
        }
        for i in 1..=BANDS {
            if edges[i] <= edges[i - 1] {
                edges[i] = (edges[i - 1] + 1).min(N / 2);
            }
        }
        Self { window, edges, re: vec![0.0; N], im: vec![0.0; N] }
    }

    fn analyze(&mut self, ring: &[f32], pos: usize, out: &Levels) {
        for i in 0..N {
            self.re[i] = ring[(pos + i) % N] * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        for b in 0..BANDS {
            let mut m = 0f32;
            for k in self.edges[b]..self.edges[b + 1].max(self.edges[b] + 1) {
                m = m.max(self.re[k].hypot(self.im[k]));
            }
            // Amplitud relativa a fondo de escala (ventana Hann => N/4).
            let db = 20.0 * (m / (N as f32 / 4.0)).max(1e-6).log10();
            // La música cae en agudos: un poco de inclinación para que se muevan.
            let tilt = 1.8 * b as f32;
            out.set(b, ((db + tilt + 50.0) / 46.0).clamp(0.0, 1.0));
        }
    }
}

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1f32, 0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn capture_starts_and_stops() {
        let c = super::Capture::default();
        c.start();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        c.stop();
        std::thread::sleep(std::time::Duration::from_millis(300));
        c.start();
        std::thread::sleep(std::time::Duration::from_millis(500));
        c.stop();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
