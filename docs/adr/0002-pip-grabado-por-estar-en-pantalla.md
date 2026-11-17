# ADR 0002 · El PiP se graba porque está en pantalla

**Estado:** aceptada (17/11/2026, grill P8 `pip-mecanismo`).

## Decisión

El PiP es una capa en pantalla que muestra la fuente de cámara. La captura de pantalla lo
graba como cualquier otra cosa visible. El pipeline de grabación no se toca.

## Consecuencias

- Lo que Sebastián ve es lo que queda grabado, y lo mueve con el mouse.
- Si el PiP queda fuera del área o del monitor grabado, no sale en el video. Grabando una
  ventana suelta tampoco sale.
- El espejado es uno solo para la pantalla y para el video: por eso se espeja antes de grabar
  y se desespeja al arrancar la grabación (grill P13 `pip-espejo`).
- La alternativa —componer la cámara dentro del pipeline— queda descartada para esta Task.
