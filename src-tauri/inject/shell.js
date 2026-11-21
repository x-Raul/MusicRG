// Se ejecuta antes que la página en cada navegación. __RG_CSS__ lo sustituye Rust.
(() => {
  if (window.top !== window || window.__rg) return;

  // Hoja construida: no la bloquea la CSP de la página y no toca el DOM.
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(__RG_CSS__);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  } catch (_) {}

  // Ventana sin foco u oculta => se pausan animaciones y vídeos mudos
  // (las carátulas animadas); el audio no se toca.
  const paused = new Set();
  const setIdle = (idle) => {
    const root = document.documentElement;
    if (!root) return;
    root.classList.toggle("rg-idle", idle);
    if (idle) {
      for (const v of document.querySelectorAll("video")) {
        if (v.muted && !v.paused) {
          v.pause();
          paused.add(v);
        }
      }
    } else {
      for (const v of paused) if (v.isConnected) v.play().catch(() => {});
      paused.clear();
    }
  };
  addEventListener("blur", () => setIdle(true));
  addEventListener("focus", () => setIdle(false));
  document.addEventListener("visibilitychange", () => setIdle(document.hidden || !document.hasFocus()));

  // Controles para el menú de la bandeja.
  const kit = () => {
    try {
      return window.MusicKit && window.MusicKit.getInstance();
    } catch (_) {
      return null;
    }
  };
  window.__rg = {
    cmd(name) {
      const m = kit();
      if (!m) return;
      if (name === "toggle") return m.isPlaying ? m.pause() : m.play();
      if (name === "next") return m.skipToNextItem();
      if (name === "prev") return m.skipToPreviousItem();
    },
  };

  // "Sonando ahora" para el widget de la barra de tareas (lo dibuja Rust).
  // Todo por eventos, nada de sondeo:
  // - play/pausa: eventos del propio <audio>/<video> (capturados en document,
  //   no dependen de MusicKit, que a veces recrea su reproductor);
  // - título/artista/carátula: MusicKit si está, si no la Media Session, y se
  //   vuelve a leer cada vez que la página cambia navigator.mediaSession.metadata.
  let last = "";
  let playing = false;
  const send = (msg) => {
    const key = JSON.stringify(msg);
    if (key === last) return;
    last = key;
    try {
      const ipc = window.__TAURI_INTERNALS__;
      if (ipc) ipc.invoke("now_playing", msg).catch(() => {});
    } catch (_) {}
  };
  const report = () => {
    const m = kit();
    const it = m && m.nowPlayingItem;
    const at = (it && it.attributes) || {};
    const meta = navigator.mediaSession && navigator.mediaSession.metadata;
    const title = (it && it.title) || at.name || (meta && meta.title) || "";
    const artist = (it && it.artistName) || at.artistName || (meta && meta.artist) || "";
    let art = "";
    const tpl = (it && it.artwork && it.artwork.url) || (at.artwork && at.artwork.url);
    if (tpl) art = tpl.replace("{w}", "96").replace("{h}", "96").replace("{f}", "jpg");
    else if (meta && meta.artwork && meta.artwork.length) art = meta.artwork[meta.artwork.length - 1].src;
    send({ playing: playing && !!title, title, artist, art });
  };
  // Apple Music usa varios elementos de audio (la canción y la siguiente
  // precargada): suena si CUALQUIERA de ellos suena, no el último que avisó.
  const media = new Set();
  const onMedia = (e) => {
    const el = e.target;
    if (!(el instanceof HTMLMediaElement)) return;
    media.add(el);
    let any = false;
    for (const x of media) {
      if (!x.isConnected && x.paused) media.delete(x);
      // Los vídeos mudos son carátulas animadas, no la música.
      else if (!x.muted && !x.paused && !x.ended) any = true;
    }
    playing = any;
    report();
  };
  for (const ev of ["playing", "pause", "ended", "emptied"]) {
    document.addEventListener(ev, onMedia, true);
  }
  try {
    const d = Object.getOwnPropertyDescriptor(MediaSession.prototype, "metadata");
    Object.defineProperty(MediaSession.prototype, "metadata", {
      configurable: true,
      enumerable: d.enumerable,
      get: d.get,
      set(v) {
        d.set.call(this, v);
        setTimeout(report, 0);
      },
    });
  } catch (_) {}
  addEventListener("pagehide", () => send({ playing: false, title: "", artist: "", art: "" }));
})();
