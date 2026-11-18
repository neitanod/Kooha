# Spec · Rectángulos guardados y rendimiento de la captura (Task 1092)

**Fuente de verdad de las decisiones:** `docs/specs/2026-11-17-saved-areas-grill-respuestas.md`
(P1 a P16; el Cuestionario completo, con el porqué de cada pregunta, en
`docs/specs/2026-11-17-saved-areas-grill.json`). **Glosario:** `CONTEXT.md`, sección «Términos de
la Task 1092». **Decisiones de arquitectura:** `docs/adr/0004` a `0006`.

## Objetivo

Que Sebastián marque el área a grabar una sola vez, sobre la pantalla real; que la pueda guardar
con nombre y recuperarla meses después para rehacer una demo con el mismo tamaño en el mismo
monitor; y que la captura gaste menos CPU y llegue a 60 FPS sin perder cuadros.

## Contexto verificado (commit `2e4dac6`, 17/11/2026)

- Modo Selección hoy: KDE comparte una fuente, Kooha la muestra en `src/area_selector/` y deja
  recortarla. El pipeline escala el cuadro entero a medidas pares y después recorta:
  `pipewiresrc → videoflip → videorate → videoscale → videocrop → compuerta de pausa → queue`
  (`src/pipeline.rs`).
- El permiso del portal se guarda en la clave `screencast-restore-token` (`src/recording.rs`).
- El selector de upstream corre dentro de `Recording::start_inner`, después del portal, y usa el
  fd de PipeWire para su vista previa (`src/recording.rs:301-306`, `src/area_selector/mod.rs:189`).
  El borde elige su monitor por tamaño con `area_border::stream_monitor` (`src/area_border/mod.rs:99`).
- El menú de la bandeja (`src/tray/model.rs`) es una lista plana de comandos sin datos
  (`TrayCommand`); `ksni` 0.3.7 trae `SubMenu` y `RadioGroup`, sin usar todavía.
- Perfil de Sebastián: `mp4` (`x264enc speed-preset=ultrafast`), 30 FPS, modo `selection`.
- Monitores (`kscreen-doctor -o`): eDP-1 en 457,1080, 1463×915 lógicos, escala 1,75 (2560×1600
  físicos); HDMI-A-5 en 0,0, 1920×1080, escala 1; DP-3 en 1920,454, 1600×1000 lógicos, escala
  1,05 (1680×1050 físicos).
- Codificadores instalados: `vah264lpenc` + `vapostproc` (Intel), `nvh264enc` (NVIDIA),
  `x264enc`. GStreamer 1.28.2. `vah264lpenc` acepta hasta 4096×4096 y entrada en memoria VA,
  DMA-BUF o memoria común.
- Cámara integrada `/dev/video0`: tiene `exposure_dynamic_framerate` (bool, default 0, valor
  actual 1). Da 30 FPS sólo en MJPG.

## Alcance

Cuatro partes, en este orden de entrega (Mensaje 15443: la primera instalable cuanto antes):

1. **Marcar el área sobre la pantalla** (P1 a P4).
2. **Rectángulos guardados** y su lista (P5 a P7), con el borde desde que se elige (P8) y la
   grabación con el área elegida (P9, P10).
3. **Rendimiento de la captura**: medición, recorte antes de escalar, colas, codificador por
   hardware y DMA-BUF (P11 a P14).
4. **Mantener los cuadros de la cámara** (P15).

## Afuera

- Guardar con el rectángulo formato, FPS, nota o la posición del PiP (P6: sólo geometría).
- Áreas que cruzan dos monitores.
- Mostrar los FPS reales en la interfaz (P12 no lo eligió).
- Exportar o importar la lista desde la interfaz (el archivo JSON alcanza, P7).
- El perfil experimental «WebM VA H264» de upstream queda como está.

## 1 · Marcar el área sobre la pantalla

**Cuándo se abre.** En modo Selección, con capas disponibles:
- el botón «Marcar área» de la ventana principal (al lado del desplegable de la parte 2);
- «Marcar área nueva…» del menú de la bandeja;
- «Volver a marcar» de un rectángulo en la ventana Rectángulos;
- al apretar Grabar en modo Selección sin ningún área elegida.

Sin capas disponibles, el modo Selección sigue con el selector de upstream, sin cambios (P1,
ADR 0004).

**Cómo se ve.** Una capa de marcado por monitor conectado, de pantalla completa, con un velo
negro al 30 % de opacidad, a ojo. Mientras se arrastra, el rectángulo queda transparente (sin
velo) con un contorno blanco de 1 píxel lógico. Al lado del rectángulo, una etiqueta con el
ancho × alto en píxeles físicos (P2, P3). En la capa del monitor donde está el mouse, un texto
de ayuda arriba y al centro: «Arrastrá para marcar · Enter confirma · Esc cancela».

**Interacción.**
- Arrastrar sobre cualquier monitor marca un rectángulo en ese monitor. El arrastre queda
  limitado a ese monitor. Empezar un arrastre nuevo en otro monitor reemplaza el rectángulo.
- Después de soltar (P2):
  - ocho manijas (cuatro esquinas, cuatro bordes) redimensionan; arrastrar el interior lo mueve;
  - las flechas mueven el rectángulo 1 píxel físico; Shift + flechas, 10;
  - un campo «Tamaño» con el ancho × alto actual, editable: al escribir `1280x720` (o `1280×720`)
    y Enter, el rectángulo toma ese tamaño manteniendo la esquina de arriba a la izquierda, y se
    corre hacia adentro si no entra en el monitor; si no entra ni así, el campo se pone en rojo y
    no cambia nada.
- **Enter** confirma (desde el campo Tamaño, Enter aplica el tamaño; un segundo Enter
  confirma). **Escape** cancela y deja el área elegida como estaba. El botón derecho del mouse
  también cancela.
- Ancho y alto se redondean a números pares, porque el codificador H.264 los exige; la
  etiqueta muestra el valor redondeado. Mínimo 64×64 píxeles físicos.

**Al confirmar (P4).** Las capas de marcado se cierran y aparece un diálogo chico de Kooha:
«Área de 1280×720 en DP-3», con un campo «Guardar como…» optativo y el botón «Listo».
- Con nombre: se agrega a la lista y queda elegida.
- Sin nombre: queda como **último rectángulo**, elegido, fuera de la lista. Marcar otra vez sin
  nombre reemplaza al último rectángulo.
- Un nombre repetido pregunta si reemplaza al guardado con ese nombre.

**Conversión de coordenadas.** La capa trabaja en píxeles lógicos del monitor. El área se
guarda en píxeles físicos: `físico = round(lógico × escala)`, con la escala fraccionaria del
monitor (`gdk::Monitor::scale()`). El tamaño físico del monitor es `round(geometría × escala)`.

## 2 · Rectángulos guardados

**El archivo (P7).** `~/.config/kooha/rectangles.json` (respetando `XDG_CONFIG_HOME`):

```json
{
  "version": 1,
  "last": { "connector": "DP-3", "x": 100, "y": 80, "width": 1280, "height": 720,
            "monitor_width": 1680, "monitor_height": 1050, "scale": 1.05 },
  "areas": [
    { "name": "Demo Agencia", "connector": "DP-3", "x": 100, "y": 80, "width": 1280,
      "height": 720, "monitor_width": 1680, "monitor_height": 1050, "scale": 1.05 }
  ],
  "selected": "Demo Agencia"
}
```

- `selected` es el nombre del área elegida, `null` para el último rectángulo, o ausente si no hay
  ninguna elegida.
- Se escribe de forma atómica (archivo temporal + rename). Si el archivo no se puede leer o
  está roto, Kooha lo renombra a `rectangles.json.broken-<fecha>`, arranca con la lista vacía y
  lo avisa con un toast.

**Desactualizada (P10, ADR 0005).** Un área está desactualizada si su conector no está entre los
monitores conectados, o si el tamaño físico o la escala actuales del monitor difieren de los
guardados (la escala con tolerancia 0,01). Se recalcula cada vez que cambia la lista de monitores
de GDK.

**Dónde se elige (P5).**
- **Ventana principal:** en modo Selección, debajo del selector de modo, una fila con un
  desplegable de áreas (los nombres guardados, más «Último rectángulo · 1280×720 en DP-3» si
  existe), el botón «Marcar área» y el botón «Rectángulos…» que abre la ventana. Un área
  desactualizada aparece en el desplegable con un ícono de advertencia.
- **Ventana Rectángulos:** lista con una fila por área guardada: nombre, «1280×720 en DP-3»,
  y botones Elegir (marca la elegida), Renombrar (edición en línea), Volver a marcar y Borrar
  (con confirmación). Un área desactualizada dice por qué («DP-3 no está conectado» o «DP-3
  cambió de 1680×1050 a 1920×1200») y ofrece Volver a marcar. Volver a marcar abre las capas de
  marcado con el rectángulo actual precargado si el monitor sigue conectado; al confirmar,
  reemplaza la geometría y conserva el nombre (sin el diálogo de «Guardar como…»).
- **Bandeja:** submenú «Área» con un ítem de radio por área (más el último rectángulo), y
  «Marcar área nueva…». Las desactualizadas aparecen deshabilitadas. Elegir un área desde la
  bandeja también pone a Kooha en modo Selección.

## 3 · Borde del área elegida y grabación

**Borde (P8).** El borde punteado de la Task 1088 (`src/area_border/`) se muestra alrededor del
área elegida, en su monitor, desde que se elige, mientras Kooha está en modo Selección y la
opción de mostrar el borde está prendida; durante la grabación sigue como hoy. Se oculta y se
vuelve a mostrar con:
- el atajo `Ctrl+B` en la ventana principal;
- el ítem «Mostrar borde del área» (casilla) de la bandeja;
- un botón con un ícono de ojo en la fila del desplegable.
La elección de ocultarlo persiste (clave nueva `show-area-border-idle`, prendida por defecto)
y no afecta al borde durante la grabación, que sigue gobernado por la opción existente.
Con el área desactualizada, el borde no se muestra.

**Grabar con el área elegida (P9, ADR 0005).**
1. Con área desactualizada, Grabar no arranca: toast «El área “<nombre>” está desactualizada:
   <motivo>» con el botón «Volver a marcar».
2. Kooha abre la sesión del portal pidiendo **monitor** (`SourceType::MONITOR`) con el permiso
   guardado.
3. Si el stream recibido mide lo mismo que el monitor del área (en píxeles físicos, ±2 por
   redondeo), graba recortando el área directamente en píxeles del stream. **No se abre el
   selector de upstream.**
4. Si no coincide: cierra esa sesión sin grabar, borra el permiso guardado, muestra el toast
   «Elegí <conector> entero en el diálogo de KDE» y abre una sesión nueva. Si la segunda tampoco
   coincide: no graba, y el toast dice «Compartiste <tamaño>, pero el área está en <conector>
   (<tamaño>)».
5. La cuenta regresiva corre después de obtener el stream correcto, como hoy.

Los toasts de esta sección necesitan un `adw::ToastOverlay` en la ventana principal, que hoy no
tiene (los errores de grabación usan `adw::AlertDialog`, `src/window/mod.rs:285`). `Ctrl+B` está
libre entre los atajos de `src/application.rs:402-411`.

## 4 · Rendimiento de la captura

**Medición (P11).** Script de desarrollo `tools/bench-capture.py` (Python + GStreamer por `gi`):
- abre la sesión del portal ScreenCast con el `screencast-restore-token` de Kooha, sin diálogo
  si KDE lo acepta; al terminar escribe en la clave el token nuevo que entrega KDE;
- graba el mismo rectángulo, 20 segundos por variante, a 60 FPS y a 30 FPS, a un archivo en
  `/tmp`, con cada variante del pipeline (escala-después-recorte de hoy, recorte primero, cada
  codificador, con y sin DMA-BUF, con y sin colas ajustadas);
- anota por variante: CPU del proceso (de `/proc/self/stat`), FPS reales a la salida del
  codificador, cuadros perdidos (huecos de marca de tiempo mayores a 1,5 cuadros) y si arrancó;
- deja una tabla Markdown en `docs/perf/2026-11-<día>-capture-bench.md`.
- La comparación con la «región» de KDE (pedida en la consigna) necesita que Sebastián elija la
  región una vez en el diálogo: el script la corre al final, sólo con `--with-kde-region`.

Mientras corre la medición, la pantalla tiene que mostrar algo que se mueva en el rectángulo; el
script abre un video de prueba (`videotestsrc pattern=ball` en una ventana) dentro del
rectángulo.

**Recortar antes de escalar (P12).** El pipeline del modo Selección pasa a
`… videorate → videocrop → (videoscale sólo si hace falta) → …`. Con el área en píxeles físicos y
medidas pares, el recorte da medidas pares y el escalado se elimina. El escalado queda sólo para
el selector de upstream (sin capas), y también ahí va después del recorte.

**Colas (P12).** Una `queue` entre la fuente y el recorte, y otra antes del codificador, con
tamaños fijados por la medición; ningún pico del codificador debe frenar a `pipewiresrc`.

**Codificador (P12, P13, ADR 0006).** Preferencia «Codificador» en la página de video de
Preferencias, visible cuando el formato es MP4 o Matroska: Automático, Intel, NVIDIA, CPU. Cada
opción no disponible se muestra deshabilitada con el motivo («`vah264lpenc` no está instalado»).
- Intel: `vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! vah264lpenc`.
- NVIDIA: `cudaupload ! cudaconvert ! nvh264enc` (o la cadena que gane la medición).
- CPU: el `x264enc` de hoy.
- Automático: el orden lo fija la medición; Intel primero si su rendimiento a 60 FPS alcanza,
  porque gasta menos batería (decisión a confirmar con los números).
- Si el codificador por hardware falla al arrancar (error del elemento en el paso a PLAYING),
  Automático reintenta con el siguiente sin que el usuario vea nada salvo el log.

**DMA-BUF (P12).** Con codificador Intel o NVIDIA, la fuente negocia `memory:DMABuf` con
PipeWire y el recorte se hace en el elemento de la GPU (`vapostproc` o `cudaconvert`) en vez de
`videocrop`, para que el cuadro nunca pase por la memoria de la CPU. Entra sólo si la medición
muestra que funciona y que mejora; si no, queda documentado por qué no.

**60 FPS (P14).** Objetivo: grabar el área a 60 FPS con menos de 1 % de cuadros perdidos en la
notebook. Si no se alcanza con ninguna variante, el informe de medición dice el máximo logrado y
por qué.

## 5 · Mantener los cuadros de la cámara (P15)

- Preferencia «Mantener los cuadros de la cámara» en la sección de la cámara, prendida por
  defecto (`camera-keep-framerate`).
- Al prender el PiP, si la fuente es un dispositivo V4L2 con el control
  `exposure_dynamic_framerate`, Kooha guarda su valor y lo pone en 0. Al apagar el PiP, o al
  cerrar Kooha, restituye el valor guardado.
- El control se aplica con `ioctl(VIDIOC_S_CTRL)` sobre el nodo `/dev/videoN` de la fuente;
  `v4l-utils` no es dependencia de Kooha.
- Si la fuente no tiene el control, la preferencia no hace nada y lo dice su subtítulo.

## Criterios de aceptación

1. En modo Selección, Marcar área abre una capa en cada uno de los tres monitores; arrastrar
   sobre DP-3, ajustar con manijas y flechas, tipear `1280x720` y Enter deja un área de
   1280×720 físicos en DP-3, y la etiqueta lo muestra.
2. Escape durante el marcado deja el área elegida como estaba.
3. Guardar con nombre la agrega a `rectangles.json` y queda elegida; sin nombre queda como
   último rectángulo y no figura en la lista.
4. Cerrar y abrir Kooha conserva la lista, el área elegida y si el borde está oculto.
5. El borde se ve alrededor del área elegida desde que se elige, antes de grabar; `Ctrl+B` lo
   oculta y lo muestra.
6. Grabar con el área elegida, con el permiso de KDE apuntando al mismo monitor, no muestra el
   diálogo de KDE ni el selector de upstream, y el video mide exactamente el área.
7. Con el permiso apuntando a otro monitor, Kooha avisa qué monitor elegir, KDE pregunta, y
   eligiendo el correcto graba; eligiendo otro, no graba y lo dice.
8. Un área en un monitor desconectado, o en uno que cambió de escala, aparece desactualizada en
   las tres listas y no se puede grabar.
9. La ventana Rectángulos renombra, borra (con confirmación), elige y vuelve a marcar.
10. La bandeja elige áreas, marca una nueva y oculta o muestra el borde.
11. El informe de medición existe con la tabla de variantes, y el pipeline elegido graba el
    área a 60 FPS con menos de 1 % de cuadros perdidos, o el informe explica el máximo.
12. La preferencia Codificador graba con cada opción disponible; Automático cae a CPU si el
    hardware falla.
13. Con «Mantener los cuadros de la cámara» prendida, prender el PiP deja
    `exposure_dynamic_framerate` en 0 (`v4l2-ctl -d /dev/video0 --get-ctrl
    exposure_dynamic_framerate`) y apagarlo lo restituye.
14. Sin capas disponibles (sesión X11), el modo Selección usa el selector de upstream como antes.
15. La suite pasa entera, con y sin `--features layer-shell`; clippy y `cargo +stable fmt
    --check` limpios.
