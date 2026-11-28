# MusicRG

Reproductor de Apple Music para Windows, ligero y con tema propio.

MusicRG abre `music.apple.com` en una ventana nativa usando **WebView2**, el motor Chromium que ya viene con Windows. No lleva un navegador dentro: el ejecutable pesa unos 4 MB, el instalador alrededor de 1,4 MB, y la app usa el mismo motor que tiene el sistema.

Encima de la web oficial añade lo que le falta para sentirse como una app de escritorio: icono en la bandeja, controles desde ahí, una sola instancia, barra de título del color de la app, un tema oscuro que puedes cambiar y varios ajustes para gastar menos CPU, GPU y RAM.

Además trae un **widget sobre la barra de tareas** con la canción que suena y unas barras que siguen el audio real, y un **aviso de Bloq Mayús** al activarlo o desactivarlo. Los dos son nativos (Win32 + Direct2D), no otro navegador.

> No está afiliado a Apple. Necesitas una suscripción a Apple Music.

[![Descargar MusicRG](https://img.shields.io/github/v/release/x-Raul/MusicRG?label=Descargar&logo=windows&style=for-the-badge)](https://github.com/x-Raul/MusicRG/releases/latest)

**[⬇ Descargar la última versión](https://github.com/x-Raul/MusicRG/releases/latest)**: en la página del release, baja `MusicRG_<versión>_x64-setup.exe` (en *Assets*) y ábrelo.

---

## Índice

- [Requisitos](#requisitos)
- [Instalar](#instalar)
- [Uso](#uso)
- [Widget de la barra de tareas y aviso de Bloq Mayús](#widget-de-la-barra-de-tareas-y-aviso-de-bloq-mayús)
- [Actualizaciones](#actualizaciones)
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

1. **Instalador** (`MusicRG_<versión>_x64-setup.exe`): lo encuentras en **[Releases](https://github.com/x-Raul/MusicRG/releases/latest)**, o en la pestaña **Actions** del repositorio, dentro del artefacto `MusicRG-windows` de cualquier ejecución. Instala la app para tu usuario y crea un acceso en el menú Inicio.
2. **Ejecutable suelto** (`musicrg.exe`): viene en el mismo artefacto. Funciona sin instalar nada, aunque necesita que WebView2 ya esté en el sistema.

Para desinstalarla: *Configuración → Aplicaciones → MusicRG → Desinstalar*.

## Uso

| Acción | Qué pasa |
| --- | --- |
| Abrir la app | Carga Apple Music. Inicias sesión con tu Apple ID como en la web, y la sesión se queda guardada. |
| Cerrar la ventana (✕) | La ventana se esconde en la bandeja del sistema y **la música sigue sonando**. |
| Clic izquierdo en el icono de la bandeja | Vuelve a mostrar la ventana y la pone delante. |
| Clic derecho en el icono de la bandeja | Abre un menú con **Play / Pause**, **Next**, **Previous**, **Pinned** (tus elementos de *Destacar*, para ponerlos a sonar), **Widget ›** (Taskbar widget, Show other apps, Flat audio bars y Caps Lock toast), **Screen & power ›** (Keep PC awake y los minutos de Screen care, u Off), **Always on top**, **Settings…** (donde también está Start with Windows), **Check for updates**, **Show MusicRG** y **Quit**. |
| Iniciar con Windows | Al arrancar el PC solo se encienden el widget, el aviso de Bloq Mayús y la bandeja (unos 5 MB). Apple Music se carga cuando abres la ventana o eliges algo de **Pinned**. |
| Clic en el widget de la barra de tareas | Muestra la ventana de MusicRG. |
| Chincheta de la barra de la ventana (o **Always on top** en la bandeja) | La ventana se queda por encima de las demás, para usarla de minirreproductor. Mientras está maximizada no se aplica. |
| Volver a abrir MusicRG con la app ya en marcha | No arranca otra copia: muestra la que ya estaba abierta. |
| Teclas multimedia / control de medios de Windows | Funcionan, porque la web de Apple Music usa la Media Session API. |
| Salir del todo | Clic derecho en la bandeja → **Quit**. |
| Actualizar | Cuando hay versión nueva, el menú de la bandeja muestra **Install update vX.Y.Z**. Ver [Actualizaciones](#actualizaciones). |

La ventana abre a 1180×760 y se puede encoger hasta 420×320, que sirve como minirreproductor.

## Widget de la barra de tareas y aviso de Bloq Mayús

### Widget "sonando ahora"

Mientras suena música aparece encima de la barra de tareas, a la izquierda:

```
┌──────┐  Título de la canción            ▎▍▌▋▊▋▌▍▎▍▌▋▊▋
│ arte │  Artista                         (barras en vivo)
└──────┘
```

- **Aparece** al darle a play y **desaparece** con un fundido cuando pausas o se acaba la música. Espera menos de un segundo antes de irse para no parpadear al cambiar de canción.
- **Las barras siguen el audio real** de MusicRG. Son 14 bandas de frecuencia, de graves a la izquierda a agudos a la derecha. Solo se oye a MusicRG: si suena otra cosa en el PC, las barras no se mueven por eso.
- **También muestra lo que suena en otras apps**: un vídeo de YouTube en el navegador, Spotify, VLC… cualquier app que aparezca en los controles multimedia de Windows (el panel del volumen). Si MusicRG está sonando tiene prioridad; si está parada, el widget enseña la otra app con su título, artista (o canal) y carátula. En ese caso las barras siguen todo el audio del PC menos MusicRG. Al hacer clic no se abre MusicRG, porque el widget no es suyo en ese momento. Se puede desactivar con **Show other apps in widget**.
- Usa los colores de Windows: el color de acento para las barras y el texto claro u oscuro según el tema de la barra de tareas.
- Se adapta al escalado de pantalla (100 %, 125 %, 150 %…).
- **Dónde se coloca:** con los iconos centrados va en el hueco libre de la izquierda (si el botón de Widgets/el tiempo está activado, deja su sitio). Con los iconos a la izquierda va junto al área de notificación.
- **Se esconde solo** con juegos, vídeos o presentaciones a pantalla completa, con la barra en vertical o con la barra en auto-ocultar mientras está escondida.
- Al pasar el ratón se ilumina un poco, y con un clic se abre MusicRG.

Si choca con algo de tu barra, puedes moverlo a mano en `%APPDATA%\MusicRG\settings.json` con `"widget_offset"`: es la distancia en píxeles (a escala 100 %) desde el borde izquierdo de la barra. Después reinicia MusicRG.

```json
{
  "widget": true,
  "caps_lock_toast": true,
  "other_apps": true,
  "widget_offset": 200
}
```

### Aviso de Bloq Mayús

Al pulsar Bloq Mayús aparece, encima de la barra de tareas, un aviso con un candado animado durante unos dos segundos:

- La tarjeta entra creciendo con un pequeño rebote y sale encogiéndose y desvaneciéndose.
- **"Caps Lock on"**: el grillete abierto gira, baja y encaja con un "clic". El candado y la pastilla de abajo pasan al color de acento.
- **"Caps Lock off"**: el grillete sube, gira hacia fuera y el candado se balancea un poco. La pastilla se encoge y se vuelve gris.
- El candado se dibuja con Direct2D, no es un glifo de fuente, y por eso puede animarse. Solo se redibuja mientras algo se mueve.

No roba el foco y los clics lo atraviesan.

Funciona aunque MusicRG esté en la bandeja, mientras la app esté abierta.

### Activar y desactivar

Clic derecho en el icono de la bandeja → submenú **Widget** (**Taskbar widget**, **Show other apps in widget**, **Flat audio bars**) y, debajo, **Caps Lock toast**. Se guarda en `settings.json` y se recuerda al volver a abrir.

### Menú de la bandeja

Arriba muestra lo que suena (lo mismo que el widget, recortado para que el menú no se ensanche). Luego vienen los controles, **Pinned**, el submenú **Widget**, las opciones con casilla y, al final, actualizar, mostrar y salir. **Keep PC awake** impide que el PC se suspenda y que se apague la pantalla mientras esté marcada.

**Always on top** deja la ventana por encima de las demás; es lo mismo que la chincheta junto a minimizar. Si la maximizas se suspende, y vuelve al restaurarla. Ojo: con la opción activa también tapa juegos en «ventana sin bordes».

**Screen care** cuida pantallas OLED/LED: si pasas 10 minutos sin tocar ratón ni teclado, apaga la pantalla (como el botón del monitor). La música sigue sonando y se enciende otra vez al mover el ratón o pulsar una tecla. No la apaga si hay algo a pantalla completa (vídeo, juego, presentación). Funciona también con **Keep PC awake** marcado: así el PC no se suspende pero la pantalla descansa. Se activa y se eligen los minutos (5, 10, 15, 30 o 60) en **Settings → Advanced settings → System → Screen care**; se aplica al momento. También se puede poner otro valor a mano en `%APPDATA%\MusicRG\settings.json` con `"screen_off_minutes"` (de 1 a 240).

### Ventana de ajustes

Clic derecho en la bandeja → **Settings…**. Arriba está lo principal, **Performance**. El resto (servicio de música, widget y sistema) va plegado en **Advanced settings**, y la ruta del archivo de ajustes en **Where settings are saved**: un clic los despliega y la ventana crece. Lo que cambies ahí se refleja en el menú de la bandeja, y al revés. Es una ventana nativa (Win32 + Direct2D por software, sin otro WebView): no existe hasta que la abres y, al cerrarla, se destruye con su hilo y la memoria vuelve a la de antes.

En **Performance** hay un dial con la resolución del temporizador de Windows que hay ahora mismo (escala en octavas, de 16 ms a 0,5 ms), con la mínima y la máxima que admite el equipo:

- **Timer resolution 0.5 ms** pide a Windows el intervalo más fino (lo mismo que el botón *Maximum* de herramientas como *Set Timer Resolution*). Tiene tres modos: **Off** (lo que decida Windows), **Auto** (solo mientras hay un juego o algo a pantalla completa; lo mira cada 2 s con la misma comprobación que esconde el widget) y **Always**. MusicRG lo pide aunque esté solo en la bandeja, y Windows lo suelta al cerrar la app. Gasta algo más de energía.
- **Free memory when idle** (activado por defecto): con la ventana del reproductor cerrada y nada sonando durante 30 minutos, MusicRG destruye el WebView2 entero, que son unos 100 MB y varios procesos. Al abrirlo desde la bandeja se vuelve a cargar en unos segundos. Si algo suena (en MusicRG o en otra app), no se descarga.
- **Apply to games** escribe `GlobalTimerResolutionRequests = 1` en `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\kernel` (pide permiso de administrador). Desde Windows 10 2004 la resolución es por proceso: sin este valor, los 0,5 ms solo valen para MusicRG y no para los juegos. Con él, la petición vale para todo el sistema. Necesita reiniciar Windows, y la ventana avisa mientras tanto. Desactivarlo deja el valor a 0.

En **Advanced settings → Player** se elige qué abre el reproductor: **Apple** (Apple Music, por defecto), **Spotify**, **YT Music**, **Custom** (cualquier dirección `https://`, que se escribe en el campo que aparece y se aplica con Enter) u **Off**. Con Off no hay reproductor ni ningún proceso de navegador; quedan el widget, el aviso de Bloq Mayús y la bandeja, y **Show MusicRG** abre los ajustes. Al cambiar de servicio, el reproductor se recarga.

El tema, `shell.js` (barra propia, Pinned, controles de la bandeja) y el aviso de lo que suena desde la página son solo para Apple Music. Las demás webs se abren tal cual, con la barra de título de Windows y sin ningún script de MusicRG. Su canción se lee de los controles multimedia de Windows, así que el widget y la cabecera del menú la siguen mostrando.

Los iconos son SVG propios de 16 px en `src-tauri/icons/menu/`. Al compilar, `build.rs` los pasa a imagen en dos tintas (menú claro y oscuro), así que la app no decodifica nada al ejecutarse.

### Privacidad

- Mientras el widget muestra MusicRG, la captura de audio solo escucha a sus procesos de WebView2 (en Windows 11). Mientras muestra otra app, escucha la salida del PC menos MusicRG. Nunca el micrófono. Se hace en memoria para calcular las barras y no se guarda nada.
- Lo que suena en otras apps (título, artista, carátula) lo lee de los controles multimedia de Windows, lo mismo que ve el panel del volumen. No sale de tu PC. Con **Show other apps in widget** desactivado no se muestra.
- Para el aviso de Bloq Mayús, Windows le pasa a MusicRG las pulsaciones de teclado (Raw Input). El código solo mira si la tecla es Bloq Mayús y descarta el resto sin guardarlas.
- La carátula se descarga solo de servidores de Apple (`*.mzstatic.com`, `*.apple.com`); cualquier otra URL se ignora.

## Actualizaciones

MusicRG se actualiza desde las GitHub Releases del repositorio:

- 20 segundos después de arrancar, y luego cada 6 horas, consulta `latest.json` del último release. No manda ningún dato tuyo: es una descarga normal de un archivo público.
- Si hay una versión más nueva, la entrada del menú de la bandeja pasa a **Install update vX.Y.Z** y el tooltip del icono lo avisa.
- **No instala nada sin que hagas clic.** Al instalar se cierra la app (se corta la música), se ejecuta el instalador sin preguntas y se vuelve a abrir.
- El instalador descargado tiene que estar firmado con la clave del proyecto. La clave pública está en `tauri.conf.json`. Si la firma no coincide, no se instala.
- Con **Check for updates** puedes comprobarlo a mano en cualquier momento.

La primera versión con actualizador hay que instalarla a mano. A partir de ahí, las siguientes llegan solas.

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

El widget y el aviso de Bloq Mayús están hechos para no notarse:

- **Ventanas nativas, no otro WebView.** Direct2D por software sobre ventanas *layered*: unos pocos MB y nada de GPU.
- **Cero trabajo en pausa.** Con la música parada no hay temporizadores ni captura de audio: el hilo del widget duerme hasta que llega un mensaje.
- **Solo repinta si algo cambia.** Mientras suena se anima a ~30 fps, pero si las barras no se mueven no se toca un píxel.
- **Audio barato.** Una FFT de 1024 puntos unas 30 veces por segundo, sin librerías externas. Medido: **0,06 % de CPU** con música sonando y el widget animado.
- **Bloq Mayús por eventos.** Usa Raw Input en vez de un hook global de teclado, así que no añade latencia al teclado de nadie y no sondea el estado de la tecla.
- **Play/pausa por eventos** de la propia página (`play`/`pause` del elemento de audio y cambios de la Media Session), sin sondeo.

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
│   ├─ Plugin single-instance → segunda ejecución = mostrar la primera         │
│   └─ comando now_playing  ◄──invoke──  shell.js (play/pausa, título, arte)  │
│                                                                              │
│  Hilo "rg-overlay" (overlay.rs) — bucle de mensajes Win32 propio            │
│   ├─ Widget: ventana layered propiedad de la barra de tareas + Direct2D     │
│   ├─ Aviso Bloq Mayús: ventana layered + Raw Input (sin hook global)        │
│   ├─ Hilo "rg-audio" (audio.rs): process loopback de WebView2 → FFT → barras │
│   └─ Hilo "rg-art" (art.rs): descarga y decodifica la carátula (URLMon+WIC) │
│                                                                              │
│  Dentro de la página (shell.js)                                              │
│   ├─ Inyecta el CSS como hoja construida (adoptedStyleSheets)                │
│   ├─ blur/focus/visibilitychange → clase .rg-idle + pausa vídeos mudos       │
│   ├─ window.__rg.cmd() → MusicKit.getInstance().play/pause/skip…            │
│   └─ eventos play/pause + Media Session → invoke('now_playing')             │
└──────────────────────────────────────────────────────────────────────────────┘
```

Paso a paso:

1. **Arranque.** `main.rs` lee el tema incluido en el binario (`include_str!`) y, si existe, `%APPDATA%\MusicRG\theme.css`. Junta los dos y mete el resultado dentro de `shell.js`, en el lugar de `__RG_CSS__`.
2. **Ventana.** Crea una ventana WebView2 que apunta a `https://music.apple.com/`, con tema oscuro, fondo `BG` (para que no haya destello blanco al cargar), los flags de Chromium y `shell.js` como *initialization script*, que se ejecuta antes que la página en cada navegación.
3. **Barra de título.** En Windows 11 llama a `DwmSetWindowAttribute(DWMWA_CAPTION_COLOR)` para pintar la barra nativa del mismo color que el fondo. En Windows 10 la llamada falla sin hacer ruido y la barra se queda oscura.
4. **Tema.** `shell.js` aplica el CSS con `new CSSStyleSheet()` + `document.adoptedStyleSheets`. Así no lo bloquea la Content Security Policy de Apple y no añade nodos al DOM.
5. **Bandeja.** Las opciones de reproducción del menú ejecutan `window.__rg.cmd(...)` en la página, que llama a la API oficial de **MusicKit JS** que ya usa la web (`play`, `pause`, `skipToNextItem`, `skipToPreviousItem`).
6. **Cerrar / mostrar.** Cerrar se intercepta (`prevent_close`), la ventana se oculta y WebView2 pasa a memoria baja. Al mostrarla vuelve a memoria normal y recupera el foco.
7. **Sonando ahora.** `shell.js` escucha los eventos `playing`/`pause`/`ended` de los elementos de audio de la página (Apple Music usa más de uno, así que suena si *cualquiera* suena) y los cambios de `navigator.mediaSession.metadata`. Cuando algo cambia, llama al comando `now_playing` por la IPC de Tauri. Una *capability* remota ([`capabilities/apple-music.json`](src-tauri/capabilities/apple-music.json)) deja a `https://music.apple.com` llamar **solo** a ese comando.
8. **Widget.** El hilo `rg-overlay` lo dibuja con Direct2D sobre un mapa de bits en memoria y lo pone en pantalla con `UpdateLayeredWindow` (transparencia por píxel). La ventana es "propiedad" de la barra de tareas (`Shell_TrayWnd`), así que siempre queda encima de ella. Cada segundo, mientras está visible, comprueba la posición de la barra y si hay algo a pantalla completa.
9. **Barras.** El hilo `rg-audio` captura con *process loopback* (WASAPI) el árbol de procesos del navegador de WebView2, cuyo PID se pide con `ICoreWebView2.BrowserProcessId` porque Windows no lo cuenta dentro del árbol de `musicrg.exe`. En Windows anterior a la build 20348 cae al loopback de la salida por defecto. Cada ~33 ms aplica una ventana Hann y una FFT de 1024 puntos, agrupa en 14 bandas logarítmicas (50 Hz – 14 kHz) y deja los niveles en atómicos que el widget lee al pintar.
10. **Bloq Mayús.** El widget se registra para Raw Input de teclado en modo *sink*. Al soltar Bloq Mayús espera 30 ms a que Windows actualice el conmutador, lee `GetKeyState(VK_CAPITAL)` y enseña el aviso.

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
   ├─ build.rs               # tauri-build + iconos del menú a 16 px
   ├─ tauri.conf.json        # Nombre, versión, identificador, bundle NSIS, WebView2, updater
   ├─ tauri.updater.json     # Lo añade el CI para generar el .sig del actualizador
   ├─ icons/
   │  ├─ icon.ico            # Icono de la app y de la bandeja
   │  ├─ icon.svg            # Fuente del icono
   │  └─ menu/*.svg          # Iconos del menú de la bandeja (16 px)
   ├─ inject/
   │  ├─ theme.css           # Tema que se inyecta en music.apple.com
   │  └─ shell.js            # Script de página: CSS, modo reposo, controles
   ├─ capabilities/
   │  └─ apple-music.json    # music.apple.com solo puede llamar a now_playing
   └─ src/
      ├─ main.rs             # Ventana del reproductor y comandos de la página
      ├─ tray.rs             # Menú de la bandeja y su cabecera
      ├─ menu_icons.rs       # Iconos del menú (rasterizados por build.rs)
      ├─ awake.rs            # Keep PC awake (petición de energía de Windows)
      ├─ on_top.rs           # Always on top (chincheta y casilla de la bandeja)
      ├─ screen.rs           # Screen care (apaga la pantalla tras N min sin actividad)
      ├─ timer.rs            # Timer resolution 0,5 ms y modo global (registro)
      ├─ panel.rs            # Ventana de ajustes nativa (Direct2D)
      ├─ player.rs           # Qué web abre el reproductor y descargarlo si no se usa
      ├─ lock.rs             # Mutex sin unwrap()
      ├─ overlay.rs          # Widget de la barra de tareas y aviso de Bloq Mayús
      ├─ audio.rs            # Captura de audio (MusicRG u otras apps) + FFT → niveles
      ├─ media.rs            # Lo que suena en otras apps (controles multimedia de Windows)
      ├─ art.rs              # Descarga y decodificación de la carátula
      ├─ settings.rs         # %APPDATA%\MusicRG\settings.json
      └─ updater.rs          # Comprobar e instalar actualizaciones
```

No hay frontend propio. La interfaz es la web de Apple Music y el proyecto solo añade la parte nativa y lo que se inyecta en la página.

### Dependencias

| Crate | Uso |
| --- | --- |
| `tauri` 2 (feature `tray-icon`) | Ventana, WebView2, bandeja, menús. |
| `tauri-plugin-single-instance` | Que solo haya una instancia. |
| `tauri-plugin-updater` | Actualizaciones firmadas desde GitHub Releases. |
| `serde_json` | Escapar el CSS para meterlo en el JS y leer/guardar `settings.json`. |
| `webview2-com` | `ICoreWebView2_19` para el nivel de memoria y `BrowserProcessId` para la captura de audio. |
| `windows` / `windows-core` | Win32: ventanas del widget, Direct2D, DirectWrite, WIC, URLMon, WASAPI, Raw Input, registro. WinRT: controles multimedia de Windows (`Media.Control`). |
| `windows-numerics` | Tipos de vector/matriz que usa Direct2D. |
| `resvg` (solo al compilar) | Pasa los SVG del menú a imagen en `build.rs`; no entra en el ejecutable. |

Aparte del actualizador (que trae su propio cliente HTTP), no hay crates de HTTP, imagen, audio ni FFT: todo eso lo hace Windows o unas pocas líneas propias.

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
| `src-tauri/target/release/musicrg.exe` | Ejecutable suelto (~4 MB). |
| `src-tauri/target/release/bundle/nsis/MusicRG_<versión>_x64-setup.exe` | Instalador (~1,4 MB). |

La primera compilación tarda unos minutos porque compila todas las dependencias. Las siguientes son mucho más rápidas.

### Probar el widget sin reproducir música

En la versión de desarrollo, la variable `MUSICRG_DEMO=1` finge una canción sonando (y `MUSICRG_DEMO_ART` le pone una carátula). La consola muestra también cada aviso `now_playing` que llega de la página.

```powershell
$env:MUSICRG_DEMO = "1"; .\src-tauri\target\debug\musicrg.exe
```

Para el aviso de Bloq Mayús, `MUSICRG_OSD_DEMO=1` lo alterna solo cada pocos segundos sin tocar la tecla, y en este modo puede convivir con la MusicRG instalada. `MUSICRG_OSD_SLOW=6` reproduce la animación 6 veces más lenta para verla con calma. Las dos variables solo existen en la versión de desarrollo.

```powershell
$env:MUSICRG_OSD_DEMO = "1"; $env:MUSICRG_OSD_SLOW = "6"; .\src-tauri\target\debug\musicrg.exe
```

Las pruebas unitarias se lanzan con `cargo test` en `src-tauri`. Hay otras dos marcadas `#[ignore]` (captura de audio real y descarga de una carátula) que se ejecutan con `cargo test -- --ignored`.

> El repositorio trae `package-lock.json` (npm) y la CI usa `npm install && npm run build`. Esos comandos también funcionan en local.

### Cambiar la versión

La versión está en dos sitios y conviene que coincidan:

- `src-tauri/tauri.conf.json` → `"version"` (es la que usa el instalador)
- `src-tauri/Cargo.toml` → `version`

## Integración continua y releases

[`.github/workflows/build.yml`](.github/workflows/build.yml) se ejecuta en **cada push** y también a mano (*workflow_dispatch*), en `windows-latest`:

1. Instala Node 22 y Rust estable, y cachea `src-tauri/target`.
2. Ejecuta `npm install` y `npm run build`. Si existe el secret `TAURI_SIGNING_PRIVATE_KEY`, firma también el instalador (`.sig`).
3. Sube `musicrg.exe` y el instalador como artefacto **`MusicRG-windows`**.
4. Si el push es una **etiqueta `v*`** (por ejemplo `v0.2.0`), genera `latest.json` y crea una **GitHub Release** con el instalador, su `.sig` y `latest.json`. Falla si la etiqueta no coincide con la versión de `tauri.conf.json` o si falta la clave de firma.

### Clave de firma del actualizador

El par de claves se crea una sola vez con `tauri signer generate`. La privada vive **fuera del repo** (`%USERPROFILE%\.tauri\musicrg.key`) y en GitHub como secret:

- *Settings → Secrets and variables → Actions → New repository secret*
- `TAURI_SIGNING_PRIVATE_KEY` = el contenido de `musicrg.key`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = no hace falta crearlo (la clave no tiene contraseña)

Si pierdes la clave privada, las versiones ya instaladas no podrán actualizarse a builds firmados con una clave nueva: habría que reinstalar a mano.

Para publicar una versión, sube antes la versión en `tauri.conf.json` y `Cargo.toml` (ver [Cambiar la versión](#cambiar-la-versión)), haz commit y luego:

```powershell
git tag v0.2.0
```

```powershell
git push origin v0.2.0
```

### Instalador firmado en local

Para tener el instalador sin pasar por GitHub, [`scripts/publish.ps1`](scripts/publish.ps1) hace lo mismo que el workflow con una etiqueta, pero en tu PC y sin publicar nada:

1. Comprueba que la versión de `tauri.conf.json` y `Cargo.toml` coincide y que `%USERPROFILE%\.tauri\musicrg.key` es la clave de `tauri.conf.json`.
2. Compila con la firma del actualizador.
3. Deja en `publish\v<versión>\` (ignorada por git) el instalador, su `.sig` y `latest.json`, listos para subir a un release `v<versión>`.

Necesita la CLI de Tauri desde crates.io (una sola vez):

```powershell
cargo install tauri-cli --version "^2" --locked
```

```powershell
powershell -File scripts/publish.ps1
```

## Datos que guarda en tu PC

| Ruta | Contenido |
| --- | --- |
| `%LOCALAPPDATA%\com.xraul.musicrg\` | Perfil de WebView2: cookies, sesión de Apple ID, caché. |
| `%APPDATA%\MusicRG\theme.css` | Tu tema personalizado (solo si lo creas). |
| `%APPDATA%\MusicRG\settings.json` | Opciones de la bandeja (widget, aviso de Bloq Mayús, Keep PC awake, Screen care, Always on top...), la última lista de Pinned, y `widget_offset` y `screen_off_minutes` si los pones. |

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

**El widget no aparece.**
Comprueba que **Widget en la barra de tareas** está marcado en el menú de la bandeja y que la música suena de verdad (no solo cargada). Tampoco sale con la barra vertical, con la barra auto-ocultada mientras está escondida o con algo a pantalla completa.

**El widget tapa el botón del tiempo u otra cosa de la barra.**
Ajusta `"widget_offset"` en `%APPDATA%\MusicRG\settings.json` (ver [Widget](#widget-de-la-barra-de-tareas-y-aviso-de-bloq-mayús)) y reinicia MusicRG.

**Las barras no se mueven.**
En Windows 11 escuchan solo a MusicRG: si la canción está en silencio o el volumen de MusicRG en el mezclador de Windows está a cero, se quedan quietas.

**Al abrir MusicRG otra vez no aparece una segunda ventana.**
Es a propósito: solo hay una instancia y la que estaba abierta pasa al frente.

## Limitaciones

- Depende de la web de Apple Music. Si Apple cambia sus clases o variables CSS, parte del tema puede dejar de aplicarse. La reproducción no se rompe porque usa MusicKit.
- Solo Windows. El código compila en otros sistemas, pero el color de la barra de título y el ahorro de memoria solo están hechos para Windows.
- No hay descargas sin conexión ni letras sincronizadas fuera de lo que ya ofrece la web.
- El widget sale solo en la barra de tareas principal (no en la de otros monitores).
- En Windows 10 (antes de la build 20348) las barras siguen todo el audio del PC, no solo a MusicRG, porque no existe la captura por proceso.
- El control multimedia de Windows (el panel de volumen) sigue mostrando la app como `msedgewebview2.exe`: el nombre lo decide WebView2.
