# Spec · Primeras mejoras del fork de Kooha (Task 1088)

**Fuente de verdad de las decisiones:** `docs/specs/grill-respuestas.md` (P1 a P22).
**Glosario:** `CONTEXT.md`. **Decisiones de arquitectura:** `docs/adr/0001` a `0003`.

## Objetivo

Que Sebastián se entere **antes** de terminar una grabación de lo que no se está grabando,
pueda pausarla, controlarla desde la bandeja, sumar su cara con un PiP de cámara, ver el
borde del área que graba, y diagnosticar por qué Kooha no anda.

## Contexto verificado en el código (commit `d5e68e3`)

- Kooha 2.3.2, Rust 2024, GTK 4 + libadwaita, GStreamer 0.25. Graba por el portal
  `org.freedesktop.portal.ScreenCast` con `pipewiresrc`.
- La pausa existe (`Recording::pause`, acción `win.toggle-pause`) pero el botón
  `pause_record_button` está oculto en `data/resources/ui/window.ui` y el atajo comentado en
  `src/application.rs`.
- El modo del puntero (`CursorMode`) se fija al abrir la sesión del portal.
- El área se graba capturando el monitor entero y recortando con `videocrop`
  (`src/pipeline.rs`, `make_videocrop`).
- No hay ícono de bandeja.
- Sesión de Sebastián: KDE Plasma sobre Wayland.

## Alcance

Seis mejoras, en este orden, un commit por mejora encadenado sobre el anterior (P21, P22):

1. Avisos de captura.
2. Ícono de bandeja con menú y toggles.
3. Pausa.
4. PiP de cámara.
5. Borde del área.
6. Reporte de requisitos.

Textos nuevos en inglés en el código, con su traducción en `po/es.po` (P20). Estilo de
Kooha, sin romper GNOME ni X11 (P21).

## Fuera de alcance

- Cambiar el audio o el puntero en medio de una grabación (P5: valen para la próxima).
- Atajo global y barra flotante de controles (P7).
- Componer la cámara dentro del pipeline (ADR 0002).
- PiP y borde fuera de un compositor con *wlr-layer-shell* (ADR 0001).

## 1 · Avisos de captura

**Qué falta** se calcula al arrancar la grabación con los valores que va a usar:

| Condición | Aviso |
|---|---|
| Micrófono y audio del escritorio apagados, o el formato no admite audio | «Without audio» |
| Sólo el micrófono apagado | «Without microphone» |
| Sólo el audio del escritorio apagado | «Without desktop audio» |
| Puntero apagado | «Without mouse pointer» |

Con el formato sin audio (GIF), el aviso es «Without audio» aunque los interruptores estén
prendidos (P2: un aviso por fuente; con las dos apagadas, «Without audio»).

**Dónde se ve** (P1):

- **Notificación del escritorio** cuando la grabación pasa a `Recording` por primera vez, con
  todos los avisos. Sin avisos, no hay notificación.
- **Cartel fijo** en la página de grabación de la ventana, en `Recording` y `Paused`.
- **Cartel en la cuenta regresiva** (página `delay_page`), si hay demora.

Criterios de aceptación:

- A1. Con micrófono apagado, audio del escritorio prendido y puntero prendido, la
  notificación y el cartel dicen sólo «Without microphone».
- A2. Con todo prendido no hay notificación ni cartel.
- A3. Con el formato GIF, el aviso incluye «Without audio».

## 2 · Ícono de bandeja

Ícono por StatusNotifierItem (crate `ksni`), presente **siempre que Kooha esté abierto** (P3).

**Menú** (P4), de arriba abajo:

1. Estado, no clicable: «Ready», «Recording — 00:12» o «Paused — 00:12».
2. Los avisos de captura, no clicables, mientras haya grabación.
3. «Start Recording» / «Stop Recording».
4. «Pause» / «Resume» (agregado en el commit de la mejora 3).
5. Casillas: «Record Desktop Audio», «Record Microphone», «Show Pointer». Reflejan y cambian
   las mismas claves de GSettings que la ventana. **Mientras hay una grabación**
   (`Delayed`, `Recording`, `Paused`, `Flushing`) se ven deshabilitadas y no se tocan (P5).
6. Casilla «Camera» (agregada en el commit de la mejora 4).
7. «Show Window» y «Quit».

Click izquierdo sobre el ícono: muestra la ventana. El ícono cambia según el estado
(inactivo, grabando, en pausa) y su tooltip lleva el estado y los avisos.

«Quit» con una grabación en curso presenta la ventana y su diálogo de confirmación
existente.

Criterios:

- B1. Cambiar «Record Microphone» en la bandeja cambia el interruptor de la ventana, y al
  revés.
- B2. Con una grabación en curso las tres casillas están deshabilitadas.
- B3. Si no hay quien muestre íconos de bandeja, Kooha arranca igual.

## 3 · Pausa

- **Botón de pausa** visible en la página de grabación y **atajo Ctrl+K** con la ventana
  enfocada (P7), más el ítem de la bandeja.
- **Archivo** (P6): el tramo pausado no está. Video y audio saltan juntos (ADR 0003).
- **Destello de reanudación** (P6, nuevo): interruptor «Flash on Resume» en Preferencias,
  apagado por defecto. Prendido, el video muestra blanco durante 150 ms al reanudar.
- El cronómetro de la ventana no avanza durante la pausa.
- Terminar la grabación estando en pausa escribe el archivo bien.

Criterios:

- C1. Grabar 3 s, pausar 2 s, reanudar, grabar 3 s: el archivo dura 6 s ± 0,3 s.
- C2. Las marcas de tiempo de video y audio no tienen hueco mayor a dos cuadros.
- C3. Con el destello prendido, los cuadros de los primeros 150 ms tras reanudar son blancos.

## 4 · PiP de cámara

- **Prender y apagar**: botón en la ventana principal junto a los interruptores, casilla en
  la bandeja, interruptor en Preferencias.
- **Elegir la fuente** (P11): en Preferencias y en la ventana principal (un menú del mismo
  botón). Fuentes: los dispositivos `Video/Source` del `gst::DeviceMonitor`.
- **Cuándo se ve** (P12): desde que se prende, también antes de grabar.
- **Mover**: arrastrando con el botón izquierdo. La posición se guarda en GSettings y se
  limita al monitor.
- **Tamaño** (P10): la rueda del mouse sobre el PiP lo agranda y achica, entre 96 y 640 px
  lógicos de alto, en pasos del 10 %. Se guarda.
- **Forma** (P9): círculo o rectángulo con esquinas redondeadas; el **botón derecho** las
  alterna. Se guarda.
- **Espejado** (P13): espejado en `Init` y `Finished`; sin espejar desde que arranca la
  cuenta regresiva o, sin demora, antes de poner el pipeline en `PLAYING`, y hasta que la
  grabación termina.
- **Monitor**: el PiP aparece en el monitor donde estaba la última vez; la primera vez, en el
  monitor primario, abajo a la derecha.
- Sin capas disponibles, los controles del PiP se ven deshabilitados y Preferencias muestra el
  aviso (P18).
- Si la fuente elegida desaparece, el PiP se apaga y se notifica.

Criterios:

- D1. Con el PiP prendido y una grabación de monitor completo, el PiP sale en el video sin
  espejar.
- D2. Botón derecho alterna la forma; la rueda cambia el tamaño; los dos persisten al
  reiniciar Kooha.
- D3. El PiP no se puede arrastrar fuera del monitor.

## 5 · Borde del área

- Se ve en la cuenta regresiva, grabando y en pausa (P14), sólo con `CaptureMode::Selection`.
- Línea opaca de 1 px lógico: guiones blancos de 4 px sobre gris claro `#c8c8c8`, sin
  transparencia (P15, y «más sutil que en el mock», `docs/specs/mock-feedback.md`).
- El borde interior queda a 2 px lógicos por fuera del rectángulo que recorta `videocrop`,
  calculado con el mismo redondeo que `make_videocrop`. Si el área toca un borde del monitor,
  ese lado no se dibuja.
- La capa es transparente al mouse.
- Sin capas disponibles, no hay borde (P18).

Criterios:

- E1. Ningún píxel del borde aparece en el video.
- E2. Con el área pegada al borde izquierdo del monitor, se dibujan sólo tres lados.

## 6 · Reporte de requisitos

**Chequeos** (P16), cada uno `Ok`, `Warning` o `Failed`:

| Chequeo | Falla si |
|---|---|
| Elementos de GStreamer | Falta `pipewiresrc`, `pulsesrc`, `videoconvert`, `videoscale`, `videocrop`, `videobalance`, o un codificador o muxer de los perfiles |
| Portal de captura | `org.freedesktop.portal.ScreenCast` no responde. Informa versión y modos de puntero |
| PipeWire | No hay socket de PipeWire. Informa la versión |
| Audio | No hay salida por defecto (para el audio del escritorio) o micrófono por defecto |
| Cámaras | Ninguna fuente `Video/Source` (es `Warning`, porque sólo afecta al PiP) |
| Capas | No hay capas disponibles (ADR 0001) |
| Bandeja | Nadie tiene el nombre D-Bus `org.kde.StatusNotifierWatcher` |
| Carpeta de grabaciones | No existe o no se puede escribir |

**Presentación** (P17):

- Diálogo «Requirements Report» desde el menú principal: un checklist con tilde verde o
  cruz roja (y signo de advertencia amarillo para `Warning`). **Sólo debajo de cada falla**:
  la descripción, qué deja de funcionar, y el comando para arreglarla con botón de copiar.
- Botón «Copy Report» que copia todo en texto.
- `kooha --check` imprime el mismo reporte en la terminal y sale con 0 si no hay fallas, 1 si
  hay alguna.
- El diálogo de error de una grabación fallida ofrece «Open Requirements Report».
- Los comandos de arreglo son para Ubuntu/Debian (`apt install …`).

Criterios:

- F1. Sin `gtk4-layer-shell` compilado, el chequeo «Capas» falla y su comando es
  `sudo apt install libgtk4-layer-shell-dev` más recompilar.
- F2. `kooha --check` corre sin abrir ventanas.

## Preferencias

Se agregan dos grupos a `preferences_dialog.ui`:

- **Recording**: «Flash on Resume».
- **Camera & Overlays**: «Show Camera», «Camera Source», «Camera Shape». Si no hay capas
  disponibles, el grupo muestra arriba el aviso «Overlays unavailable» con el motivo y un botón
  que abre el reporte de requisitos, y sus filas quedan deshabilitadas.

## Claves nuevas de GSettings

| Clave | Tipo | Default |
|---|---|---|
| `pause-flash` | `b` | `false` |
| `camera-enabled` | `b` | `false` |
| `camera-device` | `s` | `""` (la primera disponible) |
| `camera-shape` | `s` (`circle`, `rounded-rectangle`) | `"circle"` |
| `camera-size` | `u` | `200` |
| `camera-position` | `(ii)` | `(-1, -1)` (sin posición guardada) |
| `camera-monitor` | `s` | `""` (el conector del monitor, por ejemplo `DP-1`) |
