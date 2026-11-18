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
| 2 | Capa de marcado | 2.1–2.5 | 0 | `src/area_marker/*` | 1 |
| 3 | Medición y pipeline | 3.1–3.4 | 0 | `tools/`, `docs/perf/`, `src/pipeline.rs` | 1 |
| 4 | Cámara: mantener los cuadros | 4.1–4.3 | 0 | `Cargo.toml`, `src/camera_pip/v4l2.rs`, `src/camera_pip/mod.rs`, `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui` | 1 |
| 5 | Ventana, borde y grabación | 5.1–5.5 | 1, 2, 3 | `src/window/*`, `data/resources/ui/window.ui`, `src/recording.rs`, `src/area_border/mod.rs`, `src/area_border/geometry.rs` | 2 |
| 6 | Ventana Rectángulos y bandeja | 6.1–6.4 | 1, 2 | `src/areas_window.rs`, `data/resources/ui/areas_window.ui`, `data/resources/resources.gresource.xml`, `src/tray/*`, `src/application.rs`, `src/main.rs` (una línea `mod areas_window;`) | 2 |
| 7 | Codificador | 7.1–7.4 | 3, 4 | `src/encoder.rs`, `src/profile.rs`, `data/resources/profiles.yml`, `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui`, `src/doctor/checks.rs`, `src/main.rs` (una línea `mod encoder;`) | 2 |
| 8 | Traducciones e integración final (orquestador) | 8.1–8.2 | todos | `po/es.po`, `po/POTFILES.in`, `README*.md` | 3 |

**Conflictos de la tanda 2 resueltos por reparto:** `src/main.rs` lo tocan 6 y 7 con una línea
cada uno en lugares distintos de la lista de `mod`; el orquestador resuelve ese merge trivial.
`src/recording.rs` sólo lo toca el chunk 5; el chunk 7 entrega el codificador a través de
`Profile::attach` (tarea 7.3), sin tocar `recording.rs`. La acción `app.show-areas` la define el
chunk 6 en `src/application.rs`; el botón «Rectángulos…» del chunk 5 sólo usa su nombre.

**Primera versión instalable:** al integrar los chunks 5 y 6 (etapa 8 de /ciclo).

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
    MonitorInfo { connector: "eDP-1".into(), logical: (457, 1080, 1463, 915), scale: 1.75 }
}
fn dp3() -> MonitorInfo {
    MonitorInfo { connector: "DP-3".into(), logical: (1920, 454, 1600, 1000), scale: 1.05 }
}

#[test]
fn physical_size_rounds_logical_times_scale() {
    assert_eq!(edp().physical_size(), (2560, 1601)); // 1463*1.75=2560.25, 915*1.75=1601.25
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
- [ ] Verificar el redondeo real de eDP-1 contra `kscreen-doctor -o` no hace falta: el borde ya
  tolera ±2 (`SIZE_TOLERANCE`). Ajustar el test si `physical_size` da 1600 por un cálculo
  distinto; lo importante es que la función y su test digan lo mismo que `stream_monitor`.
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
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct AreasFile {
    pub version: u32,                 // 1
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<MarkedArea>,
    #[serde(default)]
    pub areas: Vec<SavedArea>,
    /// `Some(Selected::Named(name))`, `Some(Selected::Last)`, or `None`.
    #[serde(default, with = "selected_serde")]
    pub selected: Option<Selected>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selected { Named(String), Last }
```

`selected` en el JSON: ausente = `None`, `null` = `Last`, string = `Named`. Implementar
`selected_serde` a mano con `serialize_with`/`deserialize_with` (con `Option<Option<String>>` y
`#[serde(default)]` alcanza: ausente → `None`, `null` → `Some(None)`, `"x"` → `Some(Some("x"))`).

- [ ] **Tests que fallan** (con `tempfile`-free: usar `glib::mkdtemp` como `settings::test_utils`):
  - `round_trip`: guardar un `AreasFile` con `last`, dos `areas` y `selected = Named` y volver a
    leerlo da lo mismo; el JSON escrito contiene `"selected": "Demo"`.
  - `selected_null_means_last` y `selected_absent_means_none`, leyendo strings JSON literales.
  - `missing_file_is_empty`: `load(path)` de un archivo que no existe devuelve
    `Ok(AreasFile::default())` con `version` 1.
  - `broken_file_is_moved_aside`: con `{ not json` en el archivo, `load` devuelve
    `Err(LoadError::Broken { moved_to })`, el archivo original ya no existe y `moved_to` existe
    y se llama `rectangles.json.broken-<AAAAMMDD-HHMMSS>`.
  - `save_is_atomic`: `save` escribe en `rectangles.json.tmp` y renombra; después de `save` no
    queda el `.tmp`.
- [ ] Implementar `pub fn path() -> PathBuf` (`glib::user_config_dir()/kooha/rectangles.json`,
  que respeta `XDG_CONFIG_HOME`), `pub fn load(path: &Path) -> Result<AreasFile, LoadError>` y
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
    pub fn selected_area(&self) -> Option<(Option<String>, MarkedArea)>; // (nombre, área)
    pub fn select(&self, selected: Option<Selected>);
    pub fn set_last(&self, area: MarkedArea);            // y la elige
    pub fn save_named(&self, name: &str, area: MarkedArea, replace: bool) -> Result<(), NameTaken>; // y la elige
    pub fn rename(&self, from: &str, to: &str) -> Result<(), NameTaken>; // mantiene la elección
    pub fn remove(&self, name: &str);                     // si era la elegida, queda sin elegida
    pub fn replace_geometry(&self, name: &str, area: MarkedArea);
    pub fn contains(&self, name: &str) -> bool;
}
```

Cada método que cambia algo guarda el archivo y emite `changed`. Un error al guardar se loguea
con `tracing::error!` y no se propaga (la lista en memoria sigue valiendo).

- [ ] **Tests que fallan** (`#[gtk::test]` no hace falta: son objetos GLib; alcanza
  `#[test]`), uno por comportamiento: guardar con nombre lo elige y persiste; nombre repetido
  sin `replace` da `NameTaken` y no cambia nada; con `replace` reemplaza en el mismo lugar de la
  lista; renombrar la elegida mantiene la elección; borrar la elegida deja `selected() == None`;
  `set_last` reemplaza el último y lo elige; `changed` se emite una vez por operación (contador
  con `connect_closure`); recargar desde el mismo path recupera todo.
- [ ] `src/areas/monitors.rs`: `pub fn current(display: &gdk::Display) -> Vec<MonitorInfo>`
  (de `display.monitors()`, sólo `is_valid()`, `connector()` o `"unknown-<n>"`, `geometry()`,
  `scale()`) y `pub fn monitor_for(display, connector) -> Option<gdk::Monitor>`. Sin test unitario
  (depende de la pantalla); lo cubre la prueba manual de la etapa 8.
- [ ] Commit `feat: keep the list of saved areas and the chosen one`.

---

## Chunk 2 · Capa de marcado (tanda 1)

Todo bajo `#[cfg(feature = "layer-shell")]` salvo `geometry.rs`, que es puro y se compila
siempre.

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

- [ ] Test `#[gtk::test]` sin `present()`: construir la capa para el primer monitor del display
  y verificar que `view.state()` arranca con el rectángulo inicial pasado. (La interacción real
  se prueba en la etapa 8.)
- [ ] Commit `feat: draw the area marker layer on each monitor`.

### Tarea 2.3: un monitor a la vez

**Archivos:** `src/area_marker/mod.rs`.

`AreaMarker` crea una `MarkerLayer` por monitor de `areas::monitors::current()`. Un `press` en
una capa borra el rectángulo de las demás (`clear()`): el área vive en un solo monitor.

```rust
pub async fn mark(initial: Option<MarkedArea>) -> anyhow::Result<MarkedArea>;
```

- `initial` con monitor conectado y no desactualizado precarga su rectángulo en esa capa.
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
pub async fn ask(parent: &impl IsA<gtk::Widget>, area: &MarkedArea, store: &AreaStore) -> SaveChoice;
```

Con nombre ya existente en el `store`, un segundo `AlertDialog`: «There is already an area
called “{name}” ({describe})», respuestas «Change Name» (vuelve al primero con el nombre) y
«Replace» (destructiva). Devuelve `Named(name)` sólo si no existe o se eligió Replace; quien
llama guarda con `replace = true` en ese caso.

- [ ] Test puro de la decisión `fn choice_for(name: &str, exists: bool, replace_answer: Option<bool>) -> ...`
  (nombre vacío o sólo espacios → `Last`; recorta espacios).
- [ ] Commit `feat: offer to save a marked area with a name`.

---

## Chunk 3 · Medición y pipeline (tanda 1)

### Tarea 3.1: script de medición

**Archivos:** `tools/bench-capture.py` (ejecutable, `#!/usr/bin/env python3`), con docstring de
uso.

Dos fuentes:
- `--source test` (por defecto): `videotestsrc pattern=ball is-live=true` a 2560×1600, formato
  `BGRx`, a los FPS pedidos. No toca el portal ni la sesión gráfica.
- `--source portal`: abre `org.freedesktop.portal.ScreenCast` por `Gio.DBusProxy` en el bus de
  sesión, con `types=1` (monitor), `persist_mode=2` y `restore_token` leído con
  `gsettings get io.github.seadve.Kooha screencast-restore-token`; al recibir el token nuevo lo
  escribe con `gsettings set`. Usa `OpenPipeWireRemote` y `pipewiresrc fd=… path=<node>`.
  **Sólo lo corre el orquestador** (tarea 3.4), nunca un subagente.

Variantes (`--variant`, repetible; `all` por defecto), con un área de 1280×720 en (200, 160):
`today` (videorate → videoscale → videocrop → x264 como `profiles.yml`), `crop-first`
(videorate → videocrop → x264), `crop-first-queues` (con `queue max-size-buffers=3
max-size-time=0 max-size-bytes=0` antes del recorte y `queue max-size-buffers=30
max-size-time=0 max-size-bytes=0` antes del codificador), `intel` (crop-first-queues +
`vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! vah264lpenc ! h264parse`),
`nvidia` (crop-first-queues + `cudaupload ! cudaconvert ! video/x-raw(memory:CUDAMemory),format=NV12 ! nvh264enc ! h264parse`),
y en modo portal además `intel-dmabuf` (pipewiresrc negociando `video/x-raw(memory:DMABuf)` →
`vapostproc` con `video-direction`/crop por `GstVideoCropMeta` si el elemento lo acepta; si la
negociación falla, la variante se reporta como «no negocia» con el error).

Todas terminan en `mp4mux ! filesink` en `/tmp/kooha-bench-<variante>.mp4`. `--seconds 20` y
`--fps 60` por defecto (`--fps 30` para la segunda pasada).

Medidas por variante: CPU del proceso (`/proc/self/stat` utime+stime antes y después, en % de
un núcleo), FPS reales a la salida del codificador (pad probe que cuenta buffers con PTS), cuadros
perdidos (huecos entre PTS consecutivos mayores a 1,5 períodos), y si arrancó (error del bus).
Salida: una tabla Markdown por stdout y `--out <archivo>`.

- [ ] Test: `python3 -I tools/bench-capture.py --source test --variant today --seconds 2 --fps 30`
  termina con exit 0 e imprime una fila con `fps` cercano a 30 (es la prueba de que el script
  funciona; no hay suite de Python).
- [ ] Commit `tools: add a capture benchmark for pipeline variants`.

### Tarea 3.2: recortar antes de escalar

**Archivos:** `src/pipeline.rs`.

Hoy (`src/pipeline.rs:116-128`): `videosrc_bin → videoscale → caps pares → videocrop`. Pasa a:

- **Área marcada** (nuevo, lo usa el chunk 5): `PipelineBuilder::area_crop(AreaCrop)` con

```rust
/// What to cut from the whole-monitor stream, in stream (physical) pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AreaCrop { pub x: i32, pub y: i32, pub width: i32, pub height: i32 }
```

  Grafo: `videosrc_bin → videocrop(left=x, top=y) → …`, sin `videoscale`. `right` y `bottom`
  se calculan **al llegar las caps** con una sonda en el pad `sink` de `videocrop`
  (`gst::PadProbeType::EVENT_DOWNSTREAM`, evento `Caps`): `right = frame_w - x - width`,
  `bottom = frame_h - y - height`, recortados a `>= 0`. Así el recorte vale aunque el cuadro
  real difiera en un par de píxeles del tamaño guardado. Si `x + width > frame_w` (o en alto),
  el área se recorta al cuadro y se redondea a par hacia abajo (`tracing::warn!`).
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
  - `selector_path_crops_before_scaling`: con `select_area_data`, el orden de elementos es
    `videocrop` → `videoscale`.
  - Los tests existentes de `compute_crop` siguen pasando sin cambios.
- [ ] Implementar `fn make_area_crop(crop: AreaCrop) -> Result<gst::Element>` con la sonda, y el
  nuevo orden en `build()`.
- [ ] Commit `perf: crop before scaling and skip scaling for marked areas`.

### Tarea 3.3: colas

**Archivos:** `src/pipeline.rs`.

Según lo que diga la medición de la tarea 3.4 (si se corre antes, usar los valores de la
variante `crop-first-queues`): una `queue` `kooha-capture-queue` entre `videosrc_bin` y el
recorte, con `max-size-buffers=3, max-size-time=0, max-size-bytes=0, leaky=downstream` (descarta
los cuadros viejos en vez de frenar a `pipewiresrc`), y la `kooha-videoenc-queue` existente con
`max-size-buffers=30, max-size-time=0, max-size-bytes=0`.

- [ ] Test que falla: las propiedades de las dos colas en un pipeline construido.
- [ ] Commit `perf: keep a burst in the encoder from stalling the capture`.

### Tarea 3.4: correr la medición (orquestador)

- [ ] `tools/bench-capture.py --source test --fps 60` y `--fps 30`.
- [ ] `--source portal --fps 60` en la sesión de Sebastián, con el permiso guardado (P11). Si KDE
  muestra el diálogo, cortar y avisar por el Hilo; Sebastián lo corre (la spec lo prevé).
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

**Archivos:** `src/camera_pip/mod.rs`, `src/camera_pip/devices.rs` (sólo lectura de
`device_path()`).

En `start_pipeline` (`src/camera_pip/mod.rs:269-309`), con el `gst::Device` a mano: si
`settings.camera_keep_framerate()` y `devices::device_path(&device)` da un `/dev/videoN`, leer el
control; si existe y vale distinto de 0, guardar `(path, valor)` en `imp.restore_control` y
escribir 0. Al detener el pipeline (el lugar donde hoy se pone en `Null`) y en `dispose`,
restituir el valor guardado si hay. Errores: `tracing::warn!`, nunca un diálogo.

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

`adw::SwitchRow` «Keep Camera Frame Rate» en el grupo de la cámara (`camera_group`), subtítulo
«In low light the picture gets darker instead of choppy», ligado con
`settings.bind_camera_keep_framerate(&row, "active")` (o `bind("camera-keep-framerate", …)`
como las filas existentes).

- [ ] Test `#[gtk::test]` que construye el diálogo con `settings_lock` y verifica que la fila
  refleja el valor de la clave (como los tests existentes de Preferencias, si los hay; si no,
  sólo la verificación manual de la etapa 8).
- [ ] Commit (se puede unir al anterior si es chico).

---

## Chunk 5 · Ventana, borde y grabación (tanda 2)

### Tarea 5.1: `ToastOverlay` y estado del área en la ventana

**Archivos:** `data/resources/ui/window.ui`, `src/window/mod.rs`.

- [ ] Envolver el contenido de la ventana en un `AdwToastOverlay` (`toast_overlay` como
  `TemplateChild`) y agregar `Window::add_toast(&self, toast: adw::Toast)`.
- [ ] La ventana lee la lista con `Application::get().area_store()`, que agrega la tarea 0.4
  antes de que arranque la tanda 2.
- [ ] Si `store.broken_notice()` devuelve un path al arrancar, toast «The list of areas was
  unreadable; it was moved to {path}».

### Tarea 5.2: fila de área

**Archivos:** `data/resources/ui/window.ui`, `src/window/mod.rs`.

Debajo del selector de modo, visible sólo en modo Selección **y** con capas disponibles:
- `gtk::DropDown` con un `gtk::StringList` armado desde el store: cada guardada como
  `"{name} · {describe}"` (con «⚠ » adelante si está desactualizada), y al final
  `"Last area · {describe}"` si hay último. Se rearma con `store.connect_changed` y con
  `display.monitors().connect_items_changed` (desactualizadas cambian).
- botón ojo (`view-reveal-symbolic` / `view-conceal-symbolic`) ligado a la acción
  `win.show-area-border` (de `settings.create_show_area_border_action()`), atajo `Ctrl+B` con un
  `gtk::ShortcutController` de la ventana (no en `application.rs`);
- botón «Mark Area» → acción `win.mark-area`;
- botón «Areas…» → acción `app.show-areas` (la define el chunk 6).

Elegir en el desplegable llama `store.select(...)`. Una desactualizada se puede elegir (para
ver por qué no graba); el borde no se muestra.

`win.mark-area`: `area_marker::mark(store.selected_area())` → `save_dialog::ask` →
`store.set_last` / `store.save_named(.., replace)`; `MarkAgain` vuelve a marcar con el área
recién marcada como inicial; `Cancelled` no hace nada.

- [ ] Test puro `fn dropdown_items(areas, last, monitors) -> Vec<(String, Selected, bool /*stale*/)>`.
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
  `show-area-border`, el store y los monitores. Cambiar de `Idle` a `Recording` con la misma
  geometría no recrea la capa (comparar `BorderGeometry` y connector).
- [ ] Commit `feat: show the chosen area's border before recording`.

### Tarea 5.4: grabar con el área elegida

**Archivos:** `src/recording.rs`, `src/area_border/mod.rs` (`stream_monitor` → `pub(crate)`).

`CaptureConfig::from_settings` agrega `area: Option<MarkedArea>`: en modo Selección con capas
disponibles, `Application::get().area_store().selected_area()`. Si es `None` en ese modo, la
acción de grabar primero corre `win.mark-area` y graba con lo marcado (en `Window::toggle_record`,
antes de `Recording::new`; si se cancela, no graba).

En `start_inner`, antes del portal: si `area.staleness(...)` es `Some`, error
`ContextWithHelp::new("The area “{name}” is out of date", staleness.describe())` — la ventana lo
muestra como toast con botón «Mark Again» en vez del diálogo de error
(`handle_recording_finished` reconoce el tipo `StaleArea`).

Con área: la sesión del portal pide `SourceType::MONITOR` (sin `WINDOW`, sin múltiple). Después
de `new_screencast_session`:

```rust
pub(crate) enum StreamMatch { Same, Other { shared: Option<(i32, i32)> }, Unknown }
pub(crate) fn match_stream(streams: &[Stream], area: &MarkedArea, monitors: &[MonitorInfo]) -> StreamMatch;
```

Usa `stream_monitor(position, size, (0, 0), …)` con la lista de monitores en el formato de
`area_border` (`((x,y,w,h), scale)`) y compara el connector de ese índice con el del área. Más
de un stream → `Other`. `stream_monitor` → `None` (ambigüedad o sin tamaño) → `Unknown`, que se
acepta con `tracing::warn!` (ADR 0005).

- `Same`/`Unknown`: `pipeline_builder.area_crop(AreaCrop{..})`, `imp.area_border` desde
  `area_border::for_area`. **No se llama a `AreaSelector::select`.**
- `Other` en el primer intento: `screencast_session.close()`, `settings.reset_screencast_restore_token()`,
  toast «Choose {connector} (whole screen) in the sharing dialog» (por una señal nueva de
  `Recording`, `notice(String)`, que la ventana muestra como toast), y un segundo
  `new_screencast_session` sin token.
- `Other` en el segundo: error «You shared {w}×{h}, but the area is on {connector} ({mw}×{mh})».

- [ ] Tests puros de `match_stream` con los tres monitores: stream 1680×1050 sin posición ↔
  área en DP-3 → `Same`; stream 1920×1080 ↔ área en DP-3 → `Other`; dos streams → `Other`; sin
  tamaño ni posición → `Unknown`.
- [ ] Test de `CaptureConfig`: en modo Selección sin capas, `area` es `None` (sigue el selector).
- [ ] Commit `feat: record the chosen area without the upstream selector`.

### Tarea 5.5: verificación del chunk

- [ ] Suite con y sin `--features layer-shell`, clippy en los dos modos, fmt.

---

## Chunk 6 · Ventana Rectángulos y bandeja (tanda 2)

### Tarea 6.1: `area_store` y `app.show-areas`

**Archivos:** `src/application.rs`.

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
«Mark Again» (deshabilitado si el monitor no está conectado; llama
`area_marker::mark(Some(area))` y `store.replace_geometry`), borrar (confirmación
`adw::AlertDialog` destructiva). Lista vacía: `adw::StatusPage` «No Saved Areas» con el botón
«Mark Area» (`win.mark-area`). Pie: «Saved in {path}». Se rearma con `store.connect_changed` y
los cambios de monitores.

- [ ] Test puro `fn row_model(area: &SavedArea, selected: bool, monitors) -> RowModel` (texto,
  qué botones habilitados) con los casos: elegida, desactualizada por desconexión (Mark Again
  deshabilitado), desactualizada por escala (Mark Again habilitado).
- [ ] Commit `feat: add the Areas window to choose, rename, mark again and delete`.

### Tarea 6.3: bandeja

**Archivos:** `src/tray/model.rs`, `src/tray/service.rs`, `src/application.rs`.

- [ ] `TrayCommand` deja de ser `Copy` (pasa a `Clone`) y suma `SelectArea(Selected)`,
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
  `MarkArea` → presentar la ventana y activar `win.mark-area`; `ToggleAreaBorder` → invertir
  `show-area-border`. Refrescar la bandeja con `store.connect_changed` y la clave.
- [ ] Tests: los de `model.rs` (entradas con y sin capas, radio elegido, desactualizada
  deshabilitada) y los de `service.rs` (el submenú se mapea con la cantidad de hijos esperada).
- [ ] Commit `feat: choose areas and toggle their border from the tray`.

### Tarea 6.4: verificación del chunk

- [ ] Suite con y sin `--features layer-shell`, clippy en los dos modos, fmt.

---

## Chunk 7 · Codificador (tanda 2)

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
    /// Runs `videotestsrc num-buffers=2 ! <chain> ! fakesink` once per process; cached.
    pub fn works(self) -> Result<(), String>;
}

/// Encoders to try, in order, for a preference. `AUTO_ORDER` comes from
/// docs/perf/2026-11-18-capture-bench.md.
pub fn candidates(pref: VideoEncoder) -> Vec<H264Encoder>;
pub const AUTO_ORDER: [H264Encoder; 3] = [/* lo que diga la medición */];
pub fn resolve(pref: VideoEncoder, works: impl Fn(H264Encoder) -> bool) -> Option<H264Encoder>;
```

Cadenas (las ajusta la medición si encontró otra mejor):
- `Intel`: `vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! vah264lpenc`
- `Nvidia`: `cudaupload ! cudaconvert ! video/x-raw(memory:CUDAMemory),format=NV12 ! nvh264enc`
- `Cpu`: `videoconvert chroma-mode=none dither=none matrix-mode=output-only n-threads=${N_THREADS} ! x264enc qp-max=17 speed-preset=ultrafast threads=${N_THREADS} ! capsfilter caps=video/x-h264,profile=baseline`

`resolve`: forzado → ese, si `works`, si no `None`; `Auto` → el primero de `AUTO_ORDER` que
`works`, con `Cpu` siempre último.

- [ ] Tests puros de `candidates` y `resolve` con un `works` falso (todos fallan salvo CPU →
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
- [ ] `Profile::attach(…, encoder: H264Encoder)`; el único llamador
  (`src/pipeline.rs`, `PipelineBuilder::build`) se cambia **en el chunk 7** pasando
  `self.encoder`, con `PipelineBuilder::encoder(H264Encoder)` (default `Cpu`). `recording.rs`
  no cambia: `PipelineBuilder::new` resuelve el codificador leyendo la clave `video-encoder`
  con `encoder::resolve(settings.video_encoder(), |e| e.works().is_ok())`. Si da `None`
  (forzado y no anda), `build()` falla con `ContextWithHelp` «The {encoder} encoder is not
  working» + el motivo, y el botón al reporte de requisitos.
- [ ] Tests: el `mp4` construido con cada codificador parsea (con `Cpu` siempre; con Intel y
  NVIDIA sólo si `is_installed`); el texto sin `${H264_ENCODER}` queda igual que antes para
  `webm-vp8` y `gif`.
- [ ] Commit `feat: pick the H.264 encoder per recording`.

**Nota de conflicto:** este chunk toca `src/pipeline.rs` (una línea en `build` y el campo
`encoder`), igual que el chunk 3 de la tanda 1. El chunk 7 sale **después** de integrar el
chunk 3, así que no hay dos chunks de la misma tanda en ese archivo; el chunk 5 no toca
`pipeline.rs` (sólo llama `area_crop`).

### Tarea 7.3: preferencia

**Archivos:** `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui`.

- [ ] `adw::ComboRow` «Encoder» después de la fila de formato, visible sólo si el perfil
  elegido es `mp4` o `matroska-h264`. Modelo: Automatic, Intel, NVIDIA, CPU; las no instaladas
  aparecen con «(not installed)» y no se pueden elegir (factory con `set_sensitive(false)`).
  Subtítulo con el codificador que usaría Automático hoy («Automatic uses Intel»), calculado con
  `resolve` en un `glib::spawn_future_local` para no frenar el diálogo.
- [ ] Commit `feat: choose the video encoder in Preferences`.

### Tarea 7.4: reporte de requisitos

**Archivos:** `src/doctor/checks.rs`.

- [ ] El chequeo de VA-API (`:106`, hoy lista `vapostproc` y `vah264enc`) pasa a
  `vapostproc` + `vah264lpenc`, y se suma uno de NVENC (`cudaupload`, `cudaconvert`,
  `nvh264enc`). Las dos fallas son advertencias (Kooha sigue grabando con CPU), con el paquete a
  instalar (`gstreamer1.0-plugins-bad`) en el comando.
- [ ] Ajustar los tests existentes del doctor.
- [ ] Commit `feat: report the hardware encoders in the requirements check`.

---

## Tarea 0.4 (orquestador, entre la tanda 1 y la tanda 2): `area_store` en la aplicación

**Archivos:** `src/application.rs`. Va después de integrar el chunk 1, porque usa
`AreaStore::load_default()`, y antes de lanzar los chunks 5 y 6, que lo usan.

- [ ] `imp::Application` suma `area_store: OnceCell<AreaStore>`, creado en `startup` con
  `AreaStore::load_default()`, y el getter `pub fn area_store(&self) -> AreaStore`.
- [ ] Commit `feat: load the saved areas at startup`.

---

## Chunk 8 · Traducciones e integración final (orquestador, tanda 3)

### Tarea 8.1: traducciones

- [ ] Sumar a `po/POTFILES.in` los archivos nuevos con `gettext` y las `.ui` nuevas.
- [ ] Traducir al castellano en `po/es.po` todas las cadenas nuevas (`ninja -C _build
  kooha-update-po` si está, o `msgmerge` a mano), con el tono de las existentes.

### Tarea 8.2: README y verificación final

- [ ] `README.md`/`README.es.md` del fork: sección corta de rectángulos guardados y del
  codificador.
- [ ] Suite, clippy y fmt en los dos modos; comando de atribución vacío.

---

## Riesgos conocidos

- **Teclado exclusivo en la capa de marcado:** con `KeyboardMode::Exclusive`, si el proceso se
  cuelga con las capas abiertas, KDE queda sin teclado para otras ventanas. Mitigación: Escape y
  el botón derecho siempre cancelan, y las capas se destruyen en el `Drop` de `AreaMarker`.
- **KDE y el tamaño del stream:** si KDE informa el tamaño lógico (no el físico), `stream_monitor`
  ya lo acepta (compara contra los dos).
- **`vah264lpenc` y tamaños:** el modo *low power* puede exigir anchos alineados a 16; la
  medición lo dice. Si falla con 1280×720 no hay problema; con tamaños arbitrarios, la prueba
  previa con un tamaño fijo no lo detecta: la cadena Intel lleva `vapostproc` que puede ajustar,
  y si aun así falla, el error de la grabación sugiere CPU.
- **Fable y Codex revisan en paralelo con la implementación** (pedido de Sebastián): un
  hallazgo del plan que cambie un chunk ya integrado entra como arreglo en la ronda 1 del
  código.
