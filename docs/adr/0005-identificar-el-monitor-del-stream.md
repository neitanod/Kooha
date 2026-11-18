# ADR 0005 · El monitor compartido se identifica por su tamaño físico

**Estado:** aceptada (17/11/2026, grill 1092 P9 `otro-monitor`, P10 `monitor-cambio`).

## Contexto

Kooha no puede elegir qué monitor comparte KDE: sólo puede pasarle el permiso guardado
(`restore_token`), con lo que KDE repite la elección anterior sin preguntar, o no pasarlo, con lo
que KDE pregunta. El portal de KDE no informa la posición del stream con varios monitores; sí
informa su tamaño. En la máquina de Sebastián los tres monitores tienen tamaños físicos distintos
(eDP-1 2560×1600, HDMI-A-5 1920×1080, DP-3 1680×1050).

## Decisión

- El stream corresponde al monitor del área si su tamaño coincide con el tamaño físico del
  monitor (geometría lógica × escala, con tolerancia de redondeo), como ya hace el borde del área.
- Si no coincide, Kooha cierra esa sesión del portal sin grabar, borra el permiso guardado, avisa
  «Elegí <conector> entero en el diálogo» y abre una sesión nueva, para que KDE pregunte. Si la
  segunda elección tampoco coincide, no graba y lo dice.
- Si el monitor del área no está conectado, o su tamaño físico o su escala cambiaron desde que se
  marcó, el área queda **desactualizada**: no se puede grabar y se ofrece volver a marcarla.

## Consecuencias

- Nunca se graba un recorte corrido.
- Si dos monitores tienen el mismo tamaño físico, Kooha no los distingue y acepta el stream. Lo
  dice el log; con los monitores actuales de Sebastián no pasa.
