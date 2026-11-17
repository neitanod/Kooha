# ADR 0003 · La pausa descarta buffers con el pipeline en PLAYING

**Estado:** aceptada (17/11/2026, grill P6 `pausa-archivo`).

## Contexto

Kooha pausaba poniendo el pipeline en `PAUSED`. Upstream lo desactivó en el commit `08816cc`
(23/09/2021, «Temporary disable pause»), y el código de pausa quedó sin botón. Con fuentes en
vivo (`pipewiresrc`, `pulsesrc`) un pipeline en `PAUSED` deja de producir y el comportamiento al
reanudar depende de cada elemento.

## Decisión

El pipeline queda siempre en `PLAYING`. Una **compuerta de pausa** con *pad probes* en la
entrada de la cola del video y de la del audio:

- mientras hay pausa, descarta los buffers;
- al reanudar, suma lo que duró la pausa a un desfasaje acumulado y se lo resta al PTS y al DTS
  de cada buffer siguiente, así el archivo no tiene hueco.

`RecordingState::Paused` lo fija la compuerta, ya no el mensaje `StateChanged` del pipeline.
Con el destello de reanudación activado, un `videobalance` en la rama de video pasa a blanco
(saturación 0, brillo 1) durante 150 ms de tiempo de video después de reanudar.

## Consecuencias

- El video y el audio saltan juntos, porque los corre el mismo desfasaje.
- La duración que muestra la ventana excluye las pausas.
- Hay que probar la compuerta con un pipeline real de GStreamer (`videotestsrc` y
  `audiotestsrc` en vivo), no con un doble.
