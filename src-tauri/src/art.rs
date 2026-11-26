//! Descarga y decodifica la carátula con lo que ya trae Windows (`URLMon` + WIC):
//! sin crates de HTTP ni de imagen. Corre en un hilo corto, una vez por canción.
//! Las carátulas de otras apps llegan ya como bytes (ver `media.rs`).

use windows::core::{Interface, HSTRING};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::Urlmon::URLOpenBlockingStreamW;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IStream, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::SHCreateMemStream;

/// Solo se descargan imágenes de los servidores de Apple; la URL viene de la
/// página y no queremos que pueda pedir cualquier cosa.
pub fn allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    ["mzstatic.com", "apple.com"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Devuelve `px`×`px` píxeles BGRA premultiplicados.
pub fn fetch(url: &str, px: u32) -> Option<Vec<u8>> {
    if !allowed(url) || px == 0 {
        return None;
    }
    // SAFETY: COM se inicia y se cierra en este mismo hilo, emparejado; los
    // objetos COM que usa `decode` se sueltan antes de `CoUninitialize`.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        let out = decode(url, px);
        CoUninitialize();
        out
    }
}

/// Igual que `fetch`, pero con la imagen ya en memoria (PNG, JPEG...).
pub fn decode_bytes(bytes: &[u8], px: u32) -> Option<Vec<u8>> {
    if bytes.is_empty() || px == 0 {
        return None;
    }
    // SAFETY: igual que en `fetch`; `SHCreateMemStream` copia `bytes`, así
    // que el stream no depende del préstamo.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        let out = SHCreateMemStream(Some(bytes)).and_then(|s| decode_stream(&s, px));
        CoUninitialize();
        out
    }
}

/// # Safety
/// COM tiene que estar iniciado en el hilo que llama.
unsafe fn decode(url: &str, px: u32) -> Option<Vec<u8>> {
    let mut stream: Option<IStream> = None;
    URLOpenBlockingStreamW(None, &HSTRING::from(url), &raw mut stream, 0, None).ok()?;
    decode_stream(&stream?, px)
}

/// # Safety
/// COM tiene que estar iniciado en el hilo que llama.
unsafe fn decode_stream(stream: &IStream, px: u32) -> Option<Vec<u8>> {
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
    let dec = wic.CreateDecoderFromStream(stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
    let frame = dec.GetFrame(0).ok()?;
    // Recorte cuadrado centrado: las miniaturas de vídeo son 16:9 y se
    // deformarían al escalarlas a un cuadrado.
    let (mut w, mut h) = (0u32, 0u32);
    frame.GetSize(&raw mut w, &raw mut h).ok()?;
    let src: IWICBitmapSource = if w != h && w > 0 && h > 0 {
        let side = w.min(h);
        let rect =
            WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        let clip = wic.CreateBitmapClipper().ok()?;
        clip.Initialize(&frame, &raw const rect).ok()?;
        clip.cast().ok()?
    } else {
        frame.cast().ok()?
    };
    let scaler = wic.CreateBitmapScaler().ok()?;
    scaler.Initialize(&src, px, px, WICBitmapInterpolationModeFant).ok()?;
    let conv = wic.CreateFormatConverter().ok()?;
    conv.Initialize(
        &scaler,
        &GUID_WICPixelFormat32bppPBGRA,
        WICBitmapDitherTypeNone,
        None,
        0.0,
        WICBitmapPaletteTypeCustom,
    )
    .ok()?;
    let mut buf = vec![0u8; (px * px * 4) as usize];
    conv.CopyPixels(std::ptr::null(), px * 4, &mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::allowed;

    #[test]
    fn only_apple_hosts() {
        assert!(allowed("https://is1-ssl.mzstatic.com/image/thumb/a/96x96bb.jpg"));
        assert!(allowed("https://music.apple.com/x.jpg"));
        assert!(!allowed("http://is1-ssl.mzstatic.com/a.jpg"));
        assert!(!allowed("https://evil.com/mzstatic.com/a.jpg"));
        assert!(!allowed("https://mzstatic.com.evil.com/a.jpg"));
        assert!(!allowed("file:///C:/x.jpg"));
    }
}

#[cfg(test)]
mod net_tests {
    #[test]
    #[ignore = "descarga una imagen real de internet"]
    fn decodes_real_artwork() {
        let url = "https://is1-ssl.mzstatic.com/image/thumb/Music125/v4/7e/0e/1b/7e0e1b4a-0f0e-4d2b-8f46-2c1c8bfbd0cf/source/96x96bb.jpg";
        let px = super::fetch(url, 48);
        eprintln!("got {:?}", px.as_ref().map(Vec::len));
    }
}
