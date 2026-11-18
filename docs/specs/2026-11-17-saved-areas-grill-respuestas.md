# Respuestas del grill · Rectángulos guardados y rendimiento de la captura (Task 1092)

Cuestionario 92, contestado por Sebastián el 17/11/2026 a las 23:44 (Mensaje 15441 del Hilo 1149). Las preguntas completas, con el porqué y las notas de cada opción, están en `docs/specs/2026-11-17-saved-areas-grill.json`; acá va cada pregunta con lo elegido, tal como llegó. P<N> = pregunta N.


> grill `kooha-rectangulos-rendimiento-v1` · task 1092 · 11/17/2026, 11:44:59 PM

## A · Marcar el rectángulo

**1. ¿Qué pasa con el selector actual de Kooha (la ventanita que muestra la pantalla que mandó KDE y te deja recortar)?**

→ La capa reemplaza a la ventanita en el modo Selección *(the recommended one)*

**2. Después de soltar el arrastre, ¿qué se puede hacer antes de confirmar?**

→ Mover y redimensionar con manijas en bordes y esquinas *(the recommended one)*
→ Ver el ancho × alto en píxeles mientras arrastrás *(the recommended one)*
→ Tipear un tamaño exacto (por ejemplo 1280×720) y que el rectángulo se ajuste *(the recommended one)*
→ Mover de a un píxel con las flechas del teclado

**3. ¿En qué píxeles se muestran y se guardan las medidas del rectángulo?**

→ Píxeles del video (los físicos del monitor) *(the recommended one)*

**4. Al confirmar un rectángulo, ¿cómo se ofrece guardarlo?**

→ Aparece un campo «Guardar como…» optativo; si no le ponés nombre queda como «Último rectángulo» fuera de la lista *(the recommended one)*

## B · La lista de rectángulos

**5. ¿Desde dónde elegís un rectángulo guardado?**

→ Un desplegable en la ventana principal, al lado del modo Selección *(the recommended one)*
→ Una ventana «Rectángulos» para elegir, renombrar y borrar *(the recommended one)*
→ Un submenú en el ícono de la bandeja *(the recommended one)*

**6. Además del monitor y las coordenadas, ¿qué guarda cada rectángulo?**

→ Sólo monitor y coordenadas *(the recommended one)*

**7. ¿Dónde vive la lista?**

→ Un archivo `~/.config/kooha/rectangles.json` *(the recommended one)*

## C · El borde y la grabación

**8. ¿Cuándo se ve el borde punteado del rectángulo elegido?**

→ Desde que lo elegís hasta que empieza la grabación *(the recommended one)*
→ También durante la grabación, como hoy *(the recommended one)*
→ Un botón o atajo para ocultarlo y volverlo a mostrar *(the recommended one)*

**9. Si el rectángulo está en otro monitor que el que KDE recuerda de la grabación anterior, ¿qué hace Kooha?**

→ Le pide a KDE que pregunte, te avisa qué monitor elegir (por ejemplo «elegí DP-3 entero»), y si elegís otro no graba y te lo dice *(the recommended one)*

**10. Si el monitor del rectángulo cambió de resolución o de escala, o no está conectado, ¿qué hace Kooha?**

→ Lo marca como desactualizado en la lista, no deja grabarlo y ofrece volver a marcarlo *(the recommended one)*

## D · Rendimiento de la captura

**11. La medición de CPU y FPS necesita tu sesión de KDE real (el diálogo de compartir pantalla no se puede contestar solo). ¿Cómo la hacemos?**

→ Usar un token de KDE ya aceptado para que no pregunte, y medir todo yo solo *(the recommended one)*

  💬 Esta pregunta no entiendo si implica cambios en el programa o sólo cosas momentáneas durante el desarrollo.

**12. ¿Qué mejoras de la captura pruebo y mido?**

→ Recortar antes de escalar *(the recommended one)*
→ Codificar H.264 por hardware en la Intel (`vah264lpenc`) *(the recommended one)*
→ Codificar H.264 por hardware en la NVIDIA (`nvh264enc`) *(the recommended one)*
→ Pasar los cuadros de KDE al codificador por hardware sin copiarlos a la memoria de la CPU (DMA-BUF) *(the recommended one)*
→ Ajustar las colas del pipeline para que un pico del codificador no haga perder cuadros *(the recommended one)*

**13. ¿Cómo elige Kooha el codificador al grabar en MP4?**

→ Una preferencia «Codificador» con Automático, Intel, NVIDIA y CPU *(the recommended one)*

**14. ¿A cuántos FPS querés poder grabar sin perder cuadros?**

→ 60 FPS *(the recommended one)*

## E · La cámara con poca luz

**15. ¿Sumo «Mantener los cuadros de la cámara» con el control `exposure_dynamic_framerate`?**

→ Sí, en esta Task: instalo `v4l-utils`, miro si tu cámara lo tiene y, si lo tiene, sumo la preferencia (prendida por defecto) *(the recommended one)*

## F · Cierre

**16. ¿Qué me estoy olvidando de preguntar?**

→ ⏸ not answered

---

**Not answered:** #16


## Aclaraciones posteriores

- **P11** (Mensaje 15442): usar el permiso de KDE ya aceptado es sólo para el desarrollo y no cambia el programa. El script de medición reusa el `screencast-restore-token` de Kooha y al terminar le devuelve el token nuevo que entrega KDE.
- **P16** en blanco = «creo que nada».

## Datos verificados durante el grill

- Codificadores presentes en la notebook (GStreamer por `gi`): `vah264lpenc` y `vapostproc` (Intel, VA-API), `nvh264enc` y `cudaconvertscale` (NVIDIA), `x264enc`, `openh264enc`, `vp8enc`, `vp9enc`, `svtav1enc`. No está `vah264enc` (sólo la variante *low power*).
- La cámara integrada (`/dev/video0`, «Integrated Camera») tiene `exposure_dynamic_framerate` (bool, default 0, valor actual 1) y `auto_exposure` en «Aperture Priority Mode». Da 30 FPS sólo en MJPG hasta 1920×1080.
