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
