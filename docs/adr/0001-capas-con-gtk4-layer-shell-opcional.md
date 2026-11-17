# ADR 0001 · Capas sobre la pantalla con gtk4-layer-shell, opcional

**Estado:** aceptada (17/11/2026, grill P18 `layer-shell-dep`).

## Contexto

El PiP de la cámara y el borde del área tienen que dibujarse encima de todas las ventanas, en
una posición exacta del monitor. En Wayland una app no puede ubicar su ventana ni ponerla
encima de todo. `gtk4-layer-shell` implementa el protocolo *wlr-layer-shell*, que KWin (KDE)
soporta, y deja crear superficies en la capa `Overlay` con márgenes desde los bordes.

## Decisión

- Kooha usa el crate `gtk4-layer-shell` detrás de un *cargo feature* `layer-shell`. Meson lo
  activa cuando `dependency('gtk4-layer-shell-0', required: false)` lo encuentra.
- En tiempo de ejecución, las **capas están disponibles** sólo si el feature está compilado y
  `gtk4_layer_shell::is_supported()` devuelve `true`.
- Sin capas disponibles, el PiP y el borde del área quedan apagados. Lo dicen el reporte de
  requisitos y la página de Preferencias, al lado de esas opciones.

## Consecuencias

- Kooha sigue compilando y andando sin la biblioteca.
- En X11 o en GNOME (Mutter no soporta *wlr-layer-shell*) no hay PiP ni borde.
- La biblioteca C 1.x tiene que enlazarse antes que `libwayland-client`. El primer chunk del
  plan lo verifica con un ejemplo mínimo antes de construir encima.
