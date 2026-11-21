# MusicRG

Reproductor de Apple Music para Windows, ligero y con tema propio.

Carga `music.apple.com` dentro de WebView2 (el motor Chromium que Windows ya trae instalado), así que no empaqueta un navegador: el ejecutable pesa unos 3 MB y comparte el motor con el sistema.

## Instalar

Descarga el instalador desde la pestaña **Actions** del repo (artefacto `MusicRG-windows`) o desde **Releases** si hay una versión etiquetada.

Windows 10 y 11. En Windows 11 WebView2 ya viene instalado; en Windows 10 el instalador lo descarga si falta.

## Uso

- Inicia sesión con tu Apple ID como en la web; la sesión se guarda.
- Cerrar la ventana la esconde en la bandeja y la música sigue. Clic en el icono para volver.
- Clic derecho en el icono de la bandeja: reproducir/pausar, siguiente, anterior, salir.
- Las teclas multimedia del teclado y el control de medios de Windows funcionan.

## Personalizar el diseño

Crea `%APPDATA%\MusicRG\theme.css` y reinicia la app. Se aplica encima del tema incluido. Por ejemplo, para cambiar el color de acento:

```css
:root {
  --rg-accent: #7c5cff;
}
```

El tema base está en [`src-tauri/inject/theme.css`](src-tauri/inject/theme.css). Si cambias `--rg-bg` ahí, cambia también `BG` en `src-tauri/src/main.rs` para que la barra de título siga del mismo color.

## Qué hace para gastar poco

- Usa el WebView2 del sistema en vez de traer su propio Chromium.
- Apaga servicios de Chromium que un reproductor no usa (traducción, casting, sugerencias, etc.).
- Quita los desenfoques de fondo, que son lo más caro de pintar.
- Con la ventana sin foco, pausa animaciones y carátulas animadas.
- En la bandeja, pide a WebView2 que libere memoria (nivel de memoria bajo).
- Una sola instancia: abrirla otra vez trae al frente la que ya existe.

## Compilar

Necesitas [Rust](https://rustup.rs) y Node.js en Windows.

```
npm install
npm run dev     # desarrollo
npm run build   # instalador en src-tauri/target/release/bundle/nsis
```

No está afiliado a Apple. Necesitas una suscripción a Apple Music.
