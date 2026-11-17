# Respuestas de Sebastián al grill (Cuestionario 91, Task 1088)

Fuente: Mensaje 15325 del Hilo 1145, que transcribe lo que Sebastián dictó por voz en el Hilo de la Task 1090 (Mensajes 15285 a 15323). Lo que dice NUEVO lo agregó él fuera de las opciones.

Lo que se decide acá no se discute en las rondas de revisión: si el plan o el código lo contradicen, es un hallazgo.

## A · Avisos de audio y puntero

### P1 · `aviso-donde`

**Pregunta:** ¿Dónde aparece «Without audio» / «Without mouse pointer» cuando arranca la grabación?

- `notificacion`: Notificación del escritorio al arrancar — Se ve aunque la ventana esté minimizada. Desaparece sola a los pocos segundos.
- `cartel-ventana`: Cartel fijo en la ventana de grabación mientras dura — Queda a la vista todo el tiempo, al lado del cronómetro.
- `cuenta-regresiva`: También en la cuenta regresiva (si hay demora configurada) — Te da los segundos de la demora para cancelar y corregir antes de que empiece.

**Respuesta:** notificacion + cartel-ventana + cuenta-regresiva (recomendación)

### P2 · `aviso-audio-que`

**Pregunta:** Kooha tiene dos interruptores de audio: el del escritorio (`record-desktop-audio`) y el del micrófono (`record-microphone`). ¿Qué dice el aviso?

- `por-fuente`: Un aviso por cada fuente apagada — «Without microphone» y/o «Without desktop audio». Con las dos apagadas, «Without audio».
- `solo-ambas`: «Without audio» sólo cuando las dos están apagadas — Más simple, pero no avisa si grabás el escritorio y te olvidaste del micrófono.

**Respuesta:** por-fuente (recomendación)

## B · Ícono de la bandeja del sistema

### P3 · `bandeja-cuando`

**Pregunta:** ¿Cuándo está el ícono en la bandeja?

- `siempre`: Siempre que Kooha está abierto — Sirve también para arrancar una grabación o cambiar las opciones antes de grabar.
- `grabando`: Sólo mientras graba — No ocupa lugar en la bandeja cuando no hace nada.

**Respuesta:** siempre (recomendación)

### P4 · `bandeja-items`

**Pregunta:** ¿Qué trae el menú contextual del ícono?

- `estado`: Estado: grabando / en pausa, y el tiempo transcurrido
- `start-stop`: Empezar y terminar la grabación
- `pausa`: Pausar y reanudar — Pedido explícito.
- `toggles`: Interruptores de micrófono, audio del escritorio y puntero — Con una tilde que muestra el estado actual.
- `pip`: Prender y apagar el PiP de la cámara
- `ventana-salir`: Mostrar la ventana y salir

**Respuesta:** estado + start-stop + pausa + toggles + pip + ventana-salir (recomendación)

### P5 · `toggles-durante`

**Pregunta:** ¿Qué hacen los interruptores de audio y puntero de la bandeja mientras se está grabando?

- `proxima`: Durante la grabación se ven pero no se tocan; cambian la próxima grabación — Simple y sin sorpresas. El aviso del arranque sigue siendo la red de seguridad.
- `mute-vivo`: El audio se silencia y se reactiva en vivo; el puntero cambia sólo la próxima — Las fuentes de audio entran siempre al pipeline, apagadas en volumen cero si estaban desactivadas. El archivo siempre tiene pista de audio, aunque sea silencio.
- `ahora-a-despues-b`: Ahora la primera; el mute en vivo en otra Task

**Respuesta:** proxima (recomendación)

## C · Pausa

### P6 · `pausa-archivo`

**Pregunta:** ¿Qué queda en el video final en el tramo pausado?

- `corte`: Nada: el video salta del momento de pausa al de reanudar — Como una grabadora clásica. El archivo dura lo que estuvo grabando.
- `congelado`: La imagen congelada durante lo que duró la pausa — El archivo dura el tiempo real; poco útil para un screencast.

**Respuesta:** corte (recomendación). NUEVO: opción en Preferencias para marcar el salto con un destello blanco breve en el video al reanudar; si la opción está apagada, corte directo

### P7 · `pausa-donde`

**Pregunta:** ¿Desde dónde se pausa, además del ícono de la bandeja?

- `boton-ventana`: Botón de pausa en la ventana de Kooha mientras graba — Ya existe oculto en `window.ui`: es mostrarlo y conectarlo a la pausa nueva.
- `atajo-ventana`: Atajo Ctrl+K con la ventana de Kooha enfocada — Ya está escrito y comentado en `application.rs`.
- `atajo-global`: Atajo global, aunque Kooha no tenga el foco — En Wayland va por el portal GlobalShortcuts, que KDE soporta. Suma trabajo.
- `barra-flotante`: Una barra flotante chica siempre encima, con pausa y stop — Con gtk4-layer-shell. Ocupa lugar en pantalla y hay que cuidar que no quede grabada.

**Respuesta:** boton-ventana + atajo-ventana (recomendación)

## D · Picture-in-picture de la cámara

### P8 · `pip-mecanismo`

**Pregunta:** ¿Cómo entra la cámara al video?

- `ventana-en-pantalla`: Una capa sobre la pantalla con la cámara, que se graba porque está en pantalla — Ves lo mismo que se graba y la arrastrás con el mouse. Sólo sale si queda dentro de lo que grabás: no aparece si grabás una ventana suelta. Requiere gtk4-layer-shell.
- `compuesto`: Compuesta dentro del pipeline de GStreamer, con un marco en pantalla que marca dónde va — Sale siempre, también grabando una ventana suelta, y con calidad de cámara completa. Más complejo: hay que traducir la posición del marco a coordenadas del video y sincronizar dos fuentes.
- `ahora-a-despues-b`: Ahora la capa en pantalla; el compuesto en otra Task si hace falta

**Respuesta:** ventana-en-pantalla (recomendación)

### P9 · `pip-forma`

**Pregunta:** ¿Qué formas tiene el PiP?

- `circulo`: Círculo
- `rectangulo`: Rectángulo
- `redondeado`: Rectángulo con esquinas redondeadas

**Respuesta:** NUEVO: dos formas, círculo y rectángulo con esquinas redondeadas; el botón DERECHO del mouse sobre el PiP las alterna (corrige una primera versión que decía botón del medio)

### P10 · `pip-tamano`

**Pregunta:** ¿Cómo se cambia el tamaño del PiP?

- `presets`: Chico / mediano / grande en Preferencias
- `rueda`: Con la rueda del mouse encima del PiP
- `esquina`: Arrastrando un borde — Con un círculo no hay esquina natural de donde agarrar.

**Respuesta:** rueda del mouse encima del PiP agranda y achica

### P11 · `pip-fuente`

**Pregunta:** ¿Dónde elegís qué fuente de video va en el PiP?

- `preferencias`: En Preferencias
- `bandeja`: En un submenú del ícono de la bandeja
- `ventana`: En la ventana principal, junto a los interruptores de audio y puntero

**Respuesta:** preferencias + ventana (recomendación)

### P12 · `pip-cuando`

**Pregunta:** ¿Cuándo se ve el PiP en pantalla?

- `al-activarlo`: Desde que lo activás, también antes de grabar — Lo podés acomodar antes de apretar grabar.
- `solo-grabando`: Sólo durante la grabación — Hay que moverlo con la grabación ya corriendo.

**Respuesta:** al-activarlo (recomendación)

### P13 · `pip-espejo`

**Pregunta:** ¿La imagen de la cámara va espejada?

- `espejo`: Espejada, como un espejo — Natural para vos; un texto detrás tuyo sale al revés.
- `normal`: Sin espejar
- `opcion`: Una opción en Preferencias, espejada por defecto

**Respuesta:** NUEVO: espejada antes de grabar (para acomodarse), sin espejar mientras graba (para que quien mira el video lo vea como siempre). Como el PiP se graba por estar en pantalla, al arrancar la grabación la imagen en pantalla deja de estar espejada

## E · Borde del área rectangular

### P14 · `borde-cuando`

**Pregunta:** ¿Cuándo se ve el rectángulo blanco punteado?

- `grabando`: Mientras graba y en pausa
- `cuenta`: Durante la cuenta regresiva
- `seleccionada`: Siempre que haya un área elegida, aunque no grabe — Puede molestar al trabajar.

**Respuesta:** grabando (incluye pausa) + cuenta (recomendación)

### P15 · `borde-estilo`

**Pregunta:** ¿Cómo se dibuja?

- `blanco`: Blanco punteado, 2 px — Lo que pediste. Sobre una ventana blanca casi no se ve.
- `blanco-sombra`: Blanco punteado con un contorno oscuro fino — Se ve sobre fondo claro y oscuro.

**Respuesta:** NUEVO: línea opaca de dos colores, punteado blanco sobre gris clarito (los huecos del punteado son gris claro, sin transparencia)

### P16 · `layer-shell-dep`

**Pregunta:** ¿Aceptás `gtk4-layer-shell` como dependencia nueva para el borde y el PiP?

- `si-opcional`: Sí, y si falta en la máquina el borde y el PiP se apagan y el reporte de requisitos lo dice — Kooha sigue andando sin ella.
- `si-obligatoria`: Sí, como requisito duro: sin ella no compila
- `no`: No: buscar otra forma — En KDE Wayland la alternativa es una ventana común más una regla de KWin para mantenerla encima, que tendrías que configurar a mano.

**Respuesta:** si-opcional (recomendación). NUEVO: si falta, además del reporte de requisitos lo avisa la pantalla de Preferencias, junto a las opciones de borde y PiP

## F · Reporte de requisitos

### P17 · `reporte-chequeos`

**Pregunta:** ¿Qué revisa el reporte?

- `gst`: Elementos de GStreamer: `pipewiresrc`, codificadores y muxers de cada perfil, captura de audio
- `portal`: Portal de captura de pantalla: si responde, su versión y los modos de puntero que ofrece
- `pipewire`: PipeWire corriendo y su versión
- `audio`: Dispositivos de audio: salida del escritorio y micrófono
- `camaras`: Cámaras disponibles para el PiP
- `overlay`: Soporte de capas sobre la pantalla (`gtk4-layer-shell` y el compositor)
- `bandeja`: Que haya quien muestre el ícono de bandeja
- `carpeta`: Que la carpeta de guardado exista y se pueda escribir

**Respuesta:** los ocho (recomendación)

### P18 · `reporte-forma`

**Pregunta:** ¿Cómo se presenta?

- `dialogo`: Diálogo con ✓/✗ por ítem y, en cada ✗, cómo arreglarlo con el comando a correr
- `copiar`: Botón para copiar el reporte entero como texto — Para pegármelo a mí o a un issue.
- `cli`: También desde la terminal: `kooha --check` — Sirve cuando la ventana ni abre.
- `al-fallar`: Cuando una grabación falla, el error ofrece abrir el reporte

**Respuesta:** las cuatro (recomendación). NUEVO: el diálogo es un checklist con tildes y cruces rojas; debajo de cada falla (sólo de las fallas): descripción de la falla, qué deja de funcionar por eso, y el comando para arreglarla

## G · Generales

### P19 · `idioma`

**Pregunta:** ¿En qué idioma van los textos nuevos?

- `en-mas-es`: Inglés en el código, con la traducción al castellano en `po/es.po` — Sale en el idioma del sistema, como el resto de Kooha.
- `solo-en`: Sólo inglés

**Respuesta:** en-mas-es (recomendación)

### P20 · `upstream`

**Pregunta:** ¿Pensamos en mandar esto al Kooha original (SeaDve/Kooha)?

- `fork-limpio`: Es para nuestro fork, pero con un commit por feature y el estilo de Kooha — Deja la puerta abierta a proponer PRs después sin costo extra.
- `upstream`: Sí: cada feature lista para un PR — Suma tests y cuidado de compatibilidad con GNOME y X11 que vos no usás.
- `solo-fork`: Sólo fork, sin cuidar nada de eso

**Respuesta:** fork-limpio (recomendación)

### P21 · `alcance`

**Pregunta:** ¿Todo en este /ciclo, o por etapas?

- `todo`: Las siete mejoras en este /ciclo — La primera versión sale con todo.
- `etapas`: Primero avisos, bandeja, pausa y reporte; el PiP y el borde en otra Task — Sale antes lo que no necesita gtk4-layer-shell.

**Respuesta:** todo (recomendación), en esta misma Task, sin abrir otra; las mejoras en orden, con commits encadenados uno detrás del otro (cada uno hijo del anterior)

### P22 · `olvido`

**Pregunta:** ¿Qué me estoy olvidando de preguntar?

**Respuesta:** Usar el ciclo de revisión de /ciclo con Codex y Fable; si Codex se queda sin cuota, seguir sólo con Fable para no quedar esperando
