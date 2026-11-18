# CONTEXT — glosario del fork de Kooha

Términos que usan la spec, el plan, los ADRs y el código de las mejoras de la Task 1088.
El código y sus comentarios están en inglés; entre paréntesis va el nombre que usa el código.

| Término | Qué es |
|---|---|
| **Grabación** (`Recording`) | Una sesión de captura, desde que se aprieta grabar hasta que el archivo queda escrito. Estados: `Init`, `Delayed` (cuenta regresiva), `Recording`, `Paused`, `Flushing`, `Finished` |
| **Cuenta regresiva** (`Delayed`) | La demora configurada en Preferencias antes de que empiece a grabar |
| **Aviso de captura** (`CaptureWarning`) | Lo que falta en la grabación que arranca: «Without microphone», «Without desktop audio», «Without audio» o «Without mouse pointer» |
| **Ícono de bandeja** (`Tray`) | El ícono de Kooha en la bandeja del sistema, por StatusNotifierItem (el protocolo D-Bus de íconos de bandeja de KDE y freedesktop), con su menú contextual |
| **Pausa** | Tramo en que la grabación sigue abierta pero no guarda cuadros ni audio. El archivo final salta del momento de la pausa al de reanudar |
| **Destello de reanudación** (`pause-flash`) | Opción de Preferencias: al reanudar, unos cuadros en blanco marcan el salto en el video |
| **Compuerta de pausa** (`PauseGate`) | La pieza del pipeline de GStreamer que descarta los buffers mientras hay pausa y corre las marcas de tiempo para que no quede hueco |
| **PiP** (`CameraPip`) | Picture-in-picture: la imagen de una fuente de video (webcam, capturadora, cámara virtual) dibujada en una capa sobre la pantalla. Sale en el video porque está en pantalla |
| **Forma del PiP** | Círculo o rectángulo con esquinas redondeadas. El botón derecho del mouse sobre el PiP las alterna |
| **Fuente de cámara** | Un dispositivo de la clase `Video/Source` que lista el `gst::DeviceMonitor` |
| **Capa** (layer surface) | Una superficie de `gtk4-layer-shell` (biblioteca que implementa el protocolo Wayland *wlr-layer-shell*): se dibuja encima de todo y en una posición fija del monitor. La usan el PiP y el borde del área |
| **Capas disponibles** | Kooha se compiló con `gtk4-layer-shell` y el compositor soporta el protocolo. Sin eso, el PiP y el borde del área se apagan |
| **Área** (`CaptureMode::Selection`) | El rectángulo de la pantalla que se graba. Kooha captura el monitor entero y lo recorta con `videocrop` |
| **Borde del área** (`AreaBorder`) | El rectángulo punteado blanco sobre gris claro, opaco, dibujado por fuera del área para que no quede grabado |
| **Reporte de requisitos** (`doctor`) | El checklist de lo que Kooha necesita del sistema: cada falla con su descripción, lo que deja de funcionar y el comando para arreglarla. Se abre desde el menú, desde el error de una grabación fallida, o con `kooha --check` |

## Términos de la Task 1092 (rectángulos guardados y rendimiento)

| Término | Qué es |
|---|---|
| **Capa de marcado** (`AreaMarker`) | Una capa de pantalla completa por monitor, transparente y apenas oscurecida, sobre la que se arrastra el área. Reemplaza al selector de upstream cuando hay capas disponibles (ADR 0004) |
| **Área marcada** (`MarkedArea`) | El resultado de marcar: conector del monitor, rectángulo en píxeles físicos, y tamaño físico y escala del monitor al marcar |
| **Píxeles físicos** | Los del monitor sin escalar, que son los del video. En la notebook, 1463 píxeles lógicos de ancho son 2560 físicos (escala 1,75) |
| **Rectángulo guardado** (`SavedArea`) | Un área marcada con nombre, en la lista `~/.config/kooha/rectangles.json` |
| **Último rectángulo** | El área marcada más reciente que no se guardó con nombre. Se puede elegir como cualquier otro, pero no figura en la lista |
| **Área elegida** | El rectángulo guardado o el último rectángulo que se va a grabar en modo Selección. Su borde se ve desde que se elige |
| **Área desactualizada** | Un área cuyo monitor no está conectado o cambió de tamaño físico o de escala desde que se marcó. No se puede grabar (ADR 0005) |
| **Ventana Rectángulos** (`AreasWindow`) | La ventana para elegir, renombrar, borrar y volver a marcar los rectángulos guardados |
| **Codificador** (`video-encoder`) | La preferencia que elige con qué se codifica H.264: Automático, Intel (`vah264lpenc`), NVIDIA (`nvh264enc`) o CPU (`x264enc`) (ADR 0006) |
| **Medición de captura** (`tools/bench-capture`) | El script de desarrollo que graba el mismo rectángulo con cada variante del pipeline y anota CPU, FPS reales y cuadros perdidos |
| **Mantener los cuadros de la cámara** (`camera-keep-framerate`) | Preferencia que pone `exposure_dynamic_framerate` en 0 al prender la cámara, para que no baje los FPS con poca luz, y restituye el valor anterior al apagarla |
