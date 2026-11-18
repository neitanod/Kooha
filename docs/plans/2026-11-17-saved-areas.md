# Rectángulos guardados y rendimiento de la captura · plan de implementación

> **Para quien implementa:** cada chunk es un subagente en su propio worktree, con su propia
> rama, que sale de la rama de trabajo. Los pasos usan casillas (`- [ ]`). TDD: primero el test
> que falla, después el código. Un commit por tarea como mínimo.

**Objetivo:** las cinco partes de `docs/specs/2026-11-17-saved-areas-design.md`: marcar el área
sobre la pantalla, rectángulos guardados con su lista y su borde, grabar el área elegida sin el
selector de upstream, una captura más liviana (recorte antes de escalar, colas, codificador por
hardware) y mantener los FPS de la cámara.

**Arquitectura:** la lógica va en funciones puras testeadas (`src/areas/model.rs`,
`src/area_marker/geometry.rs`, `src/encoder.rs`, `src/camera_pip/v4l2.rs`) y una capa de
cableado chica en `Window`, `Recording`, `Application`, la bandeja y Preferencias. La capa de
marcado usa `gtk4-layer-shell` igual que el PiP y el borde (ADR 0001, ADR 0004). El monitor del
stream se identifica por tamaño con la función que ya usa el borde (ADR 0005). El codificador es
una preferencia con Automático, resuelta con una prueba previa por sesión (ADR 0006).

**Stack:** Rust 2024 (toolchain 1.92.0 por `rustup override` en `/home/sebas/robotin/apps/kooha`),
gtk4 0.11 (feature `gnome_50`: `gdk::Monitor::scale()` y `connector()` disponibles),
libadwaita 0.9, gstreamer 0.25 (GStreamer 1.28.2 instalado), `gsettings-macro` 0.2, `ksni`
0.3.7, `gtk4-layer-shell` 0.8, `serde` + `serde_json` (nuevo), Meson. Medición: Python 3 + `gi`.

**Rama de trabajo:** `t-1092-rectangulos-guardados-y-rendimiento-de-la-captura`, que sale de
`main` (`2e4dac6`). Repo de trabajo:
`/home/sebas/robotin/apps/kooha/.workspace/t-1092-rectangulos-guardados-y-rendimiento-de-la-captura/repo`
(en adelante `$WS` es la carpeta de trabajo, el padre de `repo/`). El checkout original
`/home/sebas/robotin/apps/kooha` no se toca hasta la integración.

**Orden de entrega (Mensaje 15443 de Sebastián):** la revisión del plan corre en paralelo con
la implementación, y la primera versión instalable sale después de la tanda 2 (partes 1 a 3).

---

## Decisiones del grill que el plan tiene que respetar

Fuente: `docs/specs/2026-11-17-saved-areas-grill-respuestas.md`.

| P | Decisión | Dónde la cubre el plan |
|---|---|---|
| P1 | La capa de marcado reemplaza al selector de upstream en modo Selección; sin capas, upstream | Chunk 5, tarea 5.4 |
| P2 | Manijas, medidas mientras se arrastra, tamaño tipeado, flechas de a 1 píxel | Chunk 2, tareas 2.1 a 2.4 |
| P3 | Medidas en píxeles físicos (los del video) | Chunk 1, tarea 1.1; chunk 2, tarea 2.1 |
| P4 | «Guardar como…» optativo; sin nombre = último rectángulo fuera de la lista | Chunk 2, tarea 2.5; chunk 1, tarea 1.3 |
| P5 | Desplegable en la ventana principal, ventana Rectángulos y submenú de bandeja | Chunk 5, tarea 5.2; chunk 6 |
| P6 | Sólo monitor y coordenadas | Chunk 1, tarea 1.1 |
| P7 | `~/.config/kooha/rectangles.json` | Chunk 1, tarea 1.3 |
| P8 | Borde desde que se elige, durante la grabación, y un atajo/botón para ocultarlo | Chunk 5, tarea 5.3 |
| P9 | Monitor distinto: repreguntar a KDE, avisar cuál elegir, no grabar si no coincide | Chunk 5, tarea 5.4 |
| P10 | Monitor cambiado o desconectado: desactualizada, no se graba, ofrecer volver a marcar | Chunk 1, tarea 1.2; chunks 5 y 6 |
| P11 | Medición con el permiso de KDE ya aceptado; sólo herramienta de desarrollo | Chunk 3, tareas 3.1 y 3.4 |
| P12 | Recortar antes de escalar, Intel, NVIDIA, DMA-BUF, colas | Chunk 3, tareas 3.2 y 3.3; chunk 7 |
| P13 | Preferencia «Codificador»: Automático, Intel, NVIDIA, CPU | Chunk 7 |
| P14 | Objetivo 60 FPS | Chunk 3, tarea 3.4; chunk 7, tarea 7.4 |
| P15 | «Mantener los cuadros de la cámara», prendida por defecto | Chunk 4 |

---

## Comandos de verificación

Todos desde el worktree del chunk. `CARGO_HOME` y `--target-dir` reusan la caché del build de
Meson.

```bash
export CARGO_HOME=$PWD/_build/cargo-home
export KOOHA_TEST_RESOURCES=$PWD/_build/data/resources/resources.gresource
ninja -C _build                                             # build completo; genera los recursos
cargo test --target-dir _build/src                          # suite
cargo test --target-dir _build/src --features layer-shell   # suite con capas
cargo clippy --target-dir _build/src -- -D warnings         # linter (y con --features layer-shell)
cargo +stable fmt --check                                   # formato (rustfmt no viene en 1.92.0)
```

`ninja` va antes de `cargo test`: los tests de perfiles cargan el `resources.gresource` generado.
Estado de partida (`2e4dac6`): la suite pasa entera con y sin `--features layer-shell`.

**Archivos reales de Sebastián:** ningún test toca `glib::user_config_dir()`:
`areas::file::path()` bajo `#[cfg(test)]` devuelve `std::env::temp_dir()/kooha-test-<pid>/rectangles.json`
(tarea 1.3), y una copia de desarrollo de Kooha se lanza sólo con `run-dev.sh`, que aísla
`XDG_CONFIG_HOME`.

**Tests de GTK:** todo test que toca GTK usa `#[gtk::test]` (corre en un único hilo; ver
`src/timer.rs`). Ningún test llama `present()` sobre una ventana, para no dibujar en la pantalla
de Sebastián. Los tests que usan Settings toman `settings::test_utils::settings_lock()` y
llaman `setup_schema()` + `init_resources()` como `src/recording.rs:834-871`.

**Meson en cada worktree**, con prefijo propio (el binario carga los recursos desde el prefijo):

```bash
meson setup _build --prefix=$PWD/_install -Dprofile=development
mkdir -p _build/cargo-home && cp -r $WS/repo/_build/cargo-home/registry _build/cargo-home/
```

**Ejecutor de desarrollo:** `$WS/run-dev.sh <worktree> [args]` (lo escribe el orquestador en
la tarea 0.3) instala la copia del worktree en su `_install` y la corre con su propio schema y
su propio `XDG_CONFIG_HOME` (`$WS/dev-config`), así no toca `~/.config/kooha` ni la
configuración de Kooha instalado. Lo usa sólo el orquestador en la etapa 8; los chunks no abren
la interfaz.

---

## Estructura de archivos

| Archivo | Responsabilidad | Chunk |
|---|---|---|
| `src/areas/mod.rs` | Reexporta y declara submódulos | 0 |
| `src/areas/model.rs` | `MonitorInfo`, `MarkedArea`, `SavedArea`, `Staleness`, conversiones lógico↔físico, `staleness()` | 0 (tipos), 1 (lógica) |
| `src/areas/file.rs` | `AreasFile` (serde), carga y guardado atómico, archivo roto | 1 |
| `src/areas/store.rs` | `AreaStore` (GObject con señal `changed`): lista, elegida, último; persiste | 1 |
| `src/areas/monitors.rs` | `MonitorInfo` desde `gdk::Display` y seguimiento de cambios | 1 |
| `src/area_marker/geometry.rs` | Estado del marcado: arrastre, manijas, mover, flechas, tamaño tipeado, pares, mínimos | 2 |
| `src/area_marker/view.rs` | Capa de marcado por monitor: dibujo y eventos | 2 |
| `src/area_marker/mod.rs` | `AreaMarker::mark(initial) -> Result<MarkedArea>` | 2 |
| `src/area_marker/save_dialog.rs` | Diálogo «Área de W×H en X» con «Guardar como…» | 2 |
| `tools/bench-capture.py` | Medición de variantes del pipeline | 3 |
| `docs/perf/2026-11-18-capture-bench.md` | Informe de la medición | 3 |
| `src/pipeline.rs` | Recorte antes de escalar, `AreaCrop`, colas | 3 |
| `src/camera_pip/v4l2.rs` | Leer y escribir `exposure_dynamic_framerate` por ioctl | 4 |
| `src/camera_pip/mod.rs` | Aplicar y restituir el control al prender y apagar el PiP | 4 |
| `src/window/mod.rs`, `data/resources/ui/window.ui` | Fila de área, `ToastOverlay`, acciones, borde en reposo | 5 |
| `src/recording.rs` | Grabar con el área elegida, verificar el monitor, repreguntar | 5 |
| `src/area_border/mod.rs` | `for_area()`; `stream_monitor` pasa a `pub(crate)` | 5 |
| `src/areas_window.rs`, `data/resources/ui/areas_window.ui` | Ventana Rectángulos | 6 |
| `src/tray/model.rs`, `src/tray/service.rs`, `src/application.rs` | Submenú Área, casilla del borde, `app.show-areas` | 6 |
| `src/encoder.rs` | `H264Encoder`, cadenas, prueba previa, resolución de Automático | 7 |
| `src/profile.rs`, `data/resources/profiles.yml` | Marcador `${H264_ENCODER}` | 7 |
| `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui` | Filas Codificador (7) y cámara (4) | 4 y 7 (tandas distintas) |
| `data/io.github.seadve.Kooha.gschema.xml.in` | Claves nuevas | 0 |
| `po/es.po`, `po/POTFILES.in` | Traducciones | 8 |

---

## Grafo de chunks

| Chunk | Nombre | Tareas | Depende de | Archivos que toca | Tanda |
|---|---|---|---|---|---|
| 0 | Preparación (orquestador) | 0.1–0.3; 0.4 entre tandas | — | gschema, `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/areas/{mod,model}.rs` (tipos), `src/area_marker/mod.rs` (vacío), `$WS/run-dev.sh` | 0 |
| 1 | Modelo y lista de áreas | 1.1–1.4 | 0 | `src/areas/*` | 1 |
| 2 | Capa de marcado | 2.1–2.5 | 0 (y 1.1, 1.2, 1.4: ver nota) | `src/area_marker/*` | 1 |
| 3 | Medición y pipeline | 3.1–3.4 | 0 | `tools/`, `docs/perf/`, `src/pipeline.rs` | 1 |
| 4 | Cámara: mantener los cuadros | 4.1–4.3 | 0 | `Cargo.toml`, `Cargo.lock`, `src/camera_pip/v4l2.rs`, `src/camera_pip/mod.rs`, `src/camera_pip/devices.rs`, `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui` | 1 |
| 5 | Ventana, borde y grabación | 5.1–5.5 | 1, 2, 3 | `src/window/*`, `data/resources/ui/window.ui`, `src/recording.rs`, `src/area_border/mod.rs`, `src/area_border/geometry.rs` | 2 |
| 6 | Ventana Rectángulos y bandeja | 6.1–6.4 | 1, 2 (con 5 sólo por los nombres de acción `win.mark-area` y `win.remark-area`) | `src/areas_window.rs`, `data/resources/ui/areas_window.ui`, `data/resources/resources.gresource.xml`, `src/tray/*`, `src/application.rs`, `src/main.rs` (una línea `mod areas_window;`) | 2 |
| 7 | Codificador | 7.1–7.5 | 3, 4, 5 | `src/encoder.rs`, `src/profile.rs`, `data/resources/profiles.yml`, `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui`, `src/doctor/checks.rs`, `src/pipeline.rs`, `src/recording.rs` (sólo `CaptureConfig`), `src/main.rs` (una línea `mod encoder;`) | 3 |
| 8 | Traducciones e integración final (orquestador) | 8.1–8.2 | todos | `po/es.po`, `po/POTFILES.in`, `README.md` | 4 |

**Chunk 2 y chunk 1 en la misma tanda (fable r1 #5):** el chunk 2 necesita la conversión
lógico↔físico, `describe()` y la lista de monitores del chunk 1. Para no frenar la tanda, el
chunk 2 trabaja con helpers privados equivalentes y recibe `name_exists: impl Fn(&str) -> bool`
en `save_dialog::ask`; la **tarea 0.5** del orquestador, entre la tanda 1 y la 2, borra esos
helpers y deja una sola conversión, la de `areas::model`.

**El chunk 7 pasa a la tanda 3** (fable r1 #9): resuelve el codificador en `CaptureConfig`
(`src/recording.rs`), que es del chunk 5 en la tanda 2.

**Conflictos de la tanda 2 resueltos por reparto:** `src/recording.rs` sólo lo toca el chunk 5. La acción `app.show-areas` la define el
chunk 6 en `src/application.rs`; el botón «Rectángulos…» del chunk 5 sólo usa su nombre.

**Primera versión instalable:** al integrar los chunks 5 y 6 (etapa 8 de /ciclo). El chunk 7 entra en una instalación siguiente.

---

## Chunk 0 · Preparación (orquestador, antes de la tanda 1)

### Tarea 0.1: claves de configuración

**Archivos:** `data/io.github.seadve.Kooha.gschema.xml.in`.

- [ ] Agregar, después de `camera-monitor`:

```xml
    <key name="camera-keep-framerate" type="b">
      <default>true</default>
    </key>
    <key name="show-area-border" type="b">
      <default>true</default>
    </key>
    <key name="video-encoder" type="s">
      <choices>
        <choice value="auto"/>
        <choice value="intel"/>
        <choice value="nvidia"/>
        <choice value="cpu"/>
      </choices>
      <default>'auto'</default>
    </key>
```

- [ ] `ninja -C _build` compila; `gsettings-macro` genera `camera_keep_framerate()`,
  `show_area_border()`, `create_show_area_border_action()` y el enum `VideoEncoder`
  (`Auto`, `Intel`, `Nvidia`, `Cpu`).

### Tarea 0.2: dependencia y esqueleto de módulos

**Archivos:** `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/areas/mod.rs`,
`src/areas/model.rs`, `src/area_marker/mod.rs`.

- [ ] `Cargo.toml`: `serde_json = "1"` en `[dependencies]`.
- [ ] `src/main.rs`: `mod areas;` y `mod area_marker;` en orden alfabético.
- [ ] `src/areas/mod.rs`: `pub mod model;` y `pub use model::*;`.
- [ ] `src/areas/model.rs`, sólo los tipos (la lógica es del chunk 1; los dos chunks de la tanda
  1 compilan contra estos tipos):

```rust
//! Marked areas: a rectangle on one monitor, in physical (video) pixels.

use serde::{Deserialize, Serialize};

/// A connected monitor, as the marker and the store see it.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// The connector name, like `DP-3`.
    pub connector: String,
    /// Logical rectangle in the global layout: (x, y, width, height).
    pub logical: (i32, i32, i32, i32),
    /// Fractional scale, like 1.75.
    pub scale: f64,
}

/// An area on one monitor, in physical pixels of that monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkedArea {
    pub connector: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Physical size and scale of the monitor when the area was marked.
    pub monitor_width: i32,
    pub monitor_height: i32,
    pub scale: f64,
}

/// A marked area with a name, in the saved list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedArea {
    pub name: String,
    #[serde(flatten)]
    pub area: MarkedArea,
}

/// Why an area can't be recorded any more.
#[derive(Debug, Clone, PartialEq)]
pub enum Staleness {
    Disconnected,
    SizeChanged { from: (i32, i32), to: (i32, i32) },
    ScaleChanged { from: f64, to: f64 },
}
```

- [ ] `src/area_marker/mod.rs`: vacío salvo un comentario de módulo; el chunk 2 lo llena.
- [ ] `ninja -C _build` y la suite pasan (con `#[allow(dead_code)]` en el módulo si clippy se
  queja de tipos sin usar; el chunk 1 lo saca).
- [ ] Commit: `chore: add the areas module skeleton and the new settings keys`.

### Tarea 0.3: ejecutor de desarrollo

- [ ] Escribir `$WS/run-dev.sh` (fuera del repo), copiando el de la Task 1088 con
  `XDG_CONFIG_HOME=$WS/dev-config` y grabaciones en `$WS/dev-recordings`.

---

## Chunk 1 · Modelo y lista de áreas (tanda 1)

### Tarea 1.1: conversiones lógico↔físico

**Archivos:** `src/areas/model.rs`.

- [ ] **Test que falla** (en `model.rs`, `mod tests`), con los monitores reales de Sebastián:

```rust
fn edp() -> MonitorInfo {
    // GDK reports eDP-1 as 1463x914 (see the area_border tests); kscreen-doctor says 915.
    MonitorInfo { connector: "eDP-1".into(), logical: (457, 1080, 1463, 914), scale: 1.75 }
}
fn dp3() -> MonitorInfo {
    MonitorInfo { connector: "DP-3".into(), logical: (1920, 454, 1600, 1000), scale: 1.05 }
}

#[test]
fn physical_size_rounds_logical_times_scale() {
    assert_eq!(edp().physical_size(), (2560, 1600)); // 1463*1.75=2560.25, 914*1.75=1599.5
    assert_eq!(dp3().physical_size(), (1680, 1050));
}

#[test]
fn to_physical_and_back_are_monitor_local() {
    let m = dp3();
    // (100, 80) logical inside DP-3 -> physical
    assert_eq!(m.to_physical(100.0, 80.0), (105, 84));
    let (lx, ly) = m.to_logical(105, 84);
    assert!((lx - 100.0).abs() < 0.01 && (ly - 80.0).abs() < 0.01);
}
```

- [ ] Correr `cargo test --target-dir _build/src areas::model` → falla (no existen los métodos).
- [ ] Implementar en `impl MonitorInfo`:
  - `physical_size(&self) -> (i32, i32)`: `round(w*scale)`, `round(h*scale)`.
  - `to_physical(&self, lx: f64, ly: f64) -> (i32, i32)`: coordenadas **locales** del monitor
    (no globales), `round(l*scale)`.
  - `to_logical(&self, px: i32, py: i32) -> (f64, f64)`: `p / scale`.
- [ ] Un segundo test con el alto 915 documenta que da 1601, dentro de la tolerancia ±2 que usan
  `staleness` y `stream_monitor`.
- [ ] `MarkedArea::fits(&self, monitor: &MonitorInfo) -> bool`: el rectángulo entra en el tamaño
  físico actual del monitor (lo usa la capa de marcado para decidir si precarga). Test.
- [ ] Pasa. Commit `feat: convert marked areas between logical and physical pixels`.

### Tarea 1.2: área desactualizada

**Archivos:** `src/areas/model.rs`.

- [ ] **Tests que fallan:**

```rust
fn area_on_dp3() -> MarkedArea {
    MarkedArea { connector: "DP-3".into(), x: 100, y: 80, width: 1280, height: 720,
                 monitor_width: 1680, monitor_height: 1050, scale: 1.05 }
}

#[test]
fn fresh_when_monitor_matches() {
    assert_eq!(area_on_dp3().staleness(&[edp(), dp3()]), None);
}

#[test]
fn disconnected_when_connector_missing() {
    assert_eq!(area_on_dp3().staleness(&[edp()]), Some(Staleness::Disconnected));
}

#[test]
fn scale_changed_beyond_tolerance() {
    let mut m = dp3();
    m.scale = 1.25;
    m.logical = (1920, 454, 1344, 840); // same physical size, other scale
    assert_eq!(area_on_dp3().staleness(&[m]),
               Some(Staleness::ScaleChanged { from: 1.05, to: 1.25 }));
}

#[test]
fn size_changed_when_physical_size_differs() {
    let mut m = dp3();
    m.logical = (1920, 454, 1920, 1200);
    m.scale = 1.0;
    assert!(matches!(area_on_dp3().staleness(&[m]), Some(Staleness::SizeChanged { .. })));
}
```

- [ ] Implementar `MarkedArea::staleness(&self, monitors: &[MonitorInfo]) -> Option<Staleness>`:
  busca por `connector`; si no está, `Disconnected`; si `physical_size()` difiere de
  `(monitor_width, monitor_height)` en más de 2 en algún eje, `SizeChanged`; si la escala
  difiere en más de 0.01, `ScaleChanged`; si no, `None`. El orden de chequeo es ese (tamaño
  antes que escala).
- [ ] Implementar `MarkedArea::describe(&self) -> String`: `"1280×720 en DP-3"` en el idioma de
  la interfaz (`gettext("{width}×{height} on {connector}")` con reemplazos, como hace el repo en
  `src/capture_warnings.rs`). Test: el texto sin traducir es `1280×720 on DP-3`.
- [ ] Implementar `Staleness::describe(&self, connector: &str) -> String`:
  `"DP-3 is not connected"`, `"DP-3 changed from 1680×1050 to 1920×1200"`,
  `"DP-3 changed scale from 1.05 to 1.25"` (en `gettext`). Un test por variante.
- [ ] Commit `feat: tell when a marked area no longer matches its monitor`.

### Tarea 1.3: archivo `rectangles.json`

**Archivos:** `src/areas/file.rs`, `src/areas/mod.rs`.

Formato exacto de la spec, sección 2:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] // Default a mano: version 1
pub struct AreasFile {
    pub version: u32,                 // 1
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<MarkedArea>,
    #[serde(default)]
    pub areas: Vec<SavedArea>,
    /// `Some(Selected::Named(name))`, `Some(Selected::Last)`, or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "selected_serde")]
    pub selected: Option<Selected>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selected { Named(String), Last }
```

`selected` en el JSON: ausente = `None`, `null` = `Last`, string = `Named`. Un `null` de serde
se deserializa como `None` externo igual que ausente, así que `selected_serde` lleva un
`deserialize_with` que envuelve en `Some`, más `#[serde(default, skip_serializing_if =
"Option::is_none")]` para que «ninguna elegida» no se escriba como `null`. `Selected` deriva
`Debug, Clone, PartialEq, Eq, Hash` (la bandeja lo mete en un enum con `Eq`). `Default` se
implementa a mano con `version: 1`. Un archivo con `version` mayor a 1 da
`LoadError::UnsupportedVersion`: no se renombra ni se sobrescribe, y el store queda vacío y en
sólo lectura: cada cambio vale en memoria por esa sesión y emite `save-failed` con «rectangles.json
was written by a newer Kooha» (fable r2 #10; test). `load_notice()` (que reemplaza a
`broken_notice()`) avisa una vez del archivo roto, de un error de lectura o de la versión nueva.

- [ ] **Tests que fallan** (directorios temporales con `std::env::temp_dir()` y el PID, como
  `settings::test_utils`):
  - `round_trip` con `selected = Named`, con `Last` y con `None`: guardar y volver a leer da lo
    mismo; el JSON escrito contiene `"selected": "Demo"`, `"selected": null` y ninguna clave
    `selected`, respectivamente.
  - `path_is_temporary_in_tests`: `path()` no empieza con `glib::user_config_dir()`.
  - `newer_version_is_left_alone`: `{"version": 2}` da `UnsupportedVersion` y el archivo queda.
  - `selected_null_means_last` y `selected_absent_means_none`, leyendo strings JSON literales.
  - `missing_file_is_empty`: `load(path)` de un archivo que no existe devuelve
    `Ok(AreasFile::default())` con `version` 1.
  - `broken_file_is_moved_aside`: con `{ not json` en el archivo, `load` devuelve
    `Err(LoadError::Broken { moved_to })`, el archivo original ya no existe y `moved_to` existe
    y se llama `rectangles.json.broken-<AAAAMMDD-HHMMSS>`.
  - `save_is_atomic`: `save` escribe en `rectangles.json.tmp-<pid>` y renombra; después de
    `save` no queda el temporal.
- [ ] Implementar `pub fn path() -> PathBuf` (`glib::user_config_dir()/kooha/rectangles.json`,
  que respeta `XDG_CONFIG_HOME`; bajo `cfg(test)`, el temporal de arriba), `pub fn load(path: &Path) -> Result<AreasFile, LoadError>` y
  `pub fn save(path: &Path, file: &AreasFile) -> io::Result<()>` (crea el directorio, escribe
  `serde_json::to_string_pretty`, `fsync`, `rename`).
- [ ] Commit `feat: persist marked areas in rectangles.json`.

### Tarea 1.4: `AreaStore` y monitores

**Archivos:** `src/areas/store.rs`, `src/areas/monitors.rs`, `src/areas/mod.rs`.

`AreaStore` es un `glib::Object` (subclase con `glib::wrapper!`, como `Recording`) con una
señal `changed` sin argumentos y este API:

```rust
impl AreaStore {
    pub fn load_default() -> Self;               // usa areas::file::path(); archivo roto -> lista vacía y `broken_notice()`
    pub fn with_path(path: PathBuf) -> Self;     // para tests
    pub fn broken_notice(&self) -> Option<PathBuf>; // dónde quedó el archivo roto, una vez
    pub fn areas(&self) -> Vec<SavedArea>;
    pub fn last(&self) -> Option<MarkedArea>;
    pub fn selected(&self) -> Option<Selected>;
    pub fn selected_area(&self) -> Option<ChosenArea>;   // None también si el nombre ya no existe
    pub fn select(&self, selected: Option<Selected>);
    pub fn set_last(&self, area: MarkedArea);            // y la elige
    pub fn save_named(&self, name: &str, area: MarkedArea, replace: bool) -> Result<(), NameTaken>; // y la elige
    pub fn rename(&self, from: &str, to: &str) -> Result<(), NameTaken>; // mantiene la elección
    pub fn remove(&self, name: &str);                     // si era la elegida, queda sin elegida
    pub fn replace_geometry(&self, name: &str, area: MarkedArea);
    pub fn contains(&self, name: &str) -> bool;
}
```

Contrato único del área elegida (codex r1 #3), en `src/areas/model.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ChosenArea { pub name: Option<String>, pub area: MarkedArea } // name None = último
```

`area_marker::mark` recibe `Option<&MarkedArea>` (quien llama pasa `chosen.as_ref().map(|c|
&c.area)`), y `CaptureConfig.area` es `Option<ChosenArea>`, para que el error de área
desactualizada tenga el nombre.

Cada método que cambia algo guarda el archivo y emite `changed`. Antes de escribir, si `(inodo, fecha de modificación, tamaño)` del archivo cambió desde la
última lectura o escritura propia (fable r2 #9), lo relee y aplica
la operación sobre lo leído (otro proceso de Kooha pudo escribirlo). Un error al guardar se
loguea con `tracing::error!` y emite la señal `save-failed(String)`, que la ventana muestra como
toast; la lista en memoria sigue valiendo.

- [ ] **Tests que fallan** (`#[gtk::test]` no hace falta: son objetos GLib; alcanza
  `#[test]`), uno por comportamiento: guardar con nombre lo elige y persiste; nombre repetido
  sin `replace` da `NameTaken` y no cambia nada; con `replace` reemplaza en el mismo lugar de la
  lista; renombrar la elegida mantiene la elección; borrar la elegida deja `selected() == None`;
  `set_last` reemplaza el último y lo elige; `changed` se emite una vez por operación (contador
  con `connect_closure`); recargar desde el mismo path recupera todo; un cambio externo del
  archivo entre dos operaciones no se pisa; un directorio sin permiso de escritura emite
  `save-failed`.
- [ ] `src/areas/monitors.rs`: `pub fn current(display: &gdk::Display) -> Vec<MonitorInfo>`
  (de `display.monitors()`, sólo `is_valid()`, `connector()` o `"unknown-<n>"`, `geometry()`,
  `scale()`), `pub fn monitor_for(display, connector) -> Option<gdk::Monitor>` y
  `pub fn watch(display, callback) -> WatchGuard`, que conecta `items_changed` de la lista y, por
  monitor, `notify::geometry` y `notify::scale` (como `src/camera_pip/mod.rs:396`), para que un
  cambio de escala sin desconexión también recalcule. Sin test unitario (depende de la
  pantalla); lo cubre la prueba manual de la etapa 8.
- [ ] Commit `feat: keep the list of saved areas and the chosen one`.

---

## Chunk 2 · Capa de marcado (tanda 1)

`view.rs` va bajo `#[cfg(feature = "layer-shell")]`. `geometry.rs` y `save_dialog.rs` compilan
siempre, con `#[cfg_attr(not(feature = "layer-shell"), allow(dead_code))]`. `mark()` existe en
los dos modos; sin el feature devuelve el error de capas no disponibles. Clippy con
`-D warnings` pasa en los dos modos.

### Tarea 2.1: estado del marcado (puro)

**Archivos:** `src/area_marker/geometry.rs`.

Trabaja en **píxeles físicos locales del monitor**; la vista convierte con
`MonitorInfo::to_physical`/`to_logical`.

```rust
pub const MIN_WIDTH: i32 = 160;
pub const MIN_HEIGHT: i32 = 120;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect { pub x: i32, pub y: i32, pub width: i32, pub height: i32 }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Handle { N, S, E, W, NE, NW, SE, SW, Inside }

pub struct MarkState {
    bounds: (i32, i32),          // physical size of the monitor
    rect: Option<Rect>,
    drag: Option<Drag>,          // what the current gesture does
}

impl MarkState {
    pub fn new(bounds: (i32, i32), initial: Option<Rect>) -> Self;
    pub fn rect(&self) -> Option<Rect>;              // always even-sized, >= MIN, inside bounds
    pub fn hit(&self, x: i32, y: i32, slop: i32) -> Option<Handle>; // handles first, then Inside
    pub fn press(&mut self, x: i32, y: i32, slop: i32);   // on a handle: resize/move; elsewhere: new rect
    pub fn motion(&mut self, x: i32, y: i32);
    pub fn release(&mut self);
    pub fn nudge(&mut self, dx: i32, dy: i32);           // arrows: 1, shift: 10
    pub fn set_size(&mut self, width: i32, height: i32) -> Result<(), DoesNotFit>;
}

pub fn parse_size(text: &str) -> Option<(i32, i32)>; // "1280x720", "1280×720", "1280 x 720"
```

Reglas: el rectángulo nuevo va del punto de press al de motion, normalizado (se puede arrastrar
hacia arriba o a la izquierda), recortado a `bounds`; ancho y alto se redondean **hacia abajo**
a par; si queda menor que el mínimo durante el arrastre, `rect()` igual lo devuelve (para
dibujarlo) pero `release` lo agranda al mínimo desde la esquina de origen, corrido hacia adentro
si no entra. Mover y `nudge` no cambian el tamaño y frenan en los bordes. `set_size` mantiene
la esquina de arriba a la izquierda y se corre hacia adentro; si el tamaño es mayor que
`bounds` o menor que el mínimo, `Err(DoesNotFit)` sin cambios; redondea a par hacia abajo.

- [ ] **Tests que fallan** (en `geometry.rs`), uno por regla: arrastre normalizado hacia arriba a
  la izquierda; recorte a los bordes; tamaño par; mínimo aplicado al soltar; mover con `Inside`;
  resize con `SE` y con `N`; `nudge` frena en el borde; `set_size(1280, 720)` en 1680×1050 desde
  `x=1000` se corre a `x=400`; `set_size(4000, 100)` da `DoesNotFit`; `parse_size` acepta los
  tres formatos y rechaza `"12x"`, `"0x10"`, `"abc"`; `hit` prioriza manija sobre `Inside`.
- [ ] Implementar. Commit `feat: model the area marking gestures`.

### Tarea 2.2: capa de marcado por monitor

**Archivos:** `src/area_marker/view.rs`.

Una `MarkerLayer` por monitor: `gtk::Window` sin decoración, `overlay::init_overlay(&window,
monitor, "kooha-area-marker")`, `overlay::anchor_all_edges`, **`KeyboardMode::Exclusive`** (para
Enter, Escape y flechas; agregar `overlay::set_keyboard_exclusive(&window)` en `src/overlay.rs`
no está permitido en este chunk: llamar `gtk4_layer_shell::LayerShell::set_keyboard_mode`
directo en `view.rs`), clase CSS con fondo transparente (`window.add_css_class` + un
`gtk::CssProvider` cargado en `view.rs` con `window.kooha-area-marker { background: none; }`).

Hijo: un `gtk::DrawingArea` que dibuja con cairo:
- velo `rgba(0,0,0,0.30)` sobre todo el monitor, menos el rectángulo (`fill_rule EvenOdd`);
- contorno blanco de 1 px lógico; ocho manijas blancas de 8 px lógicos con borde `#3584e4`;
- etiqueta `"1280 × 720"` (físicos) en un globo negro al 80 % abajo a la derecha del
  rectángulo (arriba si no entra);
- en la capa del monitor con el puntero, el texto de ayuda de la spec arriba al centro.

Eventos: `GestureDrag` (botón primario) → `press/motion/release` convirtiendo coordenadas
lógicas locales a físicas; `GestureClick` del botón secundario → cancelar; cursor según
`hit()` (`nwse-resize`, `ns-resize`, `move`, `crosshair`). `EventControllerKey`: Escape cancela,
Enter confirma (si hay rectángulo), flechas `nudge(±1)`, con Shift `±10`.

Campo Tamaño: un `gtk::Entry` chico, hijo de un `gtk::Fixed` sobre el `DrawingArea` (en un
`gtk::Overlay`), ubicado al lado de la etiqueta, visible cuando hay rectángulo y no hay arrastre
en curso. Enter en el campo llama `parse_size` + `set_size`; con error agrega la clase CSS
`error` y no cambia nada; un segundo Enter (campo sin cambios) confirma.

**Teclado con varias capas (fable r1 #7):** todas las capas son exclusivas y el compositor le da
el teclado a una sola. Una sola capa está en `KeyboardMode::Exclusive` por vez (fable r2 #6): al hacer press en una
capa, o al entrar el puntero en una capa cuando no hay rectángulo, esa capa pasa a `Exclusive` y
las demás a `KeyboardMode::None`. Al arrancar, la del rectángulo inicial, o la del puntero, o la
primera. Además cada capa reenvía sus teclas a `AreaMarker`, que actúa sobre la capa activa.

- [ ] Test `#[gtk::test]` sin `present()`, con el guard `if overlay::availability().is_err() {
  return; }` como `src/area_border/view.rs:226`: construir la capa para el primer monitor del
  display y verificar que `view.state()` arranca con el rectángulo inicial pasado. (La
  interacción real se prueba en la etapa 8.)
- [ ] Commit `feat: draw the area marker layer on each monitor`.

### Tarea 2.3: un monitor a la vez

**Archivos:** `src/area_marker/mod.rs`.

`AreaMarker` crea una `MarkerLayer` por monitor de `areas::monitors::current()`. Un `press` en
una capa borra el rectángulo de las demás (`clear()`): el área vive en un solo monitor.

```rust
pub async fn mark(initial: Option<&MarkedArea>) -> anyhow::Result<MarkedArea>;
```

- `initial` se precarga si su connector está conectado y `fits()` el tamaño físico actual,
  aunque el área esté desactualizada por escala. Sin el monitor conectado, se marca sin precarga.
- Un solo marcado a la vez: `mark()` devuelve `AlreadyMarking` si hay uno en curso.
- Corte de seguridad: con `KOOHA_MARKER_TIMEOUT_SECS` definida, el marcado se cancela solo
  pasado ese tiempo. Es obligatoria en las primeras pruebas de la etapa 8.
- Las capas se **destruyen** (no se ocultan) antes de devolver el resultado y en el `Drop` de
  `AreaMarker`; el diálogo «Guardar como…» se abre recién con las capas destruidas.
- Enter con rectángulo → cierra todas las capas y devuelve `MarkedArea` con el connector,
  el rect físico, `physical_size()` y `scale` del monitor.
- Escape o botón derecho → cierra todas las capas y devuelve `Err(Cancelled)` (el tipo de
  `src/cancelled.rs`).
- Si un monitor se desconecta durante el marcado (`monitor.connect_invalidate`), su capa se
  cierra; si era la del rectángulo, el rectángulo se pierde.
- Sin capas disponibles (`overlay::availability()` con error), devuelve un error con ese motivo.
- [ ] Test puro de la conversión final `fn finish(rect, monitor: &MonitorInfo) -> MarkedArea`.
- [ ] Commit `feat: mark an area across monitors and return it`.

### Tarea 2.4: (sin código) verificación del enlazado

- [ ] `cargo test --features layer-shell` y `cargo clippy --features layer-shell -- -D warnings`
  pasan. Sin `--features layer-shell`, `area_marker::mark` existe y devuelve el error de capas no
  disponibles (para que el chunk 5 compile en los dos modos).

### Tarea 2.5: diálogo «Guardar como…»

**Archivos:** `src/area_marker/save_dialog.rs`.

`adw::AlertDialog` con heading `MarkedArea::describe()`, body «Top left at {x}, {y}», un
`adw::EntryRow` «Save as…» como `extra_child`, respuestas `remark` («Mark Again») y `done`
(«Done», sugerida, por defecto).

```rust
pub enum SaveChoice { Last, Named(String), MarkAgain }
pub async fn ask(parent: &impl IsA<gtk::Widget>, area: &MarkedArea,
                 name_exists: impl Fn(&str) -> bool) -> SaveChoice;
```

Recibe un closure en vez del `AreaStore` (fable r1 #5): el chunk 2 corre en paralelo con el
chunk 1. Con nombre ya existente, un segundo `AlertDialog`: «There is already an area
called “{name}” ({describe})», respuestas «Change Name» (vuelve al primero con el nombre) y
«Replace» (destructiva). Devuelve `Named(name)` sólo si no existe o se eligió Replace; quien
llama guarda con `replace = true` en ese caso.

- [ ] Test puro de la decisión `fn choice_for(name: &str, exists: bool, replace_answer: Option<bool>) -> ...`
  (nombre vacío o sólo espacios → `Last`; recorta espacios).
- [ ] Commit `feat: offer to save a marked area with a name`.

---

## Chunk 3 · Medición y pipeline (tanda 1)

### Tarea 3.1: script de medición

**Archivos:** `tools/bench-capture.py` (ejecutable, `#!/usr/bin/python3`: el `python3` del PATH
es el de linuxbrew, sin `gi`; codex r1 #22), con docstring de uso. Al arrancar comprueba que
`gi` y Gst 1.0 estén y, si no, sale con un mensaje claro.

Dos fuentes:
- `--source test` (por defecto): `videotestsrc pattern=ball is-live=true` a 2560×1600, formato
  `BGRx`, a los FPS pedidos. No toca el portal ni la sesión gráfica.
- `--source portal`: abre `org.freedesktop.portal.ScreenCast` por `Gio.DBusProxy` en el bus de
  sesión, con `types=1` (monitor), `persist_mode=2` y `restore_token` leído con
  `gsettings get io.github.seadve.Kooha screencast-restore-token`. Usa `OpenPipeWireRemote` y
  `pipewiresrc fd=… path=<node>`. **Sólo lo corre el orquestador** (tarea 3.4), nunca un
  subagente. Manejo del token (fable r1 #3):
  - antes de abrir la sesión, guarda el token original en `/tmp/kooha-bench-token.bak`;
  - escribe el token nuevo en la clave en el manejador de la respuesta de `Start`, antes de
    construir cualquier pipeline, y sólo si la sesión arrancó sin diálogo (respuesta en menos de
    2 s, o `--allow-dialog` explícito) y el stream es de tipo monitor; con `--with-kde-region`,
    nunca;
  - si no pudo escribirlo, avisa por stdout que la próxima grabación de Kooha va a preguntar;
  - `--probe-session` abre la sesión, llama a `Start` y la cierra, para verificar si KDE acepta
    el token desde un proceso que no es Kooha; `Start` consume el token, así que la regla de
    escritura del sucesor vale igual para la sonda. Si apareció el diálogo, la clave queda como
    estaba; con `--allow-dialog` tampoco se escribe, salvo con `--write-token`. El backup se
    restituye a mano, y sólo si KDE pregunta siempre por ver otro identificador de app (fable
    r2 #3).
  - En modo portal, después de recibir el stream, abre una ventana con `videotestsrc
    pattern=ball` en pantalla completa sobre el monitor cuyo tamaño físico coincide con el del
    stream (`fullscreen_on_monitor`); descarta la pasada si la fuente entregó menos del 90 % de
    los cuadros pedidos en la variante `today` (fable r2 #4).

Variantes (`--variant`, repetible; `all` por defecto), con un área de 1280×720 en (200, 160):
`today` (videorate → videoscale → videocrop → x264 como `profiles.yml`), `crop-first`
(videorate → videocrop → x264), `crop-first-queues` (exactamente las colas de la tarea 3.3:
`queue max-size-buffers=3 max-size-time=0 max-size-bytes=0 leaky=downstream` entre la fuente y
`videorate`, y `queue max-size-buffers=30 max-size-time=0 max-size-bytes=0` antes del
codificador), `intel` (crop-first-queues +
`vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! vah264lpenc ! h264parse`),
`nvidia` (crop-first-queues + `cudaupload ! cudaconvert ! video/x-raw(memory:CUDAMemory),format=NV12 ! nvh264enc ! h264parse`),
y en modo portal además `intel-dmabuf` y `nvidia-dmabuf` (pipewiresrc negociando `video/x-raw(memory:DMABuf)` →
`vapostproc` con `video-direction`/crop por `GstVideoCropMeta` si el elemento lo acepta; si la
negociación falla, la variante se reporta como «no negocia» con el error).

Todas terminan en `mp4mux ! filesink` en `/tmp/kooha-bench-<variante>.mp4`. `--seconds 20` y
`--fps 60` por defecto (`--fps 30` para la segunda pasada).

Medidas por variante: CPU del proceso (`/proc/self/stat` utime+stime antes y después, en % de
un núcleo); **cuadros entregados por la fuente** (pad probe en el `src` de `pipewiresrc` o
`videotestsrc`, antes de `videorate`, que rellena duplicando); cuadros a la salida del
codificador; cuadros perdidos entre la fuente y el archivo (descartes de la cola con fuga más
huecos de PTS mayores a 1,5 períodos); y si arrancó (error del bus). `--with-kde-region` corre la
comparación con la región de KDE (pide el diálogo; nunca guarda su token).
Salida: una tabla Markdown por stdout y `--out <archivo>`.

- [ ] Test: `/usr/bin/python3 -I tools/bench-capture.py --source test --variant today --seconds 2 --fps 30`
  termina con exit 0 e imprime una fila con `fps` cercano a 30 (es la prueba de que el script
  funciona; no hay suite de Python).
- [ ] Commit `tools: add a capture benchmark for pipeline variants`.

### Tarea 3.2: recortar antes de escalar

**Archivos:** `src/pipeline.rs`.

Hoy (`src/pipeline.rs:116-128`): `videosrc_bin → videoscale → caps pares → videocrop`. Pasa a
lo siguiente.

- **Área marcada** (nuevo, lo usa el chunk 5): `PipelineBuilder::area_crop(AreaCrop)` con

```rust
/// What to cut from the whole-monitor stream, in stream (physical) pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AreaCrop {
    pub x: i32, pub y: i32, pub width: i32, pub height: i32,
    /// The physical size of the monitor the area was marked on.
    pub expected_frame: (i32, i32),
}
```

  Grafo: `videosrc_bin → videocrop(left=x, top=y) → …`, sin `videoscale`. `right` y `bottom`
  se calculan **al llegar las caps** con una sonda en el pad `sink` de `videocrop`
  (`gst::PadProbeType::EVENT_DOWNSTREAM`, evento `Caps`): `right = frame_w - x - width`,
  `bottom = frame_h - y - height`, recortados a `>= 0`. Así el recorte vale aunque el cuadro
  real difiera en un par de píxeles del tamaño guardado. Si el cuadro real difiere de
  `expected_frame` en más de 2 en algún eje, la sonda publica un error en el bus
  (`gst::element_error!`) en vez de recortar: sería un recorte corrido (fable r1 #2). Dentro de
  la tolerancia, si `x + width > frame_w` (o en alto), el área se **corre hacia adentro**
  manteniendo su tamaño (el tamaño prometido no se achica; codex r1 #9); si `width > frame_w` o
  `height > frame_h`, error en el bus.
- **Selector de upstream** (sin capas): `videosrc_bin → videocrop → videoscale → caps pares →`
  … con `compute_crop` sin cambios en sus cálculos; las caps pares pasan a calcularse sobre el
  tamaño recortado (`crop.frame` menos los cuatro recortes, redondeado a par hacia abajo).

- [ ] **Tests que fallan** (en `pipeline.rs`, `mod tests`, con `gst::init()`):
  - `area_crop_needs_no_videoscale`: construir un pipeline con `area_crop` y verificar que no
    hay ningún elemento `videoscale` (`pipeline.iterate_recurse()`), y que `videocrop` va antes
    de la compuerta.
  - `area_crop_sets_right_and_bottom_from_caps`: pipeline de prueba `videotestsrc num-buffers=1 !
    video/x-raw,width=1680,height=1050 ! <videocrop con la sonda> ! fakesink` con
    `AreaCrop { x: 100, y: 80, width: 1280, height: 720 }`; tras `PLAYING` y EOS, las
    propiedades `right` = 300 y `bottom` = 250, y las caps de salida 1280×720.
  - `area_crop_shifts_inside_within_tolerance`: cuadro real 2 y 1 píxeles más chico que
    `expected_frame`, con el área pegada al borde: la salida mantiene 1280×720, corrida.
  - `area_crop_rejects_another_frame_size`: con `expected_frame` 1680×1050 y un cuadro de
    1920×1080, el bus recibe un error y no hay salida.
  - `selector_path_crops_before_scaling`: con `select_area_data`, el orden de elementos es
    `videocrop` → `videoscale`.
  - Los tests existentes de `compute_crop` siguen pasando sin cambios.
- [ ] Implementar `fn make_area_crop(crop: AreaCrop) -> Result<gst::Element>` con la sonda, y el
  nuevo orden en `build()`.
- [ ] Commit `perf: crop before scaling and skip scaling for marked areas`.

### Tarea 3.3: colas

**Archivos:** `src/pipeline.rs`.

Una `queue` `kooha-capture-queue` **dentro de `make_videosrc_bin`, entre `videoflip` (o el
`compositor`) y `videorate`**, con `max-size-buffers=3, max-size-time=0, max-size-bytes=0,
leaky=downstream`: descarta los cuadros viejos en vez de frenar a `pipewiresrc`, y `videorate`
rellena lo descartado, así el archivo no queda con huecos (fable r1 #12). Los descartes se
cuentan (señal `overrun` o sonda) y el total va al log con `tracing::info!` al terminar. La
`kooha-videoenc-queue` existente pasa a `max-size-buffers=30, max-size-time=0,
max-size-bytes=0`. Si la medición de la tarea 3.4 muestra otros valores mejores, se ajustan
ahí.

- [ ] Test que falla: las propiedades de las dos colas en un pipeline construido.
- [ ] Commit `perf: keep a burst in the encoder from stalling the capture`.

### Tarea 3.4: correr la medición (orquestador)

- [ ] `tools/bench-capture.py --source test --fps 60` y `--fps 30`.
- [ ] Antes de usar el permiso, comprobar que Kooha no está grabando (sólo un proceso usa el
  token por vez).
- [ ] `--probe-session` primero: si KDE muestra el diálogo, no seguir; avisar por el Hilo y
  Sebastián lo corre (la spec lo prevé).
- [ ] `--source portal --fps 60` y `--fps 30` en la sesión de Sebastián, con el permiso guardado
  (P11).
- [ ] Comprobación final: una grabación corta con el Kooha instalado arranca sin diálogo. Si
  pregunta, avisar a Sebastián por el Hilo que la próxima vez va a tener que elegir el monitor.
- [ ] Escribir `docs/perf/2026-11-18-capture-bench.md`: las tablas, la máquina (CPU, placas,
  driver), la conclusión con el **orden de Automático** para el chunk 7 y si DMA-BUF entra.
- [ ] Commit `docs: record the capture benchmark`.

---

## Chunk 4 · Cámara: mantener los cuadros (tanda 1)

### Tarea 4.1: control V4L2 por ioctl

**Archivos:** `src/camera_pip/v4l2.rs` (nuevo, declarado en `src/camera_pip/mod.rs`).

Agregar `libc = "0.2"` a `Cargo.toml` (ya está en `Cargo.lock` como dependencia transitiva,
así que no baja nada nuevo). Es el único cambio de `Cargo.toml` de la tanda 1.

```rust
pub const EXPOSURE_DYNAMIC_FRAMERATE: u32 = 0x009a_0903; // V4L2_CID_EXPOSURE_AUTO_PRIORITY
pub fn get_control(path: &Path, id: u32) -> io::Result<i32>;  // VIDIOC_G_CTRL
pub fn set_control(path: &Path, id: u32, value: i32) -> io::Result<()>; // VIDIOC_S_CTRL
```

`VIDIOC_G_CTRL = _IOWR('V', 27, struct v4l2_control)` = `0xc008_561b`,
`VIDIOC_S_CTRL = _IOWR('V', 28, …)` = `0xc008_561c`; `struct v4l2_control { u32 id; i32 value; }`.
Abrir el nodo con `O_RDWR | O_NONBLOCK`.

- [ ] Test que falla: `get_control` sobre `/dev/null` devuelve `Err` (ioctl no soportado), sin
  pánico; `ioctl_numbers_match_linux_headers` afirma las dos constantes. (Los tests no tocan
  `/dev/video*`: la cámara de Sebastián prendería su LED.)
- [ ] Commit `feat: read and write V4L2 camera controls`.

### Tarea 4.2: aplicar y restituir al prender y apagar el PiP

**Archivos:** `src/camera_pip/mod.rs`, `src/camera_pip/devices.rs` (sólo para hacer
`device_path()` `pub(super)`).

En `start_pipeline` (`src/camera_pip/mod.rs:269-309`), con el `gst::Device` a mano: si
`settings.camera_keep_framerate()` y `devices::device_path(&device)` da un `/dev/videoN`, leer el
control; si existe y vale distinto de 0, guardar `(path, valor)` en `imp.restore_control` y
escribir 0. `pub fn restore_camera_controls(&self)` restituye el valor guardado si hay; se llama
al detener el pipeline (el lugar donde hoy se pone en `Null`), en `dispose` y en
`Application::shutdown` (lo agrega el orquestador en la tarea 0.4, porque `dispose` no corre si
un closure retiene al `CameraPip`). Errores: `tracing::warn!`, nunca un diálogo.

La lógica de decisión va en una función pura:

```rust
/// What to do with the control when the camera starts: write 0 and remember `current`.
fn keep_framerate_plan(enabled: bool, current: Option<i32>) -> Option<i32>; // Some(restore_to)
```

- [ ] Tests que fallan de `keep_framerate_plan`: apagada → `None`; sin control → `None`; valor 0
  → `None` (nada que hacer); valor 1 → `Some(1)`.
- [ ] Commit `feat: keep the camera frame rate in low light`.

### Tarea 4.3: preferencia

**Archivos:** `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui`.

`adw::SwitchRow` «Keep Camera Frame Rate» en el grupo de la cámara (`camera_group`). Subtítulo
según la fuente elegida: si tiene el control, «In low light the picture gets darker instead of
choppy»; si `get_control` dice que no lo tiene, «This camera can't change it». Se calcula al abrir
el diálogo y al cambiar la cámara, sólo con el ioctl de lectura (no abre el stream ni prende el
LED); la función que elige el texto es pura y se prueba con valores inyectados. Ligado con
`settings.bind_camera_keep_framerate(&row, "active")` (o `bind("camera-keep-framerate", …)`
como las filas existentes).

- [ ] Test `#[gtk::test]` con el patrón de los 9 tests existentes de `src/preferences_dialog.rs`:
  la fila refleja el valor de la clave `camera-keep-framerate`. Test de la función del subtítulo.
- [ ] Commit (se puede unir al anterior si es chico).

---

## Chunk 5 · Ventana, borde y grabación (tanda 2)

### Tarea 5.1: `ToastOverlay` y estado del área en la ventana

**Archivos:** `data/resources/ui/window.ui`, `src/window/mod.rs`.

- [ ] Envolver el contenido de la ventana en un `AdwToastOverlay` (`toast_overlay` como
  `TemplateChild`) y agregar `Window::add_toast(&self, toast: adw::Toast)`.
- [ ] La ventana lee la lista con `Application::get().area_store()`, que agrega la tarea 0.4
  antes de que arranque la tanda 2.
- [ ] Si `store.load_notice()` devuelve un path al arrancar, toast «The list of areas was
  unreadable; it was moved to {path}». La señal `save-failed` del store también va como toast
  («Couldn't save the list of areas: {error}»).
- [ ] Tests: con un store sobre un temporal con un archivo roto, la ventana encola el toast
  (método `pending_toasts()` sólo para tests, o se verifica el texto que devuelve una función
  pura `notice_for_store(&store) -> Option<String>`).

### Tarea 5.2: fila de área

**Archivos:** `data/resources/ui/window.ui`, `data/resources/ui/shortcuts.ui`, `src/window/mod.rs`.

Debajo del selector de modo, visible sólo en modo Selección **y** con capas disponibles:
- `gtk::DropDown` con un `gtk::StringList` armado desde el store: cada guardada como
  `"{name} · {describe}"` (con «⚠ » adelante si está desactualizada), y al final
  `"Last area · {describe}"` si hay último. Se rearma con `store.connect_changed` y con
  `areas::monitors::watch` (desactualizadas cambian, también por escala).
- botón ojo (`view-reveal-symbolic` / `view-conceal-symbolic`) ligado a la acción
  `win.show-area-border` (de `settings.create_show_area_border_action()`), atajo `Ctrl+B` con un
  `gtk::ShortcutController` de la ventana (no en `application.rs`);
- botón «Mark Area» → acción `win.mark-area`;
- botón «Areas…» → acción `app.show-areas` (la define el chunk 6).

Elegir en el desplegable llama `store.select(...)`. Una desactualizada se puede elegir (para
ver por qué no graba); el borde no se muestra.

**Un único camino para marcar (fable r1 #6, #18):** `Window::mark_area(&self, target:
MarkTarget) -> impl Future<Output = Option<MarkedArea>>` (async; `None` si se canceló o ya había
un marcado en curso), con `enum MarkTarget { New, Replace(String) }`. Hacia afuera de la ventana
el contrato son **dos acciones con nombre** (fable r2 #1), que no generan dependencia de
compilación: `win.mark-area` (sin parámetro, `MarkTarget::New`) y `win.remark-area` (parámetro
`s`, el nombre, `MarkTarget::Replace`). El chunk 6 las activa con
`app.window().activate_action("win.mark-area", None)` como hoy hace la bandeja con
`win.toggle-record` (`src/application.rs:256-262`).
- `New`: `area_marker::mark(store.selected_area().as_ref().map(|c| &c.area))` → `save_dialog::ask(.., |n|
  store.contains(n))` → `store.set_last` / `store.save_named(.., replace)`; `MarkAgain` vuelve a
  marcar con el área recién marcada como inicial.
- `Replace(name)`: `area_marker::mark(Some(&area_de_ese_nombre))` → `store.replace_geometry(name,
  ..)`, sin diálogo de nombre.
- `Cancelled` no hace nada. Mientras dura un marcado, `win.mark-area` y `win.toggle-record` se
  deshabilitan (fable r1 #7).
La ventana Rectángulos y la bandeja activan esas dos acciones; el toast de área desactualizada
llama `mark_area` directo (está en la ventana). Nadie llama `area_marker::mark` fuera de
`mark_area`.

`Ctrl+B` se suma también a `data/resources/ui/shortcuts.ui`.

- [ ] Test puro `fn dropdown_items(areas, last, monitors) -> Vec<(String, Selected, bool /*stale*/)>`.
- [ ] Test de que `mark_area` deshabilita y vuelve a habilitar las dos acciones (con el
  marcado reemplazado por un futuro inyectable que devuelve `Cancelled`).
- [ ] Commit `feat: choose and mark areas from the main window`.

### Tarea 5.3: borde en reposo

**Archivos:** `src/window/mod.rs` (`update_area_border`, líneas 671-701),
`src/area_border/mod.rs`.

- [ ] `area_border::for_area(area: &MarkedArea, monitors) -> Option<(BorderGeometry, gdk::Monitor)>`:
  arma un `Crop` con `frame = (monitor_width, monitor_height)` y los cuatro recortes desde el
  área, busca el monitor por connector y llama `border_geometry`. Test puro de la parte que no
  necesita `gdk::Monitor` (`fn crop_for_area(area) -> Crop`).
- [ ] `update_area_border` pasa a decidir con una función pura testeada:

```rust
enum BorderSource { None, Recording, Idle }
fn border_source(state: RecordingState, selection_mode: bool, show_setting: bool,
                 has_fresh_area: bool, overlays: bool) -> BorderSource;
```

  `show_setting` falso → `None` siempre. Grabando/pausa/cuenta regresiva → `Recording` (lo de
  hoy, ahora sin depender del selector). En reposo (`Init`/`Finished`/sin grabación) → `Idle`
  si modo Selección, área elegida fresca y capas. Tests de la tabla completa.
- [ ] Recalcular al cambiar el estado de la grabación (como hoy), el modo, la clave
  `show-area-border`, el store y los monitores (`areas::monitors::watch`). Cambiar de `Idle` a `Recording` con la misma
  geometría no recrea la capa (comparar `BorderGeometry` y connector).
- [ ] Commit `feat: show the chosen area's border before recording`.

### Tarea 5.4: grabar con el área elegida

**Archivos:** `src/recording.rs`, `src/area_border/mod.rs` (`stream_monitor` → `pub(crate)`).

`CaptureConfig::from_settings(settings, area: Option<ChosenArea>)` recibe el área de quien llama
(fable r2 #5): `start_inner` pasa, en modo Selección con capas disponibles,
`Application::get().area_store().selected_area()`; los tests existentes sin aplicación
(`config_is_a_snapshot`) pasan `None`.

Cuándo se marca antes de grabar, con una función pura (fable r1 #15):

```rust
fn needs_marking(mode: CaptureMode, overlays_available: bool, has_area: bool) -> bool {
    mode == CaptureMode::Selection && overlays_available && !has_area
}
```

Si da `true`, `Window::toggle_record` corre `mark_area(New)` y graba con lo marcado; si se
cancela, no graba. Sin capas, nunca se llama a `area_marker` y el selector de upstream sigue como
hoy. Tests: la tabla completa de `needs_marking`, y que `CaptureConfig` sin capas da `area: None`.

En `start_inner`, antes del portal: si `area.staleness(...)` es `Some`, error
`ContextWithHelp::new("The area “{name}” is out of date", staleness.describe())` — la ventana lo
muestra como toast con botón «Mark Again» en vez del diálogo de error
(`handle_recording_finished` reconoce el tipo `StaleArea`). El botón llama
`mark_area(Replace(name))` si el área tiene nombre, y `mark_area(New)` si es el último
rectángulo.

Con área: la sesión del portal pide `SourceType::MONITOR` (sin `WINDOW`, sin múltiple). Después
de `new_screencast_session`:

```rust
pub(crate) enum StreamMatch { Same, Other { shared: Option<(i32, i32)> }, Unknown }
pub(crate) fn match_stream(streams: &[Stream], area: &MarkedArea, monitors: &[MonitorInfo]) -> StreamMatch;
```

Compara el tamaño del stream directamente contra el monitor del área (fable r1 #2), sin pasar
por `stream_monitor`: `Same` si coincide con el físico guardado `(monitor_width,
monitor_height)` ±2, o con el lógico actual de ese monitor ±1; `Other { shared }` si trae tamaño
y no coincide; `Unknown` sólo si el stream no trae tamaño (se acepta con `tracing::warn!`, y la
sonda de `AreaCrop` es la segunda defensa: con un cuadro de otro tamaño, falla). Más de un
stream → `Other`. Dos monitores del mismo tamaño dan `Same` con aviso en el log (ADR 0005).

- `Same`/`Unknown`: `pipeline_builder.area_crop(AreaCrop{..})`, `imp.area_border` desde
  `area_border::for_area`. **No se llama a `AreaSelector::select`.**
- `Other` en el primer intento: `screencast_session.close()`, `settings.reset_screencast_restore_token()`,
  toast «Choose {connector} (whole screen) in the sharing dialog» (por una señal nueva de
  `Recording`, `notice(String)`, que la ventana muestra como toast), y un segundo
  `new_screencast_session` sin token.
- `Other` en el segundo: error «You shared {w}×{h}, but the area is on {connector} ({mw}×{mh})».

- [ ] Tests puros de `match_stream` con los tres monitores: stream 1680×1050 sin posición ↔
  área en DP-3 → `Same`; stream 1600×1000 (lógico) ↔ área en DP-3 → `Same`; stream 1463×915 ↔
  área en eDP-1 (GDK 1463×914) → `Same`; stream 1920×1080 ↔ área en DP-3 → `Other`; stream
  3520×2130 ↔ área en DP-3 → `Other`; dos streams → `Other`; sin tamaño → `Unknown`.
- [ ] El reintento de P9 va en una función async genérica en el tipo de sesión (fable r2 #2):
  `open: impl Fn(Option<String>) -> Future<Output = Result<(S, Vec<Stream>, Option<String>)>>`
  y `close: impl Fn(S) -> Future<Output = ()>`; los tests usan `S = u32` y arman los `Stream` con
  `Stream::from_variant(&(node_id, dict).to_variant())`. Ningún test crea un `gio::DBusProxy` ni
  toca el bus de sesión. Se prueban
  los cuatro caminos: `Same` al primer intento; `Other` y después `Same` (cierra la primera
  sesión, borra el token, emite `notice`); `Other` dos veces (error final, y el token de la
  segunda sesión **no** se guarda); `Unknown` (graba).
- [ ] Test de `CaptureConfig`: en modo Selección sin capas, `area` es `None` (sigue el selector).
- [ ] Commit `feat: record the chosen area without the upstream selector`.

### Tarea 5.5: verificación del chunk

- [ ] Suite con y sin `--features layer-shell`, clippy en los dos modos, fmt.

---

## Chunk 6 · Ventana Rectángulos y bandeja (tanda 2)

### Tarea 6.1: `area_store` y `app.show-areas`

**Archivos:** `src/application.rs`.

- [ ] Test: `app.show-areas` está registrada después de `startup`.
- [ ] Acción `app.show-areas` que abre
  (o presenta, si ya está abierta) la ventana Rectángulos con la ventana principal como padre.

### Tarea 6.2: ventana Rectángulos

**Archivos:** `src/areas_window.rs`, `data/resources/ui/areas_window.ui`,
`data/resources/resources.gresource.xml`, `src/main.rs` (`mod areas_window;`).

`adw::Window` (modal no) con `adw::HeaderBar` título «Areas», y un `gtk::ListBox` con clase
`boxed-list`: una `adw::ActionRow` por área guardada, título = nombre, subtítulo =
`describe()` y «· chosen» si es la elegida, o el motivo de `staleness` con clase `warning`.
Sufijos: «Choose» (deshabilitado si está elegida o desactualizada), renombrar (abre un
`adw::AlertDialog` con `adw::EntryRow` precargada; `NameTaken` → mensaje en el diálogo),
«Mark Again» (siempre habilitado, también con el monitor desconectado: P10; activa
`win.remark-area` con el nombre en la ventana principal, que precarga si el monitor está y si no
marca desde cero), borrar (confirmación
`adw::AlertDialog` destructiva). Lista vacía: `adw::StatusPage` «No Saved Areas» con el botón
«Mark Area», que activa `win.mark-area` sobre la ventana principal (`app.window()`; el grupo
`win` no existe en esta ventana: fable r1 #18). Pie: «Saved in {path}». Se rearma con `store.connect_changed` y
los cambios de monitores.

- [ ] Test puro `fn row_model(area: &SavedArea, selected: bool, monitors) -> RowModel` (texto,
  qué botones habilitados) con los casos: elegida, desactualizada por desconexión (Choose
  deshabilitado, Mark Again habilitado), desactualizada por escala (Mark Again habilitado).
- [ ] Commit `feat: add the Areas window to choose, rename, mark again and delete`.

### Tarea 6.3: bandeja

**Archivos:** `src/tray/model.rs`, `src/tray/service.rs`, `src/application.rs`.

- [ ] `TrayCommand` deja de ser `Copy` (pasa a `Clone`; `KoohaTray::send` clona o loguea antes
  de mover el comando al `TrayRequest`) y suma `SelectArea(Selected)`,
  `MarkArea` y `ToggleAreaBorder`. `TrayModel` suma `areas: Vec<(String /*label*/, Selected,
  bool /*enabled*/, bool /*chosen*/)>` y `show_area_border: bool`.
- [ ] `TrayEntry` suma `SubMenu { label, children: Vec<TrayEntry> }` y `Radio { label, command,
  enabled, selected }`. `menu_entries` agrega, después de los toggles de captura, el submenú
  «Area» (radios + separador + «Mark New Area…») y la casilla «Show Area Border». Sin capas
  disponibles no se agregan.
- [ ] `service.rs` mapea `SubMenu` a `ksni::menu::SubMenu` y los radios a `RadioGroup` (un grupo
  con todos los radios del submenú; `select` manda el `SelectArea` del índice). Las desactualizadas
  van con `enabled: false`.
- [ ] `handle_tray_command`: `SelectArea` → `store.select` + `settings.set_capture_mode(Selection)`;
  `MarkArea` → presentar la ventana y activar `win.mark-area` sobre ella; `ToggleAreaBorder` → invertir
  `show-area-border`. Refrescar la bandeja con `store.connect_changed`, la clave y `areas::monitors::watch` (fable r2
  #8); test de `model.rs` con un área que pasa a desactualizada.
- [ ] Tests: los de `model.rs` (entradas con y sin capas, radio elegido, desactualizada
  deshabilitada) y los de `service.rs` (el submenú se mapea con la cantidad de hijos esperada).
- [ ] Commit `feat: choose areas and toggle their border from the tray`.

### Tarea 6.4: verificación del chunk

- [ ] Suite con y sin `--features layer-shell`, clippy en los dos modos, fmt.

---

## Chunk 7 · Codificador (tanda 3)

### Tarea 7.1: cadenas y prueba previa

**Archivos:** `src/encoder.rs`, `src/main.rs` (`mod encoder;`).

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Encoder { Intel, Nvidia, Cpu }

impl H264Encoder {
    /// The `gst-launch` chain from raw video to `video/x-h264`, before `queue ! h264parse`.
    pub fn chain(self) -> &'static str;
    pub fn required_elements(self) -> &'static [&'static str];
    pub fn is_installed(self) -> bool;          // ElementFactory::find de cada uno
    /// Runs `videotestsrc num-buffers=2 ! video/x-raw,format=BGRx,width=W,height=H
    /// ! <chain, expanded> ! fakesink` with the recording's output size, in
    /// `gio::spawn_blocking`, with a 3 s timeout and the pipeline set to Null at the end;
    /// cached per (encoder, size).
    pub async fn works(self, size: (i32, i32)) -> Result<(), String>;
}

/// Encoders to try, in order, for a preference. `AUTO_ORDER` comes from
/// docs/perf/2026-11-18-capture-bench.md.
pub fn candidates(pref: VideoEncoder) -> Vec<H264Encoder>;
pub const AUTO_ORDER: [H264Encoder; 3] = [/* lo que diga la medición */];
/// Tries the candidates in order and stops at the first that works.
pub async fn resolve<F, Fut>(pref: VideoEncoder, works: F) -> Option<H264Encoder>
where F: Fn(H264Encoder) -> Fut, Fut: Future<Output = bool>;
```

Cadenas (las ajusta la medición si encontró otra mejor):
- `Intel`: `vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! vah264lpenc`
- `Nvidia`: `cudaupload ! cudaconvert ! video/x-raw(memory:CUDAMemory),format=NV12 ! nvh264enc`
- `Cpu`: `videoconvert chroma-mode=none dither=none matrix-mode=output-only n-threads=${N_THREADS} ! x264enc qp-max=17 speed-preset=ultrafast threads=${N_THREADS} ! capsfilter caps=video/x-h264,profile=baseline`

`resolve`: forzado → ese, si `works`, si no `None`; `Auto` → el primero de `AUTO_ORDER` que
`works` con el tamaño de salida de la grabación, con `Cpu` siempre último (codex r1 #17). Un
timeout no se guarda en el caché (fable r2 #7). El tamaño de salida se conoce en `start_inner`
después de recibir el stream y fijar el recorte, antes de `build()`.

**Una sola expansión de marcadores (codex r1 #4):** `profile::expand(description, encoder)`
reemplaza `${H264_ENCODER}` y después `${N_THREADS}`; la usan `parse_bin_inner` y `works()`. La
cadena `Cpu` sin expandir no parsea (`n-threads="${N_THREADS}"`), así que `works()` nunca recibe
la cadena cruda. Test: `Cpu.works((1280, 720))` pasa.

- [ ] Tests puros de `candidates` y `resolve` con un `works` falso asíncrono (todos fallan salvo CPU →
  CPU; forzado Intel que falla → `None`). Test real de `Cpu.works()` (x264 está instalado en la
  máquina de desarrollo; si no está, el test se saltea con un mensaje).
- [ ] Commit `feat: describe the H.264 encoders and test them before use`.

### Tarea 7.2: marcador en los perfiles

**Archivos:** `data/resources/profiles.yml`, `src/profile.rs`.

- [ ] En `mp4` y `matroska-h264`, `videoenc` pasa a `${H264_ENCODER} ! queue ! h264parse`.
- [ ] `parse_bin_inner` reemplaza `${H264_ENCODER}` por la cadena del codificador que recibe
  (nuevo parámetro) **antes** de `${N_THREADS}`. `Profile::is_available()` y
  `required_elements()` usan `H264Encoder::Cpu` (el perfil está disponible si al menos la CPU
  anda; el chequeo por hardware es aparte).
- [ ] `Profile::attach(…, encoder: H264Encoder)`; los llamadores son
  `PipelineBuilder::build` (`src/pipeline.rs`), que pasa `self.encoder` con
  `PipelineBuilder::encoder(H264Encoder)` (default `Cpu`), y el test `profiles_validity`
  (`src/profile.rs:355`), que pasa `Cpu`.
- [ ] `CaptureConfig` (`src/recording.rs`) suma `encoder: VideoEncoder`, leído una vez como el
  resto. **Sólo para los perfiles `mp4` y `matroska-h264`** (codex r1 #12): con cualquier otro
  perfil no se resuelve ni se prueba nada, y un H.264 forzado que no anda no impide grabar WebM o
  GIF (test). `start_inner` resuelve con `encoder::resolve` (la prueba previa corre con
  `gio::spawn_blocking`, fuera del hilo principal, con el tamaño de salida) y lo pasa con `pipeline_builder.encoder(..)`.
  Si da `None` (forzado y no anda), la grabación falla con `ContextWithHelp` «The {encoder}
  encoder is not working», el motivo, y la ayuda «Choose Automatic or CPU in Preferences». Si el
  codificador por hardware pasó la prueba pero falla con el tamaño real (error del bus al
  arrancar), el error lleva la misma ayuda.
- [ ] Tests: el `mp4` construido con cada codificador parsea (con `Cpu` siempre; con Intel y
  NVIDIA sólo si `is_installed`); el texto sin `${H264_ENCODER}` queda igual que antes para
  `webm-vp8` y `gif`.
- [ ] Commit `feat: pick the H.264 encoder per recording`.

**Nota de conflicto:** este chunk toca `src/pipeline.rs` y `src/recording.rs`; va en la tanda
3, después de integrar los chunks 3 y 5.

### Tarea 7.3: preferencia

**Archivos:** `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui`.

- [ ] `adw::ComboRow` «Encoder» después de la fila de formato, visible sólo si el perfil
  elegido es `mp4` o `matroska-h264`. Modelo: Automatic, Intel, NVIDIA, CPU; las no instaladas
  aparecen con «(`vah264lpenc` is not installed)» y no se pueden elegir (en `bind`,
  `list_item.set_selectable(false)` y `set_activatable(false)`). Si la clave guarda un
  codificador que dejó de estar instalado, la fila lo muestra igual con el motivo y la clave no
  se toca; la grabación da el error del ADR 0006.
- [ ] Test: la fila es visible sólo con `mp4` y `matroska-h264`.
  Subtítulo con el codificador que usaría Automático hoy («Automatic uses Intel»), calculado con
  `resolve` para 1280×720; la prueba corre en `gio::spawn_blocking` (dentro de `works`), así el
  diálogo no se frena.
- [ ] Commit `feat: choose the video encoder in Preferences`.

### Tarea 7.4: reporte de requisitos

**Archivos:** `src/doctor/checks.rs`.

- [ ] La línea 106 es la tabla `PACKAGES`: sumar `vah264lpenc`, `nvh264enc`, `cudaupload` y
  `cudaconvert` **sin sacar** `vah264enc` (lo usa el perfil experimental `va-h264`, y el test
  `every_element_has_a_package` lo exige). Confirmar el paquete de cada uno con `dpkg -S` sobre
  el `.so` de los plugins `va` y `nvcodec`.
- [ ] Chequeo nuevo «Hardware video encoders», con su test que falla primero: informa cuáles
  de Intel y NVIDIA pasan la prueba previa. Las fallas son advertencias (Kooha sigue grabando
  con CPU), con el paquete a instalar en el comando.
- [ ] Commit `feat: report the hardware encoders in the requirements check`.

### Tarea 7.5: DMA-BUF (condicional)

Sólo si el informe de la tarea 3.4 dice que alguna variante `*-dmabuf` negocia y mejora la CPU o
los cuadros perdidos. Si no, el informe lo documenta y esta tarea no se hace; el Resumen de
cierre lo dice (P12 lo había elegido).
- [ ] Test que falla: con el codificador correspondiente, la fuente del pipeline ofrece
  `video/x-raw(memory:DMABuf)` y el recorte se hace por `GstVideoCropMeta` (`videocrop` la agrega
  cuando el elemento siguiente la acepta) en vez de copiar el cuadro.
- [ ] Implementar la cadena que ganó la medición, con respaldo a memoria común si no negocia.

---

## Tarea 0.4 (orquestador, entre la tanda 1 y la tanda 2): `area_store` en la aplicación

**Archivos:** `src/application.rs`. Va después de integrar el chunk 1, porque usa
`AreaStore::load_default()`, y antes de lanzar los chunks 5 y 6, que lo usan.

- [ ] `imp::Application` suma `area_store: OnceCell<AreaStore>` con un getter perezoso
  `pub fn area_store(&self) -> AreaStore` que lo crea con `AreaStore::load_default()` la primera
  vez, así los tests que construyen `Window` sin `startup` no entran en pánico. Bajo
  `cfg(test)`, `load_default()` usa el temporal de `areas::file::path()` (fable r1 #1).
- [ ] `shutdown` llama `if let Some(pip) = self.camera_pip() { pip.restore_camera_controls() }`
  (fable r1 #19; `camera_pip()` devuelve `Option<&CameraPip>`).
- [ ] `.gitignore` suma `_install/`.
- [ ] Bajo `cfg(test)`, `areas::file::path()` suma un contador atómico al nombre, así cada store
  creado en un test tiene su archivo (fable r2 #5).
- [ ] Test: `Window::new(&Application::new())` sin `startup` y `area_store()` funciona, con la
  ruta bajo el temporal.
- [ ] Commit `feat: load the saved areas at startup`.

## Tarea 0.5 (orquestador, entre la tanda 1 y la tanda 2): una sola conversión

- [ ] Borrar de `src/area_marker/` los helpers privados de conversión lógico↔físico, de
  `describe()` y de la lista de monitores, y usar `areas::model` y `areas::monitors`. Los tests
  del chunk 2 siguen pasando.
- [ ] Anotar en este plan la firma real de `save_dialog::ask` y de `area_marker::mark` antes de
  lanzar el chunk 5.
- [ ] Commit `refactor: use one pixel conversion for marking and saving areas`.

---

## Chunk 8 · Traducciones e integración final (orquestador, tanda 4)

### Tarea 8.1: traducciones

- [ ] Sumar a `po/POTFILES.in` los archivos nuevos con `gettext` y las `.ui` nuevas.
- [ ] Traducir al castellano en `po/es.po` todas las cadenas nuevas (`ninja -C _build
  kooha-update-po` si está, o `msgmerge` a mano), con el tono de las existentes.

### Tarea 8.2: README y verificación final

- [ ] `README.md` del fork (no hay `README.es.md`): sección corta de rectángulos guardados y
  del codificador.
- [ ] Suite, clippy y fmt en los dos modos; comando de atribución vacío.

### Prueba de la etapa 8 (orquestador, con `run-dev.sh` y `KOOHA_MARKER_TIMEOUT_SECS=60`)

Cada criterio de aceptación de la spec, en la sesión KDE real. El criterio 14 (sin capas) en el
Xvfb aislado con `GDK_BACKEND=x11` y `dbus-run-session` (sin portal): se comprueba que la fila
de área no aparece, que la bandeja no suma el submenú Área y que Grabar no abre capas; la
grabación con el selector de upstream la cubre `selector_path_crops_before_scaling`. La primera
prueba de la capa de marcado comprueba además cómo reparte KWin el foco entre las capas. Lo que no se puede probar sin Sebastián (elegir en el
diálogo de KDE) se le pide en el aviso de la primera versión.

---

## Riesgos conocidos

- **Teclado exclusivo en la capa de marcado:** con `KeyboardMode::Exclusive`, si el proceso se
  cuelga con las capas abiertas, KDE queda sin teclado para otras ventanas. Mitigación: Escape y
  el botón derecho cancelan, un solo marcado a la vez, las capas se destruyen en el `Drop` de
  `AreaMarker`, y `KOOHA_MARKER_TIMEOUT_SECS` corta el marcado en las pruebas.
- **KDE y el tamaño del stream:** si KDE informa el tamaño lógico (no el físico), `match_stream`
  lo acepta (compara contra los dos), y la sonda de `AreaCrop` valida el cuadro real.
- **`vah264lpenc` y tamaños:** el modo *low power* puede exigir anchos alineados a 16; la
  medición lo dice. Si falla con 1280×720 no hay problema; con tamaños arbitrarios, la prueba
  previa con un tamaño fijo no lo detecta: la cadena Intel lleva `vapostproc` que puede ajustar,
  y si aun así falla, el error de la grabación sugiere CPU.
- **Fable y Codex revisan en paralelo con la implementación** (pedido de Sebastián): un
  hallazgo del plan que cambie un chunk ya integrado entra como arreglo en la ronda 1 del
  código.
