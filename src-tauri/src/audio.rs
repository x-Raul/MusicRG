//! Captura el audio que suena en MusicRG y lo convierte en niveles por banda
//! para las barras del widget.
//!
//! En Windows 11 (build 20348+) usa *process loopback*: solo oye el árbol de
//! procesos de WebView2 de MusicRG, no el resto del PC. En sistemas más
//! viejos cae al loopback de la salida por defecto (todo lo que suena).
//!
//! Si lo que suena es otra app (el navegador, Spotify...), se captura al
//! revés: todo lo que suena en el PC *menos* MusicRG.
//!
//! El hilo solo existe mientras el widget está en pantalla. Trabaja ~33 veces
//! por segundo con una FFT de 1024 puntos: unos microsegundos por pasada.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::lock::lock;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{Interface, HRESULT};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, WaitForSingleObject};
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

/// De quién son las barras.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// El audio de MusicRG (su árbol de procesos de WebView2).
    MusicRG,
    /// Todo lo demás que suena en el PC (otra app reproduciendo).
    Others,
}

/// Un hilo de captura en marcha.
struct Running {
    /// Bandera propia de este arranque: un hilo que está terminando nunca
    /// puede apagar al siguiente.
    run: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    source: Source,
}

#[derive(Default)]
pub struct Capture {
    levels: Arc<Levels>,
    current: Mutex<Option<Running>>,
}

impl Capture {
    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    pub fn start(&self, source: Source) {
        let mut cur = lock(&self.current);
        if let Some(r) = cur.as_ref() {
            if r.source == source && r.run.load(Ordering::SeqCst) && !r.thread.is_finished() {
                return; // ya está capturando esa fuente
            }
        }
        if let Some(old) = cur.take() {
            old.run.store(false, Ordering::SeqCst);
            let _ = old.thread.join(); // sale en <=30 ms
        }
        let run = Arc::new(AtomicBool::new(true));
        let (levels, flag) = (self.levels.clone(), run.clone());
        if let Ok(thread) = std::thread::Builder::new().name("rg-audio".into()).spawn(move || {
            // SAFETY: es un hilo nuevo dedicado solo a la captura.
            if let Err(e) = unsafe { capture(&levels, &flag, source) } {
                log(&format!("audio: {e}"));
            }
            levels.clear();
        }) {
            *cur = Some(Running { run, thread, source });
        }
    }

    pub fn stop(&self) {
        if let Some(r) = lock(&self.current).as_ref() {
            r.run.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(debug_assertions)]
fn log(s: &str) {
    eprintln!("[musicrg] {s}");
}
#[cfg(not(debug_assertions))]
fn log(_: &str) {}

/// Cómo viene cada muestra en el búfer de WASAPI (siempre little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sample {
    F32,
    I16,
    I24,
    I32,
}

impl Sample {
    /// Bytes que ocupa una muestra.
    fn size(self) -> usize {
        match self {
            Self::I16 => 2,
            Self::I24 => 3,
            Self::F32 | Self::I32 => 4,
        }
    }

    /// Lee una muestra de `b` (exactamente `size()` bytes) a -1.0..1.0.
    /// Se lee byte a byte: el búfer no tiene por qué estar alineado.
    fn read(self, b: &[u8]) -> f32 {
        match (self, b) {
            (Self::F32, &[a, b, c, d]) => f32::from_le_bytes([a, b, c, d]),
            (Self::I16, &[a, b]) => f32::from(i16::from_le_bytes([a, b])) / 32_768.0,
            (Self::I24, &[a, b, c]) => i32::from_le_bytes([0, a, b, c]) as f32 / 2_147_483_648.0,
            (Self::I32, &[a, b, c, d]) => i32::from_le_bytes([a, b, c, d]) as f32 / 2_147_483_648.0,
            _ => 0.0,
        }
    }

    /// Del formato de mezcla de Windows. `sub` es el primer campo del GUID
    /// `SubFormat` cuando el formato es `WAVE_FORMAT_EXTENSIBLE`.
    fn from_wave(tag: u16, bits: u16, sub: Option<u32>) -> Option<Self> {
        const PCM: u16 = 1;
        const FLOAT: u16 = 3;
        let float = match (tag, sub) {
            (FLOAT, _) | (EXTENSIBLE, Some(3)) => true,
            (PCM, _) | (EXTENSIBLE, Some(1)) => false,
            _ => return None,
        };
        match (float, bits) {
            (true, 32) => Some(Self::F32),
            (false, 16) => Some(Self::I16),
            (false, 24) => Some(Self::I24),
            (false, 32) => Some(Self::I32),
            _ => None,
        }
    }
}

/// `WAVE_FORMAT_EXTENSIBLE`: el tipo real va en `SubFormat`.
const EXTENSIBLE: u16 = 0xFFFE;

struct Format {
    channels: usize,
    sample: Sample,
    rate: f32,
}

/// Un evento de Windows que se cierra solo al soltarlo.
struct Event(HANDLE);

impl Event {
    fn new() -> windows::core::Result<Self> {
        // SAFETY: sin nombre ni atributos de seguridad; el handle es nuestro.
        unsafe { CreateEventW(None, false, false, None) }.map(Self)
    }
}

// SAFETY: un handle de evento del kernel se puede señalar, esperar y cerrar
// desde cualquier hilo; `Event` solo lo cierra una vez, en `Drop`.
unsafe impl Send for Event {}
// SAFETY: ver arriba; `&Event` solo da acceso de lectura al handle.
unsafe impl Sync for Event {}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: el handle lo abrió `Event::new` y solo se cierra aquí.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// # Safety
/// Hay que llamarla en un hilo propio: inicia COM en él y lo cierra al salir.
unsafe fn capture(levels: &Levels, run: &AtomicBool, source: Source) -> windows::core::Result<()> {
    CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    let out = (|| {
        let event = Event::new()?;
        let (client, fmt) = match process_loopback(event.0, source) {
            Ok(v) => {
                log(&format!("audio: process loopback ({source:?})"));
                v
            }
            Err(e) => {
                log(&format!("audio: process loopback no disponible ({e}), uso la salida"));
                endpoint_loopback()?
            }
        };
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        let block = fmt.channels * fmt.sample.size();
        let mut ring = vec![0f32; N];
        let mut pos = 0usize;
        let mut dsp = Dsp::new(fmt.rate);
        let mut last = Instant::now();
        while run.load(Ordering::Relaxed) {
            // Con process loopback el evento llega cada ~10 ms; con la salida
            // no hay evento y esto hace de sondeo cada 30 ms.
            let _ = WaitForSingleObject(event.0, 30) == WAIT_OBJECT_0;
            loop {
                let n = cap.GetNextPacketSize()?;
                if n == 0 {
                    break;
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                cap.GetBuffer(&raw mut data, &raw mut frames, &raw mut flags, None, None)?;
                let frames_n = frames as usize;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0.cast_unsigned() != 0;
                let bytes: &[u8] = if silent || data.is_null() || frames_n == 0 {
                    &[]
                } else {
                    // SAFETY: WASAPI deja `frames` tramas de `block` bytes
                    // legibles en `data` hasta `ReleaseBuffer`, y el slice no
                    // sale de este bloque. Es `[u8]`: no exige alineación.
                    std::slice::from_raw_parts(data, frames_n * block)
                };
                if bytes.is_empty() {
                    for _ in 0..frames_n {
                        ring[pos] = 0.0;
                        pos = (pos + 1) % N;
                    }
                } else {
                    for frame in bytes.chunks_exact(block) {
                        let sum: f32 = frame.chunks_exact(fmt.sample.size()).map(|b| fmt.sample.read(b)).sum();
                        ring[pos] = sum / fmt.channels as f32;
                        pos = (pos + 1) % N;
                    }
                }
                cap.ReleaseBuffer(frames)?;
            }
            if last.elapsed() >= FRAME {
                last = Instant::now();
                dsp.analyze(&ring, pos, levels);
            }
        }
        let _ = client.Stop();
        Ok(())
    })();
    CoUninitialize();
    out
}

/// Aviso de Windows cuando termina la activación asíncrona.
mod done {
    // El código que genera `#[implement]` no es nuestro.
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use std::sync::Arc;

    use windows::core::{implement, Ref};
    use windows::Win32::Media::Audio::{
        IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Impl,
    };
    use windows::Win32::System::Com::{IAgileObject, IAgileObject_Impl};
    use windows::Win32::System::Threading::SetEvent;

    use super::Event;

    /// Comparte el evento con quien espera: si la espera se rinde antes de
    /// que Windows avise, el handle sigue abierto hasta que Windows suelte
    /// este objeto, así que nunca se señala un handle ya cerrado.
    #[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
    pub(super) struct Done(pub(super) Arc<Event>);

    impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
        fn ActivateCompleted(&self, _: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
            // SAFETY: el `Arc` mantiene el evento abierto mientras exista `self`.
            unsafe { SetEvent(self.0 .0) }
        }
    }
    impl IAgileObject_Impl for Done_Impl {}
}

/// PID del proceso navegador de WebView2 (lo rellena `main.rs`). Windows no
/// cuenta ese proceso dentro del árbol de MusicRG para el process loopback,
/// así que se captura su árbol directamente: sigue siendo solo audio de MusicRG.
pub static BROWSER_PID: AtomicU32 = AtomicU32::new(0);

fn target_pid() -> u32 {
    match BROWSER_PID.load(Ordering::Relaxed) {
        // SAFETY: sin argumentos ni efectos; siempre es válida.
        0 => unsafe { GetCurrentProcessId() },
        pid => pid,
    }
}

/// # Safety
/// COM tiene que estar iniciado en el hilo que llama.
unsafe fn process_loopback(event: HANDLE, source: Source) -> windows::core::Result<(IAudioClient, Format)> {
    let mode = match source {
        Source::MusicRG => PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        Source::Others => PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    };
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: target_pid(),
                ProcessLoopbackMode: mode,
            },
        },
    };
    // ManuallyDrop: el Drop de PROPVARIANT llama a `PropVariantClear`, que
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
                        pBlobData: (&raw const params).cast_mut().cast(),
                    },
                },
            }),
        },
    });
    let done = Arc::new(Event::new()?);
    let handler: IActivateAudioInterfaceCompletionHandler = done::Done(done.clone()).into();
    // `params` y `pv` siguen vivos en la pila hasta que vuelve la llamada,
    // que es cuando Windows deja de leerlos.
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&raw const *pv),
        &handler,
    )?;
    let _ = WaitForSingleObject(done.0, 5000);
    let mut hr = HRESULT(0);
    let mut unk = None;
    op.GetActivateResult(&raw mut hr, &raw mut unk)?;
    hr.ok()?;
    let client: IAudioClient = unk.ok_or_else(windows::core::Error::empty)?.cast()?;

    // Process loopback no admite `GetMixFormat`: se pide un formato fijo.
    let fmt = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 2,
        nSamplesPerSec: 44_100,
        wBitsPerSample: 16,
        nBlockAlign: 4,
        nAvgBytesPerSec: 44_100 * 4,
        cbSize: 0,
    };
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        2_000_000,
        0,
        &raw const fmt,
        None,
    )?;
    client.SetEventHandle(event)?;
    Ok((client, Format { channels: 2, sample: Sample::I16, rate: 44_100.0 }))
}

/// # Safety
/// COM tiene que estar iniciado en el hilo que llama.
unsafe fn endpoint_loopback() -> windows::core::Result<(IAudioClient, Format)> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let pf = client.GetMixFormat()?;
    // `WAVEFORMATEX` es `packed`: se copia entero en vez de tomar referencias.
    let f = pf.read_unaligned();
    let sub = (f.wFormatTag == EXTENSIBLE).then(|| pf.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned().SubFormat.data1);
    let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, pf, None);
    CoTaskMemFree(Some(pf.cast_const().cast()));
    r?;
    let sample = Sample::from_wave(f.wFormatTag, f.wBitsPerSample, sub)
        .ok_or_else(|| windows::core::Error::new(AUDCLNT_E_UNSUPPORTED_FORMAT, "formato de mezcla no soportado"))?;
    Ok((client, Format { channels: usize::from(f.nChannels.max(1)), sample, rate: f.nSamplesPerSec as f32 }))
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
        let window = (0..N).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (N as f32 - 1.0)).cos()).collect();
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
    use super::Sample;

    #[test]
    fn reads_samples_unaligned() {
        assert!((Sample::F32.read(&0.5f32.to_le_bytes()) - 0.5).abs() < 1e-6);
        assert!((Sample::I16.read(&i16::MIN.to_le_bytes()) + 1.0).abs() < 1e-6);
        assert!((Sample::I24.read(&[0, 0, 0x40]) - 0.5).abs() < 1e-6);
        assert!((Sample::I32.read(&(i32::MAX / 2 + 1).to_le_bytes()) - 0.5).abs() < 1e-6);
        assert!(Sample::F32.read(&[1, 2]).abs() < f32::EPSILON); // tamaño incorrecto => silencio
    }

    #[test]
    fn picks_sample_format() {
        assert_eq!(Sample::from_wave(3, 32, None), Some(Sample::F32));
        assert_eq!(Sample::from_wave(1, 16, None), Some(Sample::I16));
        assert_eq!(Sample::from_wave(0xFFFE, 32, Some(3)), Some(Sample::F32));
        assert_eq!(Sample::from_wave(0xFFFE, 32, Some(1)), Some(Sample::I32));
        assert_eq!(Sample::from_wave(0xFFFE, 24, Some(1)), Some(Sample::I24));
        assert_eq!(Sample::from_wave(1, 8, None), None);
        assert_eq!(Sample::from_wave(0xFFFE, 32, None), None);
    }

    #[test]
    #[ignore = "necesita un dispositivo de audio real"]
    fn capture_starts_and_stops() {
        let c = super::Capture::default();
        c.start(super::Source::MusicRG);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        c.stop();
        std::thread::sleep(std::time::Duration::from_millis(300));
        c.start(super::Source::MusicRG);
        std::thread::sleep(std::time::Duration::from_millis(500));
        c.stop();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
