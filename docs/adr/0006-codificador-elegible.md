# ADR 0006 · Codificador H.264 elegible, con Automático por defecto

**Estado:** aceptada (17/11/2026, grill 1092 P12 `trucos`, P13 `codificador-defecto`, P14 `fps-objetivo`).

## Contexto

Los perfiles MP4 y Matroska codifican con `x264enc` por CPU. En la notebook están instalados
`vah264lpenc` (Intel, VA-API) y `nvh264enc` (NVIDIA, NVENC). El objetivo es grabar a 60 FPS sin
perder cuadros.

## Decisión

- Una preferencia **Codificador** (`video-encoder`) con cuatro valores: Automático, Intel, NVIDIA
  y CPU. Se aplica a los perfiles que codifican H.264 (`mp4` y `matroska-h264`).
- Automático prueba los codificadores en el orden que fije la medición de la Task 1092 y usa el
  primero que esté instalado y arranque. Si ninguno por hardware arranca, usa `x264enc`.
- Un codificador forzado que no está disponible es un error de la grabación, con su explicación
  en el reporte de requisitos.

## Consecuencias

- Upstream tenía un perfil experimental «WebM VA H264»; queda como está, fuera de esta decisión.
- El orden de Automático es un dato medido, escrito en el código con la fecha de la medición.
