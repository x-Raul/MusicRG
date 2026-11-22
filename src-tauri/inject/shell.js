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
  // Lo que hay en "Destacar", en el orden de la barra lateral. Rust solo
  // recibe los nombres; al pulsar uno en la bandeja pide la posición.
  let pins = [];
  window.__rg = {
    cmd(name) {
      const m = kit();
      if (!m) return;
      if (name === "toggle") return m.isPlaying ? m.pause() : m.play();
      if (name === "next") return m.skipToNextItem();
      if (name === "prev") return m.skipToPreviousItem();
    },
    playPin(i) {
      const m = kit();
      const p = pins[i];
      if (!m || !p) return;
      m.setQueue({ [p.kind]: p.id, startPlaying: true }).catch(() => {});
    },
  };

  // La lista sale de la API de la biblioteca (la misma que usa la página).
  // Se vuelve a pedir solo cuando cambia lo que se ve en Destacar.
  let pinsKey = null;
  let pinsSent = null;
  let pinsWait = 0;
  const loadPins = async () => {
    const m = kit();
    if (!m || !m.isAuthorized) {
      // MusicKit aún cargando (o sin sesión): se reintenta un rato.
      if (pinsWait++ < 40) setTimeout(loadPins, 3000);
      return;
    }
    pinsWait = 0;
    try {
      const r = await m.api.music("/v1/me/library/pins");
      pins = ((r && r.data && r.data.data) || [])
        .map((d) => {
          const at = d.attributes || {};
          const pp = at.playParams || {};
          return { name: at.name || "", kind: pp.kind, id: pp.id };
        })
        .filter((p) => p.name && p.id && ["playlist", "album", "song", "station"].includes(p.kind));
    } catch (_) {
      return;
    }
    const names = pins.map((p) => p.name);
    const key = JSON.stringify(names);
    if (key === pinsSent) return;
    pinsSent = key;
    try {
      const ipc = window.__TAURI_INTERNALS__;
      if (ipc) ipc.invoke("pins", { names }).catch(() => {});
    } catch (_) {}
  };
  // La página cambia el DOM sin parar: como mucho una comprobación por segundo.
  let pinsCheck = 0;
  const watchPins = () => {
    if (pinsCheck) return;
    pinsCheck = setTimeout(() => {
      pinsCheck = 0;
      const folder = document.querySelector('[data-testid="library.pins.folder"]');
      const li = folder && folder.closest("li");
      const key = li ? li.textContent : "";
      if (key === pinsKey) return;
      pinsKey = key;
      loadPins();
    }, 1000);
  };
  new MutationObserver(watchPins).observe(document, { childList: true, subtree: true });

  // Ventana sin barra de título: botones propios arriba a la derecha y la
  // franja superior de la página sirve para arrastrar (doble clic = maximizar).
  const ctl = (action) => {
    try {
      const ipc = window.__TAURI_INTERNALS__;
      if (ipc) ipc.invoke("window_ctl", { action }).catch(() => {});
    } catch (_) {}
  };
  const DRAG_H = 32;
  let host = null;
  const mountChrome = () => {
    if (host && host.isConnected) return;
    host = document.createElement("div");
    host.style.cssText = "position:fixed;top:0;right:0;z-index:2147483647;display:flex;";
    // Shadow DOM: los estilos de Apple no le afectan y viceversa.
    const root = host.attachShadow({ mode: "closed" });
    const css = new CSSStyleSheet();
    css.replaceSync(`
      button { all: unset; width: 46px; height: ${DRAG_H}px; display: grid; place-items: center;
        font: 10px "Segoe Fluent Icons", "Segoe MDL2 Assets"; color: rgba(255,255,255,.85); }
      button:hover { background: rgba(255,255,255,.08); }
      button:active { background: rgba(255,255,255,.05); }
      button.x:hover { background: #c42b1c; color: #fff; }
      button.x:active { background: #b22a1b; }
    `);
    root.adoptedStyleSheets = [css];
    const btn = (action, glyph, label, cls) => {
      const b = document.createElement("button");
      b.textContent = glyph;
      b.title = label;
      b.setAttribute("aria-label", label);
      if (cls) b.className = cls;
      b.addEventListener("click", () => ctl(action));
      root.append(b);
      return b;
    };
    btn("minimize", "", "Minimize");
    const max = btn("maximize", "", "Maximize");
    btn("close", "", "Close", "x");
    const sync = () => {
      const full = outerWidth >= screen.availWidth && outerHeight >= screen.availHeight;
      max.textContent = full ? "" : "";
      max.title = full ? "Restore" : "Maximize";
    };
    addEventListener("resize", sync);
    sync();
    document.documentElement.append(host);
  };
  if (document.documentElement) mountChrome();
  document.addEventListener("DOMContentLoaded", mountChrome);
  document.addEventListener(
    "mousedown",
    (e) => {
      if (e.button !== 0 || e.clientY >= DRAG_H || e.target === host) return;
      const t = e.target instanceof Element ? e.target : null;
      if (t && t.closest("a,button,input,textarea,select,label,[role=button],[role=link],[role=slider],[contenteditable]")) return;
      e.preventDefault();
      ctl(e.detail === 2 ? "maximize" : "drag");
    },
    true,
  );

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
