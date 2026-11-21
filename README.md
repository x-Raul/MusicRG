# MusicRG

Reproductor de Apple Music para Windows, ligero y con tema propio.

MusicRG abre `music.apple.com` en una ventana nativa usando **WebView2**, el motor Chromium que ya viene con Windows. No lleva un navegador dentro: el ejecutable pesa unos 3 MB, el instalador alrededor de 1 MB, y la app usa el mismo motor que tiene el sistema.

Encima de la web oficial añade lo que le falta para sentirse como una app de escritorio: icono en la bandeja, controles desde ahí, una sola instancia, barra de título del color de la app, un tema oscuro que puedes cambiar y varios ajustes para gastar menos CPU, GPU y RAM.

> No está afiliado a Apple. Necesitas una suscripción a Apple Music.

---

## Índice

- [Requisitos](#requisitos)
- [Instalar](#instalar)
- [Uso](#uso)
- [Personalizar el diseño](#personalizar-el-diseño)
- [Qué hace para gastar poco](#qué-hace-para-gastar-poco)
- [Cómo funciona](#cómo-funciona)
- [Estructura del proyecto](#estructura-del-proyecto)
- [Compilar desde el código](#compilar-desde-el-código)
- [Integración continua y releases](#integración-continua-y-releases)
- [Datos que guarda en tu PC](#datos-que-guarda-en-tu-pc)
- [Problemas frecuentes](#problemas-frecuentes)
- [Limitaciones](#limitaciones)

---

## Requisitos

- Windows 10 o Windows 11 (x64).
- WebView2 Runtime. Windows 11 ya lo trae. En Windows 10, el instalador lo descarga si falta.
- Una cuenta de Apple Music.

## Instalar

Hay dos maneras:

1. **Instalador** (`MusicRG_<versión>_x64-setup.exe`): lo encuentras en **Releases** cuando hay una versión etiquetada, o en la pestaña **Actions** del repositorio, dentro del artefacto `MusicRG-windows` de cualquier ejecución. Instala la app para tu usuario y crea un acceso en el menú Inicio.
2. **Ejecutable suelto** (`musicrg.exe`): viene en el mismo artefacto. Funciona sin instalar nada, aunque necesita que WebView2 ya esté en el sistema.

Para desinstalarla: *Configuración → Aplicaciones → MusicRG → Desinstalar*.

## Uso

| Acción | Qué pasa |
| --- | --- |
| Abrir la app | Carga Apple Music. Inicias sesión con tu Apple ID como en la web, y la sesión se queda guardada. |
| Cerrar la ventana (✕) | La ventana se esconde en la bandeja del sistema y **la música sigue sonando**. |
| Clic izquierdo en el icono de la bandeja | Vuelve a mostrar la ventana y la pone delante. |
| Clic derecho en el icono de la bandeja | Abre un menú con **Reproducir / Pausar**, **Siguiente**, **Anterior**, **Mostrar MusicRG** y **Salir**. |
| Volver a abrir MusicRG con la app ya en marcha | No arranca otra copia: muestra la que ya estaba abierta. |
| Teclas multimedia / control de medios de Windows | Funcionan, porque la web de Apple Music usa la Media Session API. |
| Salir del todo | Clic derecho en la bandeja → **Salir**. |

La ventana abre a 1180×760 y se puede encoger hasta 420×320, que sirve como minirreproductor.

## Personalizar el diseño

Sin recompilar nada:

1. Crea el archivo `%APPDATA%\MusicRG\theme.css`. Puedes pegar esa ruta en el Explorador; la carpeta `MusicRG` la creas tú.
2. Escribe tu CSS dentro.
3. Reinicia MusicRG (bandeja → **Salir** y vuelve a abrirla).

Tu archivo se aplica **después** del tema incluido, así que lo que pongas ahí manda. Por ejemplo, para cambiar el color de acento:

```css
:root {
  --rg-accent: #7c5cff;
}
```

Variables del tema:

| Variable | Valor por defecto | Para qué sirve |
| --- | --- | --- |
| `--rg-accent` | `#ff4d6d` | Color de acento: botones, selección, foco, controles. |
| `--rg-bg` | `#0b0b0f` | Fondo de la página y de la barra lateral. |
| `--rg-surface` | `#14141a` | Fondo de la barra del reproductor. |
| `--rg-line` | `rgba(255,255,255,0.07)` | Líneas separadoras. |

El tema base está en [`src-tauri/inject/theme.css`](src-tauri/inject/theme.css). Lo que hace es dar valor a las variables propias de Apple Music (`--keyColor`, `--pageBG`, `--navSidebarBG`, `--playerBackground`) a partir de las de arriba. Además pone barras de scroll finas y oculta los avisos de «abrir en la app».

> **Color de la barra de título:** la barra nativa de Windows se pinta desde Rust, no desde el CSS. Si cambias `--rg-bg` y quieres que la barra siga igual, cambia también la constante `BG` en [`src-tauri/src/main.rs`](src-tauri/src/main.rs) y recompila.

## Qué hace para gastar poco

- **Usa el WebView2 del sistema** en vez de llevar su propio Chromium, así que no hay otros 150 MB de navegador en disco ni en memoria.
- **Apaga servicios de Chromium que un reproductor no usa**: traducción, casting (Media Router), sugerencias de optimización, compras y colecciones de Edge, SmartScreen, informes de fiabilidad de dominio y pings de auditoría.
- **Limita los procesos de renderizado a dos** (`--renderer-process-limit=2`).
- **Quita los desenfoques de fondo** (`backdrop-filter`), que son lo más caro de pintar en la GPU.
- **Con la ventana sin foco u oculta** pausa las animaciones CSS, desactiva las transiciones y para los vídeos silenciados, que son las carátulas animadas. El audio no se toca. Cuando la ventana vuelve a tener foco, todo se reanuda.
- **En la bandeja** pide a WebView2 que baje su nivel de memoria (`MemoryUsageTargetLevel = Low`), para que suelte cachés y lo que no hace falta mientras solo suena música.
- **Una sola instancia**, así que nunca hay dos copias cargando la web a la vez.
- **Binario optimizado por tamaño**: `opt-level = "s"`, LTO, una sola unidad de compilación, `panic = "abort"` y símbolos eliminados.

Lo que **no** se desactiva a propósito: las actualizaciones de componentes de Chromium. Por ahí llega Widevine (el DRM), y sin él Apple Music no reproduce nada.

## Cómo funciona

```
┌──────────────────────── musicrg.exe (Rust + Tauri 2) ────────────────────────┐
│                                                                              │
│  main.rs                                                                     │
│   ├─ Ventana WebView2 → https://music.apple.com/                             │
│   │    ├─ flags de Chromium (BROWSER_ARGS)                                   │
│   │    ├─ fondo y barra de título del color BG (DWM, Windows 11)             │
│   │    └─ script de inicio: shell.js + theme.css (+ tu theme.css)            │
│   ├─ Icono de bandeja + menú  ──eval──►  window.__rg.cmd('toggle'|'next'…)   │
│   ├─ Cerrar → ocultar + memoria baja; mostrar → memoria normal               │
│   └─ Plugin single-instance → segunda ejecución = mostrar la primera         │
│                                                                              │
│  Dentro de la página (shell.js)                                              │
│   ├─ Inyecta el CSS como hoja construida (adoptedStyleSheets)                │
│   ├─ blur/focus/visibilitychange → clase .rg-idle + pausa vídeos mudos       │
│   └─ window.__rg.cmd() → MusicKit.getInstance().play/pause/skip…            │
└──────────────────────────────────────────────────────────────────────────────┘
```

Paso a paso:

1. **Arranque.** `main.rs` lee el tema incluido en el binario (`include_str!`) y, si existe, `%APPDATA%\MusicRG\theme.css`. Junta los dos y mete el resultado dentro de `shell.js`, en el lugar de `__RG_CSS__`.
2. **Ventana.** Crea una ventana WebView2 que apunta a `https://music.apple.com/`, con tema oscuro, fondo `BG` (para que no haya destello blanco al cargar), los flags de Chromium y `shell.js` como *initialization script*, que se ejecuta antes que la página en cada navegación.
3. **Barra de título.** En Windows 11 llama a `DwmSetWindowAttribute(DWMWA_CAPTION_COLOR)` para pintar la barra nativa del mismo color que el fondo. En Windows 10 la llamada falla sin hacer ruido y la barra se queda oscura.
4. **Tema.** `shell.js` aplica el CSS con `new CSSStyleSheet()` + `document.adoptedStyleSheets`. Así no lo bloquea la Content Security Policy de Apple y no añade nodos al DOM.
5. **Bandeja.** Las opciones de reproducción del menú ejecutan `window.__rg.cmd(...)` en la página, que llama a la API oficial de **MusicKit JS** que ya usa la web (`play`, `pause`, `skipToNextItem`, `skipToPreviousItem`).
6. **Cerrar / mostrar.** Cerrar se intercepta (`prevent_close`), la ventana se oculta y WebView2 pasa a memoria baja. Al mostrarla vuelve a memoria normal y recupera el foco.

## Estructura del proyecto

```
MusicRG/
├─ package.json              # Solo trae @tauri-apps/cli y los scripts dev/build
├─ package-lock.json
├─ .github/workflows/
│  └─ build.yml              # CI: compila en Windows, sube artefactos y crea releases
└─ src-tauri/
   ├─ Cargo.toml             # Dependencias Rust y perfil release optimizado
   ├─ Cargo.lock
   ├─ build.rs               # tauri-build
   ├─ tauri.conf.json        # Nombre, versión, identificador, bundle NSIS, WebView2
   ├─ icons/
   │  ├─ icon.ico            # Icono de la app y de la bandeja
   │  └─ icon.svg            # Fuente del icono
   ├─ inject/
   │  ├─ theme.css           # Tema que se inyecta en music.apple.com
   │  └─ shell.js            # Script de página: CSS, modo reposo, controles
   └─ src/
      └─ main.rs             # Toda la lógica nativa
```

No hay frontend propio. La interfaz es la web de Apple Music y el proyecto solo añade la parte nativa y lo que se inyecta en la página.

### Dependencias

| Crate | Uso |
| --- | --- |
| `tauri` 2 (feature `tray-icon`) | Ventana, WebView2, bandeja, menús. |
| `tauri-plugin-single-instance` | Que solo haya una instancia. |
| `serde_json` | Escapar el CSS para meterlo en el JS. |
| `webview2-com` | Acceso a `ICoreWebView2_19` para el nivel de memoria. |
| `windows` / `windows-core` | `DwmSetWindowAttribute` para el color de la barra de título. |

## Compilar desde el código

### Requisitos

- [Rust](https://rustup.rs) estable, con la toolchain `x86_64-pc-windows-msvc`:
  ```powershell
  winget install Rustlang.Rustup
  ```
- **Visual Studio Build Tools** con la carga de trabajo «Desarrollo para el escritorio con C++» (MSVC + Windows SDK).
- **Node.js** 18 o superior, más [pnpm](https://pnpm.io).

### Comandos

Con `pnpm dlx` no hace falta instalar dependencias de Node: descarga la CLI de Tauri y la ejecuta.

```powershell
# Desarrollo: compila en modo debug y abre la app (con consola para ver logs)
pnpm --package=@tauri-apps/cli@2 dlx tauri dev

# Release: ejecutable + instalador NSIS
pnpm --package=@tauri-apps/cli@2 dlx tauri build
```

Resultado:

| Archivo | Qué es |
| --- | --- |
| `src-tauri/target/release/musicrg.exe` | Ejecutable suelto (~3 MB). |
| `src-tauri/target/release/bundle/nsis/MusicRG_<versión>_x64-setup.exe` | Instalador (~1 MB). |

La primera compilación tarda unos minutos porque compila todas las dependencias. Las siguientes son mucho más rápidas.

> El repositorio trae `package-lock.json` (npm) y la CI usa `npm install && npm run build`. Esos comandos también funcionan en local.

### Cambiar la versión

La versión está en dos sitios y conviene que coincidan:

- `src-tauri/tauri.conf.json` → `"version"` (es la que usa el instalador)
- `src-tauri/Cargo.toml` → `version`

## Integración continua y releases

[`.github/workflows/build.yml`](.github/workflows/build.yml) se ejecuta en **cada push** y también a mano (*workflow_dispatch*), en `windows-latest`:

1. Instala Node 22 y Rust estable, y cachea `src-tauri/target`.
2. Ejecuta `npm install` y `npm run build`.
3. Sube `musicrg.exe` y el instalador como artefacto **`MusicRG-windows`**.
4. Si el push es una **etiqueta `v*`** (por ejemplo `v0.2.0`), crea una **GitHub Release** con el instalador adjunto.

Para publicar una versión:

```powershell
git tag v0.2.0
```

```powershell
git push origin v0.2.0
```

## Datos que guarda en tu PC

| Ruta | Contenido |
| --- | --- |
| `%LOCALAPPDATA%\com.xraul.musicrg\` | Perfil de WebView2: cookies, sesión de Apple ID, caché. |
| `%APPDATA%\MusicRG\theme.css` | Tu tema personalizado (solo si lo creas). |

Para **cerrar sesión del todo** o empezar de cero: sal de MusicRG desde la bandeja y borra `%LOCALAPPDATA%\com.xraul.musicrg\`. Desinstalar la app no borra estas carpetas.

## Problemas frecuentes

**La ventana sale en blanco o la app no abre.**
Falta WebView2 Runtime. Instálalo desde la página oficial de Microsoft («WebView2 Runtime Evergreen») o usa el instalador de MusicRG, que lo descarga.

**No suena nada / error al reproducir.**
Apple Music necesita Widevine (DRM), que WebView2 descarga como componente la primera vez. Espera un minuto con la app abierta, reiníciala y comprueba que no haya un firewall o proxy bloqueando las descargas de componentes de Edge.

**Los controles de la bandeja no hacen nada.**
Funcionan cuando la página ya ha cargado MusicKit, es decir, después de iniciar sesión y de que Apple Music termine de cargar. Antes de eso no hacen nada.

**Mi `theme.css` no se aplica.**
Comprueba que la ruta es exactamente `%APPDATA%\MusicRG\theme.css` y que has **salido** de la app desde la bandeja (cerrar la ventana solo la esconde). Si una regla no tiene efecto, añádele `!important`, porque muchos estilos de Apple Music son muy específicos.

**La barra de título no tiene el color del fondo.**
Eso solo funciona en Windows 11. En Windows 10 se queda la barra oscura del sistema.

**Al abrir MusicRG otra vez no aparece una segunda ventana.**
Es a propósito: solo hay una instancia y la que estaba abierta pasa al frente.

## Limitaciones

- Depende de la web de Apple Music. Si Apple cambia sus clases o variables CSS, parte del tema puede dejar de aplicarse. La reproducción no se rompe porque usa MusicKit.
- Solo Windows. El código compila en otros sistemas, pero el color de la barra de título y el ahorro de memoria solo están hechos para Windows.
- No hay descargas sin conexión ni letras sincronizadas fuera de lo que ya ofrece la web.
