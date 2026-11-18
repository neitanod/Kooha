# ADR 0004 · El área se marca sobre la pantalla real, antes de grabar

**Estado:** aceptada (17/11/2026, grill 1092 P1 `relacion-selector`, P3 `unidad`).

## Contexto

El selector de upstream (`src/area_selector/`) se abre después de que KDE comparte la
pantalla: muestra el cuadro que mandó KDE en una ventana y deja recortarlo. Si en el diálogo de
KDE se elige «región», la selección sale doble, y como el portal no dice dónde quedó esa región,
Kooha no sabe en qué monitor ni en qué coordenadas está el área. Sin eso no hay borde ni
rectángulo que se pueda guardar.

El portal ScreenCast no deja que una app le pida una región a KDE: la región sólo la elige el
usuario en el diálogo. Lo que sí se puede es pedir un monitor entero y recortarlo.

## Decisión

- Con capas disponibles (ADR 0001), el modo Selección marca el área con una **capa de marcado**
  por monitor: una capa de pantalla completa, transparente y apenas oscurecida, sobre la que se
  arrastra el rectángulo. Se marca antes de grabar, sin pasar por el diálogo de KDE.
- El área queda definida por el **conector** del monitor (`DP-3`, `eDP-1`) y un rectángulo en
  **píxeles físicos** del monitor, que son los del video. Se guarda también el tamaño físico y la
  escala del monitor al momento de marcar.
- Al grabar, KDE comparte el monitor entero y Kooha lo recorta.
- Sin capas disponibles (X11, GNOME), el modo Selección sigue usando el selector de upstream.

## Consecuencias

- Una sola selección, sobre lo que se ve, y Kooha conoce monitor y coordenadas.
- Un área vive en un solo monitor: el arrastre queda limitado al monitor donde empezó.
- El cuadro entero del monitor viaja por el pipeline hasta el recorte. El codificador trabaja
  sólo con el área.
- Elegir «región» en el diálogo de KDE deja de tener sentido en modo Selección: Kooha pide
  monitor (`SourceType::MONITOR`).
