# Medición de la captura (Task 1092)

**Estado:** parcial. La fuente sintética está medida; la medición con el portal de KDE (la que
decide el orden de Automático y si entra DMA-BUF) está pendiente de que Sebastián autorice
correrla en su sesión (Mensaje 15452).

## Máquina

- CPU: 13th Gen Intel Core i7-13650HX, 20 hilos.
- GPU: Intel Raptor Lake-S UHD (driver `intel-media-va-driver` 26.1.2) y NVIDIA GeForce RTX
  4060 Laptop (driver 580.178.04).
- GStreamer 1.28.2. Área: 1280×720 en (200, 160) de un cuadro de 2560×1600.

## Fuente sintética (`--source test`, 20 s por variante)

`videotestsrc pattern=ball` genera cada cuadro de 2560×1600 en la CPU, y ese trabajo entra en la
columna «cpu %». Por eso las diferencias de CPU entre variantes salen chicas: miden el pipeline
más la fuente.

Source: test, 60 FPS, 20 s per variant, area 1280x720 at (200, 160)

| variant | started | startup s | source fps | fps | source frames | frames | lost | dup | drop | cpu % | error |
|---|---|---|---|---|---|---|---|---|---|---|---|
| today | yes | 0.4 | 60.0 | 60.0 | 1221 | 1222 | 0 | 0 | 0 | 116 |  |
| crop-first | yes | 0.4 | 60.0 | 60.0 | 1221 | 1222 | 0 | 0 | 0 | 114 |  |
| crop-first-queues | yes | 0.4 | 60.0 | 60.0 | 1221 | 1222 | 0 | 0 | 0 | 113 |  |
| intel | yes | 0.1 | 49.6 | 49.7 | 995 | 996 | 0 | 0 | 0 | 92 |  |
| intel-convert | yes | 0.1 | 60.0 | 60.1 | 1202 | 1203 | 0 | 0 | 0 | 102 |  |
| nvidia | yes | 0.2 | 60.0 | 60.1 | 1202 | 1203 | 0 | 0 | 0 | 89 |  |

Source: test, 30 FPS, 20 s per variant, area 1280x720 at (200, 160)

| variant | started | startup s | source fps | fps | source frames | frames | lost | dup | drop | cpu % | error |
|---|---|---|---|---|---|---|---|---|---|---|---|
| today | yes | 0.7 | 30.0 | 30.0 | 621 | 622 | 0 | 0 | 0 | 75 |  |
| crop-first | yes | 0.7 | 30.0 | 30.0 | 621 | 622 | 0 | 0 | 0 | 76 |  |
| crop-first-queues | yes | 0.7 | 30.0 | 30.0 | 621 | 622 | 0 | 0 | 0 | 74 |  |
| intel | yes | 0.1 | 30.0 | 30.1 | 602 | 603 | 0 | 0 | 0 | 60 |  |
| intel-convert | yes | 0.1 | 30.0 | 30.1 | 602 | 603 | 0 | 0 | 0 | 69 |  |
| nvidia | yes | 0.2 | 30.0 | 30.1 | 602 | 603 | 0 | 0 | 0 | 56 |  |

## Lectura provisional

- **Recortar antes de escalar** casi no cambia la CPU en la fuente sintética (116 → 114 % a 60
  FPS). Creo que es porque, con un cuadro de medidas pares, el `videoscale` de hoy no escala:
  pasa el cuadro sin tocarlo. El beneficio real es sacar un elemento del camino y no depender de
  esa casualidad.
- **`intel`** (`vapostproc` pegado a `videocrop`) no sostiene 60 FPS: 49,6. Según el
  implementador del chunk 3, el pool de memoria VA llega hasta la fuente y el cuadro de
  2560×1600 se dibuja en esa memoria. **`intel-convert`** (convertir a NV12 en la CPU antes de
  `vapostproc`) sostiene 60.
- **`nvidia`** sostiene 60 con la menor CPU de las variantes (89 % contra 113 % de
  `crop-first-queues`). Arranca en 0,2 s; con la placa dormida el arranque fue de varios
  segundos.
- A 30 FPS todas las variantes llegan sin pérdidas; las de hardware gastan menos CPU (56 a 69 %
  contra 74 a 76 %).

**Orden provisional de Automático:** Intel con conversión previa, NVIDIA, CPU. Intel va primero
porque `intel-convert` sostiene 60 FPS sin pérdidas (60,0 sobre 60) y, como pide la spec, gasta
menos batería y deja dormida la placa NVIDIA. NVIDIA gasta menos CPU (89 % contra 102 %), pero
despertarla cuesta batería y, con la placa dormida, varios segundos de arranque. El orden se
confirma con la medición del portal, que mide la fuente real de KDE.

## Calidad de los codificadores

Las cadenas de hardware fijan un cuantizador constante de 17, el `qp-max=17` de la cadena de CPU
(`x264enc`), para que el texto en pantalla salga igual de nítido con cualquiera de los tres. Los
nombres y los valores salen de `list_properties()` de cada elemento con Python gi, en esta
máquina (GStreamer 1.28.2):

| codificador | propiedades | valores por defecto que reemplazan |
|---|---|---|
| `vah264lpenc` | `rate-control=cqp qpi=17 qpp=17 qpb=17` | `rate-control=cbr`, `qpi`/`qpp`/`qpb` en 26 |
| `nvh264enc` | `rc-mode=constqp qp-const=17` | `rc-mode=default`, `qp-const=-1` |
| `x264enc` | `qp-max=17` (sin cambios) | `qp-max=51` |

`vah264lpenc` en esta máquina admite `cbr`, `vbr`, `cqp` y `qvbr`. Las dos cadenas codificaron 30
cuadros de 1280×720 hasta el EOS con esos valores. Las mediciones de las tablas de arriba son
anteriores a este cambio: corrieron con los valores por defecto de cada elemento.
`tools/bench-capture.py` usa desde entonces los mismos valores, y la cola antes del codificador
con el tope de 256 MiB del código, así que la medición con el portal mide las cadenas de Kooha.

## Portal de KDE

Pendiente.
