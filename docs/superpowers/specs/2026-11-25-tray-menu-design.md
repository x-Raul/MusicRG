# Menú de la bandeja: más ordenado y con iconos

**Tamaño:** cambio pequeño. Aprobado por el usuario el 2026-11-25 (opción A).

## Intención

El clic derecho en el icono de la bandeja tiene que verse mejor y más organizado, con iconos.
El rendimiento manda: el menú no puede costar memoria ni tiempo de apertura.

- **Objetivos:** agrupar las opciones, poner iconos en las acciones y mostrar arriba lo que suena.
- **Fuera de alcance:** la ventana de ajustes grande con estilo anatomy (siguiente paso, aparte).
- **Anti-objetivo:** cualquier cosa que añada un WebView, un hilo o memoria residente para el menú.

## Enfoques

- **A, elegido: menú nativo de Windows** (el de Tauri, `muda`), reorganizado con submenús e iconos.
  Coste cero, y Windows se encarga del teclado, el foco, los monitores y el tema claro/oscuro.
- **B, descartado por ahora:** menú propio dibujado con Direct2D. Da control total del aspecto, pero
  obliga a reimplementar el teclado, cerrar al hacer clic fuera, los submenús, el DPI y la
  accesibilidad. Es más trabajo y más fácil que algo falle.

## Diseño

```
♪  Título — Artista             (desactivado; "Nothing playing" si no suena nada)
──────────────────────
▶  Play / Pause
⏭  Next
⏮  Previous
📌 Pinned                     ›
──────────────────────
▣  Widget                     ›   ✓ Taskbar widget / ✓ Show other apps / ✓ Flat audio bars
   ✓ Caps Lock toast
   ✓ Keep PC awake
   ✓ Start with Windows
──────────────────────
⟳  Check for updates
⧉  Show MusicRG
⏻  Quit
```

- **Iconos:** dibujados para MusicRG (primero eran de Lucide; el usuario los prefirió con líneas
  rectas y sin redondeos): trazos de 1 px sobre la rejilla de 16 px, esquinas en inglete. Los SVG
  están en `src-tauri/icons/menu/` y `build.rs` los pasa a RGBA de 16×16 con `resvg` (dependencia solo de compilación),
  en dos tintas: clara para el menú oscuro y oscura para el claro. En ejecución no hay ningún
  decodificador: solo bytes incluidos en el binario.
- **Tema:** el menú sigue el modo de las apps de Windows (`AppsUseLightTheme`). Al pulsar con el
  botón derecho en el icono, antes de que se abra el menú, se mira ese valor y, si cambió, se
  cambian las tintas de los iconos.
- **Casillas:** en Windows la marca ✓ ocupa el hueco del icono, así que las opciones con casilla
  no llevan icono. Las tres del widget pasan a un submenú "Widget".
- **Cabecera:** muestra lo mismo que el widget: la canción de MusicRG si suena y, si no, la de
  otra app (si "Show other apps" está activo). Se recorta a 28 caracteres (como "Show other apps in widget"), para que el menú no sea más ancho que el anterior: el usuario lo pidió así tras verlo con 60.
- **Ajustes guardados:** no cambian; los ids de las casillas siguen siendo los mismos.

## Queda para quien lo construya

El trazo exacto de los iconos a 16 px y el texto de la cabecera cuando solo hay título.
