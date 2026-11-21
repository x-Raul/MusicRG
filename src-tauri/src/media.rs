//! Lo que suena en otras apps (el navegador, Spotify, VLC...), leído de los
//! controles multimedia de Windows: la misma fuente que el panel de volumen.
//!
//! Todo por eventos: Windows avisa cuando cambia una sesión, su canción o su
//! estado (play/pausa) y entonces se vuelve a mirar. Sin sondeo. La sesión de
//! la propia MusicRG se ignora: esa ya la cuenta la página (shell.js).

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::overlay::NowPlaying;

/// Carátula de la canción actual: (clave, bytes de la imagen tal cual).
/// El widget la decodifica al tamaño que necesita (art::decode_bytes).
static THUMB: Mutex<Option<(String, Vec<u8>)>> = Mutex::new(None);
/// Las claves de carátula de este módulo empiezan así (en vez de "https://").
pub const ART_PREFIX: &str = "media:";

pub fn thumbnail(key: &str) -> Option<Vec<u8>> {
    match &*THUMB.lock().unwrap() {
        Some((k, bytes)) if k == key => Some(bytes.clone()),
        _ => None,
    }
}

/// Lanza el hilo. `report` recibe cada cambio de lo que suena fuera de MusicRG.
pub fn spawn(report: impl Fn(NowPlaying) + Send + 'static) {
    let _ = std::thread::Builder::new().name("rg-media".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Err(e) = run(&report) {
            log(&format!("media: {e}"));
        }
    });
}

#[cfg(debug_assertions)]
fn log(s: &str) {
    eprintln!("[musicrg] {s}");
}
#[cfg(not(debug_assertions))]
fn log(_: &str) {}

/// Suscripciones a una sesión, para quitarlas cuando cambia la lista.
struct Hook {
    session: Session,
    playback: i64,
    media: i64,
}

fn run(report: &dyn Fn(NowPlaying)) -> windows::core::Result<()> {
    let mgr = Manager::RequestAsync()?.join()?;
    let (tx, rx) = channel::<bool>(); // true = cambió la lista de sesiones

    let t = tx.clone();
    mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(true);
        Ok(())
    }))?;
    let t = tx.clone();
    mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(false);
        Ok(())
    }))?;

    let mut hooks: Vec<Hook> = Vec::new();
    let mut last = NowPlaying::default();
    let mut rehook = true;
    loop {
        if rehook {
            for h in hooks.drain(..) {
                let _ = h.session.RemovePlaybackInfoChanged(h.playback);
                let _ = h.session.RemoveMediaPropertiesChanged(h.media);
            }
            if let Ok(list) = mgr.GetSessions() {
                for session in list {
                    let (a, b) = (tx.clone(), tx.clone());
                    let playback = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                        let _ = a.send(false);
                        Ok(())
                    }));
                    let media = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                        let _ = b.send(false);
                        Ok(())
                    }));
                    if let (Ok(playback), Ok(media)) = (playback, media) {
                        hooks.push(Hook { session, playback, media });
                    }
                }
            }
        }
        let now = evaluate(&mgr).unwrap_or_default();
        if now != last {
            log(&format!("media: playing={} {:?} - {:?}", now.playing, now.title, now.artist));
            report(now.clone());
            last = now;
        }
        rehook = wait(&rx)?;
    }
}

/// Espera al siguiente aviso y agrupa los que llegan seguidos (al cambiar de
/// canción Windows manda varios en pocos milisegundos).
fn wait(rx: &Receiver<bool>) -> windows::core::Result<bool> {
    let Ok(mut rehook) = rx.recv() else {
        return Err(windows::core::Error::empty());
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(r) => rehook |= r,
            Err(RecvTimeoutError::Timeout) => return Ok(rehook),
            Err(RecvTimeoutError::Disconnected) => return Err(windows::core::Error::empty()),
        }
    }
}

fn is_own(session: &Session) -> bool {
    let id = session.SourceAppUserModelId().unwrap_or_default().to_string().to_lowercase();
    id.contains("musicrg")
}

fn is_playing(session: &Session) -> bool {
    session
        .GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|s| s == Status::Playing)
        .unwrap_or(false)
}

/// La sesión que manda: la "actual" de Windows si está sonando; si no, la
/// primera que esté sonando. Si no suena nada, `playing: false`.
fn evaluate(mgr: &Manager) -> windows::core::Result<NowPlaying> {
    let mut candidates: Vec<Session> = Vec::new();
    if let Ok(cur) = mgr.GetCurrentSession() {
        candidates.push(cur);
    }
    for s in mgr.GetSessions()? {
        candidates.push(s);
    }
    #[cfg(debug_assertions)]
    for s in &candidates {
        log(&format!(
            "media: session {:?} playing={}",
            s.SourceAppUserModelId().unwrap_or_default().to_string(),
            is_playing(s)
        ));
    }
    let Some(session) = candidates.into_iter().find(|s| !is_own(s) && is_playing(s)) else {
        return Ok(NowPlaying::default());
    };
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
    let mut artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
    if artist.is_empty() {
        artist = props.AlbumArtist().map(|s| s.to_string()).unwrap_or_default();
    }
    if title.is_empty() {
        return Ok(NowPlaying::default());
    }
    let app = session.SourceAppUserModelId().unwrap_or_default();
    let key = format!("{ART_PREFIX}{app}|{title}|{artist}");
    let art = if thumbnail(&key).is_some() {
        key
    } else {
        match props.Thumbnail().ok().and_then(|t| read_all(&t.OpenReadAsync().ok()?.join().ok()?)) {
            Some(bytes) => {
                *THUMB.lock().unwrap() = Some((key.clone(), bytes));
                key
            }
            None => String::new(),
        }
    };
    Ok(NowPlaying { playing: true, title: clip(title), artist: clip(artist), art })
}

fn read_all(stream: &windows::Storage::Streams::IRandomAccessStreamWithContentType) -> Option<Vec<u8>> {
    let size = stream.Size().ok()?;
    if size == 0 || size > 8 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(stream).ok()?;
    let n = reader.LoadAsync(size as u32).ok()?.join().ok()?;
    let mut buf = vec![0u8; n as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

fn clip(s: String) -> String {
    s.chars().take(300).collect()
}

