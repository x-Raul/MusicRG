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
})();
