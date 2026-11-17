# Primeras mejoras del fork de Kooha · plan de implementación

> **Para quien implementa:** cada chunk es un subagente que trabaja en su propio worktree y
> entrega **un solo commit** encadenado sobre el anterior. Los pasos usan casillas (`- [ ]`).
> TDD: primero el test que falla, después el código.

**Objetivo:** las seis mejoras de la spec `docs/specs/2026-11-17-first-improvements-design.md`
(avisos de captura, ícono de bandeja, pausa, PiP de cámara, borde del área, reporte de
requisitos), cada una en un commit.

**Arquitectura:** cada mejora es un módulo nuevo con su lógica en funciones puras testeadas
(`capture_warnings.rs`, `tray/`, `pause_gate.rs`, `camera_pip/`, `area_border/`, `doctor/`) y
una capa de cableado chica en `Application`, `Window`, `Recording`, `PipelineBuilder` y
`PreferencesDialog`. Las capas sobre la pantalla usan `gtk4-layer-shell` detrás del feature
`layer-shell` (ADR 0001). La pausa deja el pipeline en `PLAYING` y descarta buffers (ADR 0003).

**Stack:** Rust 2024 (toolchain 1.92.0 por `rustup override` en `/home/sebas/robotin/apps/kooha`),
gtk4 0.11, libadwaita 0.9, gstreamer 0.25, `gsettings-macro` 0.2, `ksni` 0.3, `gtk4-layer-shell`
0.8, Meson.

**Rama:** `t-1088-first-improvements`, que sale de `main` (`d5e68e3`). Repo de trabajo:
`/home/sebas/robotin/apps/kooha/.workspace/t-1088-first-improvements/repo`. El checkout
original `/home/sebas/robotin/apps/kooha` no se toca.

---

## Decisiones del grill que el plan tiene que respetar

| P | Decisión | Dónde la cubre el plan |
|---|---|---|
| P1 | Aviso por notificación, cartel en la ventana grabando y en la cuenta regresiva | Chunk 1, tareas 1.3 a 1.5 |
| P2 | Un aviso por fuente; con las dos apagadas, «Without audio» | Chunk 1, tarea 1.2 |
| P3 | Ícono de bandeja siempre que Kooha esté abierto | Chunk 2, tarea 2.4 |
| P4 | Menú: estado, start/stop, pausa, toggles, cámara, ventana y salir | Chunks 2, 3 y 4 |
| P5 | Toggles de audio y puntero deshabilitados durante una grabación | Chunk 2, tarea 2.2 |
| P6 | Pausa con corte; destello blanco opcional al reanudar | Chunk 3 |
| P7 | Pausa con botón en la ventana y Ctrl+K | Chunk 3, tarea 3.5 |
| P8 | PiP como capa en pantalla | Chunk 4 (ADR 0002) |
| P9 | Círculo y rectángulo redondeado, alternados con botón derecho | Chunk 4, tareas 4.1 y 4.4 |
| P10 | Rueda del mouse para el tamaño | Chunk 4, tareas 4.1 y 4.4 |
| P11 | Fuente elegida en Preferencias y en la ventana | Chunk 4, tarea 4.5 |
| P12 | PiP visible desde que se prende | Chunk 4, tarea 4.4 |
| P13 | Espejado antes de grabar, sin espejar grabando | Chunk 4, tareas 4.1 y 4.4 |
| P14 | Borde en cuenta regresiva, grabando y en pausa | Chunk 5, tarea 5.3 |
| P15 | Borde punteado blanco sobre gris claro, opaco | Chunk 5, tarea 5.2 |
| P17 | Ocho chequeos | Chunk 6, tarea 6.2 |
| P18 | Checklist con detalle sólo en las fallas, copiar, `--check`, botón en el error | Chunk 6 |
| P16 | gtk4-layer-shell opcional; aviso en Preferencias | Chunk 4, tareas 4.0 y 4.5 |
| P19 | Inglés en el código y traducción en `po/es.po` | Todos los chunks, último paso |
| P20 | Fork limpio: un commit por mejora, estilo de Kooha | Todos los chunks |
| P21 | Todo en esta Task, en orden, commits encadenados | Grafo de chunks |

---

## Comandos de verificación

Todos desde el repo de trabajo. `CARGO_HOME` y `--target-dir` reusan la caché del build de
Meson.

```bash
export CARGO_HOME=$PWD/_build/cargo-home
export KOOHA_TEST_RESOURCES=$PWD/_build/data/resources/resources.gresource
ninja -C _build                                            # build completo; genera los recursos
# a mitad de un paso TDD que todavía no compila: ninja -C _build data/resources/resources.gresource
cargo test --target-dir _build/src                         # suite
cargo test --target-dir _build/src --features layer-shell  # suite con capas (desde el chunk 4)
cargo clippy --target-dir _build/src -- -D warnings        # linter (y con --features layer-shell)
cargo fmt --check                                          # formato
```

`ninja` va **antes** de `cargo test`: los tests de perfiles cargan el `resources.gresource`
generado (codex r2 #1, fable r2 #2). `KOOHA_TEST_RESOURCES` le dice a los tests dónde está; sin
la variable, cargan `config::RESOURCES_FILE` como hoy. Para que `meson test` también la tenga,
agregarla al `env` de los dos `test()` de `src/meson.build`
(`'KOOHA_TEST_RESOURCES=' + meson.project_build_root() / 'data' / 'resources' / 'resources.gresource'`).

**Tests de GTK:** todo test que toca GTK usa `#[gtk::test]`, que corre esos tests en un único
hilo (gtk4-macros 0.11.0, `src/lib.rs` líneas 416–428; el repo ya lo usa en `src/timer.rs`,
línea 200). Llamar `gtk::init()` a mano desde tests comunes entra en pánico desde el segundo
hilo (gtk4 0.11.2, `src/rt.rs` línea 132; codex r2 #2, fable r2 #4). La sesión de los agentes
tiene `WAYLAND_DISPLAY`, así que corren de verdad; ningún test llama `present()` sobre una
ventana, para no dibujar en la pantalla de Sebastián.

Estado de partida (`d5e68e3`): 42 tests pasan; `settings::tests::default_profile` aborta porque
busca el schema en `data/`, donde sólo está el `.gschema.xml.in`. Lo arregla la tarea 1.1.

**Configurar Meson en cada worktree**, con un prefijo propio dentro del worktree (codex r1
#1): el binario carga `resources.gresource` desde `PKGDATADIR`, que sale del prefijo
(`src/config.rs.in`), así que con `/usr/local` probaría la interfaz del Kooha instalado.

```bash
meson setup _build --prefix=$PWD/_install -Dprofile=development
```

### Ejecutor de desarrollo

Toda prueba que abra Kooha usa
`/home/sebas/robotin/apps/kooha/.workspace/t-1088-first-improvements/run-dev.sh <worktree> [args]`,
nunca el `kooha` del `PATH`. El script (lo deja escrito el orquestador antes del chunk 1):

```sh
#!/bin/sh
# Builds, installs into <worktree>/_install and runs that copy, isolated
# from the installed Kooha: own resources, own schema, own settings file.
set -e
WT=$(realpath "$1"); shift
ninja -C "$WT/_build"
meson install -C "$WT/_build" --no-rebuild --quiet
glib-compile-schemas "$WT/_install/share/glib-2.0/schemas"
export GSETTINGS_SCHEMA_DIR="$WT/_install/share/glib-2.0/schemas"
export GSETTINGS_BACKEND=keyfile
export XDG_CONFIG_HOME=/home/sebas/robotin/apps/kooha/.workspace/t-1088-first-improvements/dev-config
# The isolated XDG_CONFIG_HOME hides ~/.config/user-dirs.dirs, so pin the
# recordings folder inside the workspace instead of letting Kooha fall back
# to the real home (fable r2 #7).
REC=/home/sebas/robotin/apps/kooha/.workspace/t-1088-first-improvements/dev-recordings
mkdir -p "$REC"
gsettings set io.github.seadve.Kooha.Devel saving-location "b'$REC'"
exec "$WT/_install/bin/kooha" "$@"
```

`GSETTINGS_BACKEND=keyfile` con su propio `XDG_CONFIG_HOME` es lo que aísla las
preferencias (codex r1 #3): el schema fija `path="/io/github/seadve/Kooha/"` también para el
APP_ID `.Devel`, así que con dconf compartiría la configuración, la selección y el token del
portal del Kooha instalado.

## Grafo de chunks

P21 pide los commits encadenados en el orden de las mejoras, y casi todos los chunks tocan los
mismos archivos (`application.rs`, `window/mod.rs`, `window.ui`, el schema, `po/es.po`). Por eso
el grafo es una cadena: una tanda por chunk, un chunk por tanda.

| Chunk | Mejora | Depende de | Tanda | Archivos nuevos | Archivos que modifica |
|---|---|---|---|---|---|
| 1 | Avisos de captura | — | 1 | `src/capture_warnings.rs` | `src/settings.rs` (test), `src/profile.rs` (test), `src/meson.build`, `src/recording.rs`, `src/window/mod.rs`, `data/resources/ui/window.ui`, `data/resources/style.css`, `src/application.rs`, `src/main.rs`, `po/POTFILES.in`, `po/es.po` |
| 2 | Ícono de bandeja | 1 | 2 | `src/tray/mod.rs`, `src/tray/model.rs`, `src/tray/service.rs` | `Cargo.toml`, `Cargo.lock`, `src/application.rs`, `src/window/mod.rs`, `src/main.rs`, `po/*` |
| 3 | Pausa | 2 | 3 | `src/pause_gate.rs` | `Cargo.toml`, `Cargo.lock` (`gstreamer-pbutils` de desarrollo), `src/pipeline.rs`, `src/recording.rs`, `src/window/mod.rs`, `data/resources/ui/window.ui`, `data/resources/ui/shortcuts.ui`, `src/application.rs`, schema, `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui`, `src/tray/*`, `po/*` |
| 4 | PiP de cámara | 3 | 4 | `src/overlay.rs`, `src/camera_pip/{mod,geometry,devices,view}.rs` | `Cargo.toml`, `meson.build`, `meson_options.txt`, `src/meson.build`, schema, `src/application.rs`, `src/window/mod.rs`, `src/recording.rs`, `window.ui`, `style.css`, `preferences_dialog.*`, `src/tray/*`, `po/*` |
| 5 | Borde del área | 4 | 5 | `src/area_border/{mod,geometry,view}.rs` | `src/pipeline.rs`, `src/recording.rs`, `src/window/mod.rs`, `src/application.rs`, `style.css`, `po/*` |
| 6 | Reporte de requisitos | 5 | 6 | `src/doctor/{mod,checks,dialog}.rs` | `src/main.rs`, `src/application.rs`, `src/window/mod.rs`, `window.ui`, `preferences_dialog.rs`, `po/*` |

**Cómo se entrega cada chunk:** el subagente trabaja en `wt/c<N>`, rama
`t-1088-first-improvements-c<N>`, que sale de la punta de la rama de trabajo con el chunk
anterior ya integrado. Puede hacer commits intermedios; antes de entregar los aplasta en
**un solo commit** (`git reset --soft <punta de la rama de trabajo>` y `git commit`) con el
mensaje que dice el chunk. El orquestador integra con `git merge --ff-only`, así la historia
queda lineal. Ningún commit lleva `Co-Authored-By` ni «Generated with».

**Estilo:** el de Kooha. Comentarios y textos en inglés; `tracing` para logs; errores con
`anyhow`; `clone!` con `#[weak]` en las clausuras de GTK; nada de `unwrap()` fuera de tests
salvo donde el código existente ya lo hace por el mismo motivo. Cada texto visible pasa por
`gettext` y entra en `po/es.po` con su traducción.

**Traducciones:** cada chunk agrega sus archivos nuevos con textos a `po/POTFILES.in` y sus
`msgid` nuevos a `po/es.po`, con `msgstr` en castellano y la referencia `#: src/archivo.rs`.
Verificación: `msgfmt --check -o /dev/null po/es.po` sale con 0.

---

## Chunk 1: Avisos de captura

Commit: `feat: warn when a recording starts without audio or pointer`.

### Tarea 1.1 · Arreglar el test de schema de upstream

**Archivos:** modificar `src/settings.rs` (módulo `tests`, función `setup_schema`).

El test compila `data/` con `glib-compile-schemas`, pero ahí está
`io.github.seadve.Kooha.gschema.xml.in` con `@app-id@` sin reemplazar, así que el schema
`config::APP_ID` no existe y el test aborta. Lo mismo va a afectar a todo test nuevo que use
`Settings`.

- [ ] **Paso 1:** correr `cargo test --target-dir _build/src settings::tests::default_profile`
  (con las variables de «Comandos de verificación»). Esperado: aborta con
  `Settings schema 'io.github.seadve.Kooha.Devel' is not installed`.
- [ ] **Paso 2:** reescribir `setup_schema` para que genere el schema en un directorio temporal:

```rust
fn setup_schema() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let template = include_str!("../data/io.github.seadve.Kooha.gschema.xml.in");
        let schema = template
            .replace("@app-id@", APP_ID)
            .replace("@gettext-package@", crate::config::GETTEXT_PACKAGE);

        let schema_dir = env::temp_dir().join(format!("kooha-test-schemas-{}", std::process::id()));
        fs::create_dir_all(&schema_dir).unwrap();
        fs::write(schema_dir.join(format!("{APP_ID}.gschema.xml")), schema).unwrap();

        let output = Command::new("glib-compile-schemas")
            .arg(&schema_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Failed to compile GSchema for tests: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        unsafe {
            env::set_var("GSETTINGS_SCHEMA_DIR", &schema_dir);
            env::set_var("GSETTINGS_BACKEND", "memory");
        }
    });
}
```

  Hacer `setup_schema` `pub(crate)` y moverla a `#[cfg(test)] pub(crate) mod test_utils` en
  `src/settings.rs`, para que la usen los tests de los otros módulos.
- [ ] **Paso 2b:** mover `init_gresources` de `src/profile.rs` (tests) a `test_utils` como
  `init_resources()`, cargando `std::env::var("KOOHA_TEST_RESOURCES")` o, si no está,
  `config::RESOURCES_FILE`. `Profile::all()` lee `profiles.yml` de los recursos registrados
  (`src/profile.rs` línea 73), así que **todo test que use un perfil llama a `setup_schema()` y
  a `init_resources()`**, empezando por `default_profile`, que hoy pasa sólo si otro test de
  `profile.rs` corrió antes en el mismo proceso. Agregar también
  `test_utils::temp_saving_location(&Settings) -> PathBuf`: crea un directorio temporal y lo
  fija en `saving-location`, para que ningún test cree `~/Videos/Kooha` (fable r2 #7).
- [ ] **Paso 3:** `cargo test --target-dir _build/src`. Esperado: 43 tests pasan, también
  corriendo `default_profile` solo con filtro.

### Tarea 1.2 · Cálculo de los avisos

**Archivos:** crear `src/capture_warnings.rs`; registrar `mod capture_warnings;` en
`src/main.rs`.

- [ ] **Paso 1: test que falla.**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn labels(w: &CaptureWarnings) -> Vec<String> {
        w.labels()
    }

    #[test]
    fn nothing_missing() {
        let w = CaptureWarnings::compute(true, true, true, true);
        assert!(w.is_empty());
        assert!(labels(&w).is_empty());
    }

    #[test]
    fn only_microphone_missing() {
        let w = CaptureWarnings::compute(true, true, false, true);
        assert_eq!(labels(&w), ["Without microphone"]);
    }

    #[test]
    fn only_desktop_audio_missing() {
        let w = CaptureWarnings::compute(true, false, true, true);
        assert_eq!(labels(&w), ["Without desktop audio"]);
    }

    #[test]
    fn both_audio_sources_missing() {
        let w = CaptureWarnings::compute(true, false, false, true);
        assert_eq!(labels(&w), ["Without audio"]);
    }

    #[test]
    fn profile_without_audio_support() {
        let w = CaptureWarnings::compute(false, true, true, true);
        assert_eq!(labels(&w), ["Without audio"]);
    }

    #[test]
    fn pointer_missing_goes_last() {
        let w = CaptureWarnings::compute(true, true, false, false);
        assert_eq!(labels(&w), ["Without microphone", "Without mouse pointer"]);
    }
}
```

- [ ] **Paso 2:** correr `cargo test --target-dir _build/src capture_warnings`. Esperado: no
  compila (`CaptureWarnings` no existe).
- [ ] **Paso 3: implementación.**

```rust
use gettextrs::gettext;

/// What a recording that is about to start will not capture.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "KoohaCaptureWarnings")]
pub struct CaptureWarnings {
    pub missing_desktop_audio: bool,
    pub missing_microphone: bool,
    pub missing_pointer: bool,
}

impl CaptureWarnings {
    pub fn compute(
        profile_supports_audio: bool,
        record_desktop_audio: bool,
        record_microphone: bool,
        show_pointer: bool,
    ) -> Self {
        Self {
            missing_desktop_audio: !profile_supports_audio || !record_desktop_audio,
            missing_microphone: !profile_supports_audio || !record_microphone,
            missing_pointer: !show_pointer,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.missing_desktop_audio && !self.missing_microphone && !self.missing_pointer
    }

    /// Translated labels, audio first.
    pub fn labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        match (self.missing_desktop_audio, self.missing_microphone) {
            (true, true) => labels.push(gettext("Without audio")),
            (true, false) => labels.push(gettext("Without desktop audio")),
            (false, true) => labels.push(gettext("Without microphone")),
            (false, false) => {}
        }
        if self.missing_pointer {
            labels.push(gettext("Without mouse pointer"));
        }
        labels
    }
}
```

  (`use gtk::glib;` arriba.) Los tests corren sin catálogo de traducción, así que `gettext`
  devuelve el `msgid`.
- [ ] **Paso 4:** el test pasa.

### Tarea 1.3 · Una sola foto de la configuración por grabación

**Archivos:** modificar `src/recording.rs`.

Hoy `start_inner` lee `settings` en varios momentos: el puntero antes del portal, el audio
después del portal y del selector de área (`src/recording.rs`, líneas 163 y 218). Mientras el
portal espera, el estado sigue en `Init`. Un cambio en la bandeja en ese rato dejaría los avisos
contradiciendo lo que se graba (codex r1 #9). Por eso la grabación toma **una** foto al
empezar y usa sólo esa.

```rust
/// Everything a recording reads from the settings, read once when it starts.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub profile: &'static Profile,
    pub record_desktop_audio: bool,
    pub record_microphone: bool,
    pub show_pointer: bool,
    pub capture_mode: CaptureMode,
    pub framerate: gst::Fraction,
    pub record_delay: Duration,
    pub saving_location: PathBuf,
}

impl CaptureConfig {
    pub fn from_settings(settings: &Settings) -> Result<Self>; // NoProfileError if no profile
    pub fn warnings(&self) -> CaptureWarnings;
}
```

- [ ] **Paso 1: test que falla** (`recording::tests`, con `setup_schema()` de la tarea 1.1 y
  `gst::init()`): `config_is_a_snapshot`: crea `Settings::default()` con micrófono apagado,
  toma `CaptureConfig::from_settings`, prende el micrófono en `settings`, y afirma que la foto
  sigue con `record_microphone == false` y que `warnings().labels()` es `["Without microphone"]`.
- [ ] **Paso 2:** no compila; implementar `CaptureConfig`.
- [ ] **Paso 3:** `start_inner` arranca con `let config = CaptureConfig::from_settings(settings)?;`
  y usa `config` en **todas** las lecturas que hoy hace de `settings` (perfil, puntero para el
  `CursorMode`, ruta, framerate, modo de captura, audio, demora). Las escrituras del token del
  portal siguen yendo a `settings`.
- [ ] **Paso 4:** propiedad de sólo lectura `#[property(get)] capture_warnings: Cell<CaptureWarnings>`
  en `imp::Recording`, fijada con `config.warnings()` y `notify_capture_warnings()` justo
  después de tomar la foto, antes del portal. Así los avisos ya están en la cuenta regresiva.
- [ ] **Paso 5:** suite verde.

### Tarea 1.4 · Carteles en la ventana

**Archivos:** modificar `data/resources/ui/window.ui` y `src/window/mod.rs`.

- [ ] **Paso 1:** en `window.ui`, dentro de `recording_page`, debajo del `GtkBox` del
  cronómetro, agregar `GtkFlowBox id="recording_warnings_box"` (`selection-mode` none,
  `halign` center, `max-children-per-line` 2, `visible` false). Lo mismo en `delay_page`,
  debajo de `delay_label`: `GtkFlowBox id="delay_warnings_box"`.
- [ ] **Paso 2: test que falla**, con `#[gtk::test]`:
  `warning_chips(&CaptureWarnings::compute(true, true, false, false))` devuelve dos
  `gtk::Label` con los textos «⚠ Without microphone» y «⚠ Without mouse pointer» y la clase
  `capture-warning`; con avisos vacíos devuelve cero.
- [ ] **Paso 3:** en `src/window/mod.rs`, la función libre `fn warning_chips(w: &CaptureWarnings) -> Vec<gtk::Label>`,
  los `TemplateChild` de las dos cajas, y

```rust
fn update_warnings_boxes(&self) {
    let imp = self.imp();
    let warnings = imp
        .recording
        .borrow()
        .as_ref()
        .map(|(recording, _)| recording.capture_warnings())
        .unwrap_or_default();
    for flow_box in [&*imp.recording_warnings_box, &*imp.delay_warnings_box] {
        flow_box.remove_all();
        for chip in warning_chips(&warnings) {
            flow_box.append(&chip);
        }
        flow_box.set_visible(!warnings.is_empty());
    }
}
```

  Llamarla desde `update_view` y desde un `recording.connect_capture_warnings_notify` en
  `toggle_record` (agregar el handler al vector `handler_ids`).
- [ ] **Paso 4:** en `data/resources/style.css` (existe, lo carga la app), la clase
  `.capture-warning`: fondo `alpha(@warning_color, 0.15)`, color `@warning_fg_color`, padding
  `2px 10px`, `border-radius: 999px`, `font-weight: bold`.
- [ ] **Paso 5:** `ninja -C _build` compila y el test pasa.

### Tarea 1.5 · Notificación al arrancar

**Archivos:** modificar `src/application.rs` y `src/window/mod.rs`.

**Orden TDD** (codex r4 #2): escribir primero los tests de los pasos 2 y 3 y verlos fallar;
después los pasos 1 y 4.

- [ ] **Paso 1:** en `Application`, siguiendo el modelo de `send_record_success_notification`:

```rust
pub fn send_capture_warnings_notification(&self, warnings: &CaptureWarnings) {
    if warnings.is_empty() {
        return;
    }
    let notification = gio::Notification::new(&gettext("Recording started"));
    notification.set_body(Some(&warnings.labels().join(" · ")));
    self.send_notification(Some("capture-warnings"), &notification);
}
```

  Y en `handle_recording_finished` de la ventana, `app.withdraw_notification("capture-warnings")`.
- [ ] **Paso 2: test que falla** de la decisión, pura, en `capture_warnings.rs`:
  `fn should_notify(already_notified: bool, state: RecordingState, warnings: &CaptureWarnings) -> bool`.
  Casos: `Recording` sin notificar y con avisos → `true`; ya notificado → `false` (una pausa y
  su reanudación no notifican de nuevo); `Delayed` → `false`; avisos vacíos → `false`.
- [ ] **Paso 3: test que falla** del cableado (codex r3 #3): `WarningsNotifier` (en
  `capture_warnings.rs`) guarda si ya notificó y recibe el emisor como clausura:
  `fn on_state(&self, state: RecordingState, warnings: &CaptureWarnings, send: impl FnOnce(&CaptureWarnings))`.
  Test `notifies_once_per_recording`: la secuencia `Delayed`, `Recording`, `Paused`,
  `Recording`, `Flushing` llama `send` **una** vez; después de `reset()` (grabación nueva), otra
  vez una.
- [ ] **Paso 4:** en `Window`, un `WarningsNotifier` que se resetea en `toggle_record` al crear
  la grabación; el handler de `connect_state_notify` llama `on_state` con un `send` que usa
  `send_capture_warnings_notification`.
- [ ] **Paso 5:** la prueba con la notificación real va en la etapa 8 (A1).

### Tarea 1.6 · Traducciones, verificación y commit

- [ ] Agregar `src/capture_warnings.rs` a `po/POTFILES.in`. Agregar a `po/es.po`:
  «Without audio» → «Sin audio», «Without desktop audio» → «Sin audio del escritorio»,
  «Without microphone» → «Sin micrófono», «Without mouse pointer» → «Sin puntero del mouse»,
  «Recording started» → «Grabación iniciada».
- [ ] Suite, clippy, fmt, `ninja -C _build` y `msgfmt --check` sin errores.
- [ ] Aplastar y commitear con el mensaje del chunk.

**Criterios cubiertos:** A1, A2 y A3 por los tests de la tarea 1.2 y el cableado de 1.3 a 1.5.

---

## Chunk 2: Ícono de bandeja

Commit: `feat: add a system tray icon with recording controls`.

### Tarea 2.1 · Dependencia

**Archivos:** `Cargo.toml`, `Cargo.lock`.

- [ ] Agregar `ksni = { version = "0.3", default-features = false, features = ["blocking", "async-io"] }`.
  Verificado el 17/11/2026 con un ejemplo mínimo: `blocking` solo no compila; necesita
  `async-io` (o `tokio`) también.
- [ ] `cargo build --target-dir _build/src` compila.

### Tarea 2.2 · Modelo del menú, puro

**Archivos:** crear `src/tray/mod.rs` (reexporta), `src/tray/model.rs`.

El modelo es una foto del estado; el menú se arma con una función pura que se testea sin
D-Bus.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// `Starting` covers a recording that exists but has not begun yet: the portal
/// dialog, the area selector and the countdown (codex r1 #9).
pub enum TrayRecordingState { Idle, Starting, Recording, Paused, Flushing }

#[derive(Debug, Clone, PartialEq)]
pub struct TrayModel {
    pub state: TrayRecordingState,
    pub duration_text: String,          // "00:12"
    pub warnings: Vec<String>,          // CaptureWarnings::labels() of the current recording
    pub record_desktop_audio: bool,
    pub record_microphone: bool,
    pub show_pointer: bool,
    pub audio_supported: bool,          // the active profile supports audio
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ToggleRecord,
    ToggleDesktopAudio,
    ToggleMicrophone,
    TogglePointer,
    ShowWindow,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrayEntry {
    Label(String),                                   // not clickable
    Button { label: String, command: TrayCommand, enabled: bool },
    Check { label: String, checked: bool, command: TrayCommand, enabled: bool },
    Separator,
}

pub fn menu_entries(model: &TrayModel) -> Vec<TrayEntry>;
pub fn icon_name(state: TrayRecordingState) -> &'static str;
pub fn tooltip(model: &TrayModel) -> (String, String); // (title, body)
```

- [ ] **Paso 1: tests que fallan** en `model.rs`:
  - `idle_menu`: estado `Idle` → primera entrada `Label("Ready")`, hay
    `Button{"Start Recording", ToggleRecord, enabled: true}`, las tres `Check` habilitadas,
    termina en `Button("Show Window")` y `Button("Quit")`.
  - `recording_menu_disables_capture_toggles`: estado `Recording` → `Label("Recording — 00:12")`,
    después un `Label` por aviso, `Button("Stop Recording")`, y las tres `Check` con
    `enabled: false` (P5).
  - `starting_and_flushing_disable_toggles`: con `Starting` y `Flushing` las `Check` también
    están deshabilitadas, y en los dos el botón de grabar está deshabilitado (igual que
    `win.toggle-record` en `Delayed` y `Flushing`).
  - `audio_checks_disabled_without_audio_support`: con `audio_supported: false` las dos `Check`
    de audio quedan deshabilitadas aun en `Idle` (igual que `update_audio_actions` de la ventana).
  - `icon_follows_state`: `Idle` y `Starting` → `crate::config::APP_ID` (el ícono de la app, que en
    desarrollo es el `.Devel`; `KoohaTray::icon_theme_path` devuelve `config::PKGDATADIR/../icons`
    para que Plasma lo encuentre aunque esté en `_install`), `Flushing` →
    `"document-save-symbolic"`, `Recording` →
    `"media-record-symbolic"`, `Paused` → `"media-playback-pause-symbolic"`.
  - `tooltip_lists_warnings`: el cuerpo del tooltip contiene cada aviso.
- [ ] **Paso 2:** fallan por no compilar.
- [ ] **Paso 3:** implementar `menu_entries`, `icon_name` y `tooltip`. Textos con `gettext`:
  «Ready», «Recording — {time}», «Paused — {time}», «Starting…» (para `Starting`),
  «Saving…» (para `Flushing`), «Start Recording», «Stop Recording», «Record Desktop Audio»,
  «Record Microphone», «Show Pointer», «Show Window», «Quit». Los que llevan variable usan
  `gettext_f` de `src/i18n.rs` con `{time}`.
- [ ] **Paso 4:** los tests pasan.

### Tarea 2.3 · Servicio StatusNotifierItem

**Archivos:** crear `src/tray/service.rs`.

```rust
pub struct TrayService { handle: Option<ksni::blocking::Handle<KoohaTray>> }

struct KoohaTray {
    model: TrayModel,
    sender: futures_channel::mpsc::UnboundedSender<TrayCommand>,
}

impl ksni::Tray for KoohaTray {
    fn id(&self) -> String { crate::config::APP_ID.into() }
    fn title(&self) -> String { gettext("Kooha") }
    fn icon_name(&self) -> String { model::icon_name(self.model.state).into() }
    fn tool_tip(&self) -> ksni::ToolTip { /* from model::tooltip */ }
    fn activate(&mut self, _x: i32, _y: i32) { let _ = self.sender.unbounded_send(TrayCommand::ShowWindow); }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> { /* map menu_entries(&self.model) */ }
}
```

- Cada `TrayEntry` se traduce a `StandardItem` (botón y etiqueta, esta con `enabled: false`),
  `CheckmarkItem` y `MenuItem::Separator`. El `activate` de cada ítem manda su `TrayCommand`
  por el canal: corre en el hilo de ksni, así que **no** toca GTK.
- `TrayService::spawn(sender) -> Self`: `KoohaTray { … }.spawn()` de
  `ksni::blocking::TrayMethods`. Si falla (no hay `StatusNotifierWatcher`), loguear con
  `tracing::warn!` y quedarse con `handle: None` (criterio B3). Verificar en la documentación
  de ksni 0.3.7 el nombre exacto del método y su manejo de error; si ofrece
  `assume_sni_available`, no usarlo.
- `TrayService::update(&self, model: TrayModel)`: `handle.update(|t| t.model = model)` si hay
  handle.

- [ ] **Paso 1:** test `tray_menu_maps_entries` que construye un `KoohaTray` con un modelo en
  `Idle` y comprueba que `menu()` devuelve la misma cantidad de ítems que `menu_entries` y que
  la primera es un `StandardItem` deshabilitado. No necesita D-Bus.
- [ ] **Paso 1b:** test `spawn_without_watcher_does_not_fail` (criterio B3), marcado
  `#[ignore]` porque necesita un bus de sesión propio, y corrido así, sin tocar el D-Bus de
  Sebastián:
  `dbus-run-session -- cargo test --target-dir _build/src -- --ignored tray::service::tests::spawn_without_watcher_does_not_fail`.
  El test no usa GTK: un proceso GTK dentro de `dbus-run-session` activa los portales en ese
  bus y deja `xdg-desktop-portal-kde` huérfano (fable r3 #6). En ese bus no hay `StatusNotifierWatcher`: `TrayService::spawn` devuelve un servicio sin
  handle, `update` no hace nada y el test termina en menos de 5 s (lo afirma midiendo).
- [ ] **Paso 2–4:** falla, implementar, pasa.

### Tarea 2.4 · Cableado en la aplicación

**Archivos:** modificar `src/application.rs`, `src/window/mod.rs`, `src/main.rs` (`mod tray;`).

**Orden TDD** (codex r4 #2): escribir primero los tres tests del paso 4 y verlos fallar
(no compilan, porque `tray_model`, `apply_tray_command` y `update_tray(snapshot)` no existen);
después los pasos 1 a 3.

- [ ] **Paso 1:** en `imp::Application`, `tray: OnceCell<TrayService>`. En `startup`, crear el
  canal `futures_channel::mpsc::unbounded()`, hacer `TrayService::spawn(sender)` y lanzar con
  `glib::spawn_future_local` un bucle que recibe `TrayCommand` y los despacha en el hilo de
  GTK:

| Comando | Acción |
|---|---|
| `ToggleRecord` | `window.activate_action("win.toggle-record", None)` |
| `ToggleDesktopAudio` | `settings.set_record_desktop_audio(!settings.record_desktop_audio())` |
| `ToggleMicrophone` | igual con `record_microphone` |
| `TogglePointer` | igual con `show_pointer` |
| `ShowWindow` | `window.present()` |
| `Quit` | `window.present()` y después `app.activate_action("quit", None)`, que ya pide confirmación si hay grabación |

  Los tres toggles revisan antes que la ventana no tenga una grabación
  (`window.recording_snapshot().is_none()`): el menú ya los deshabilita, y este chequeo cubre un
  click que llegó tarde. De todos modos la grabación ya no lee esas claves después de su foto
  (tarea 1.3).
- [ ] **Paso 2:** `Application::update_tray(&self, snapshot: Option<(RecordingState, gst::ClockTime, CaptureWarnings)>)`:
  arma el `TrayModel` con el `snapshot` que recibe y con `self.settings()`, y llama
  `tray.update`. **Nunca llama a `Application::window()`** (fable r2 #1): `update_view` corre
  dentro de `Window::constructed`, cuando la ventana todavía no está en la lista de la
  aplicación, y `window()` crearía otra ventana, en un bucle sin fin. Los `connect_*_changed`
  de la aplicación guardan el último `snapshot` recibido en un `RefCell` de
  `imp::Application` y lo reusan. En `Window`, agregar
  `pub fn recording_snapshot(&self) -> Option<(RecordingState, gst::ClockTime, CaptureWarnings)>`:
  `Some` mientras exista la grabación, también en `Init`. La función pura
  `tray_state(Option<RecordingState>) -> TrayRecordingState` (en `model.rs`, con test) mapea
  `None` y `Finished` → `Idle`, `Init` y `Delayed` → `Starting`, y el resto 1:1.
- [ ] **Paso 3:** llamar `update_tray(self.recording_snapshot())` desde: `toggle_record`,
  **inmediatamente después** de `imp.recording.replace(...)` y antes del primer `await`, para
  que la bandeja muestre `Starting` mientras espera el portal (codex r2 #6); `Window::update_view` (cambios de estado),
  el handler de `connect_duration_notify` (sólo si cambia el segundo entero, para no
  actualizar D-Bus cinco veces por segundo), `handle_recording_finished`, y los
  `connect_*_changed` de `record-desktop-audio`, `record-microphone`, `show-pointer` y
  `profile-id` en `Application::startup`.
- [ ] **Paso 4: test** `#[gtk::test] app_and_window_construct`: inicializa `setup_schema()`,
  `init_resources()` y `gst::init()`, construye `Application` y `Window::new(&app)` sin
  presentarla, y termina; detecta el bucle de `constructed` → `window()`. **No** llama
  `register()` ni `run()`: eso correría `startup`, tomaría el nombre D-Bus de Kooha y pondría un
  ícono en la bandeja real de Sebastián (fable r3 #3). `update_tray` no hace nada si el tray
  todavía no existe.
  Test `apply_tray_command_syncs_window_action` (codex r3 #3), con `setup_schema()`: la función
  `apply_tray_command(cmd, &settings, has_recording) -> bool`, que despacha los toggles,
  cambia `record-microphone`, y la acción `settings.create_record_microphone_action()` —la
  misma que usa la ventana— refleja el estado nuevo; con `has_recording = true` no cambia
  nada.
  Otro test `#[gtk::test] tray_shows_starting_while_portal_pending`: con un
  `recording_snapshot` en `Init`, el `TrayModel` que arma `update_tray` tiene estado
  `Starting` y las tres casillas de captura deshabilitadas (se prueba la función que arma el
  modelo, `tray_model(snapshot, &settings) -> TrayModel`, extraída de `update_tray`).
- [ ] **Paso 5:** la prueba de punta a punta del menú se hace en la etapa 8, con el ejecutor
  de desarrollo.
- [ ] **Paso 6:** `Application::window()` y `quit_request` hacen
  `active_window().downcast::<Window>().unwrap()` (`src/application.rs` líneas 111 y 174). Las
  ventanas-capa de los chunks 4 y 5 se crean **sin** la propiedad `application`, así nunca son
  la ventana activa de la app; el plan de esos chunks lo repite.

### Tarea 2.5 · Traducciones, verificación y commit

- [ ] `src/tray/model.rs` y `src/tray/service.rs` a `POTFILES.in`; textos nuevos a `es.po`
  («Listo», «Grabando — {time}», «En pausa — {time}», «Empezando…», «Guardando…»,
  «Empezar a grabar», «Terminar de grabar», «Grabar audio del escritorio», «Grabar micrófono»,
  «Mostrar puntero», «Mostrar ventana», «Salir»).
- [ ] Suite, clippy, fmt, ninja, msgfmt. Aplastar y commitear.

**Criterios:** B1 (las casillas y la ventana leen y escriben las mismas claves de GSettings),
B2 (test `recording_menu_disables_capture_toggles`), B3 (tarea 2.3).

---

## Chunk 3: Pausa

Commit: `feat: re-enable pause without gaps in the recording`.

### Tarea 3.1 · Línea de tiempo de pausas, pura

**Archivos:** crear `src/pause_gate.rs`; `mod pause_gate;` en `src/main.rs`.

```rust
/// Pause intervals in pipeline running time, and the mapping from input
/// timestamps to output timestamps without the paused spans.
#[derive(Debug, Default, Clone)]
pub struct PauseTimeline {
    closed: Vec<(gst::ClockTime, gst::ClockTime)>, // [start, end)
    open: Option<gst::ClockTime>,
}

pub enum Mapped { Drop, Keep(gst::ClockTime) }

impl PauseTimeline {
    pub fn pause(&mut self, at: gst::ClockTime);
    pub fn resume(&mut self, at: gst::ClockTime);
    pub fn is_paused(&self) -> bool;
    /// Drop if `ts` falls inside a pause (open or closed); otherwise
    /// subtract the length of every closed pause that ended at or before `ts`.
    pub fn map(&self, ts: gst::ClockTime) -> Mapped;
    /// Recorded time up to `now`, excluding pauses.
    pub fn recorded_duration(&self, now: gst::ClockTime) -> gst::ClockTime;
    /// Output timestamp where the most recent resume lands, if any.
    pub fn last_resume_output(&self) -> Option<gst::ClockTime>;
}
```

- [ ] **Paso 1: tests que fallan.**
  - `no_pauses_is_identity`: `map(5s)` → `Keep(5s)`.
  - `inside_open_pause_drops`: `pause(2s)`; `map(3s)` → `Drop`; `map(1s)` → `Keep(1s)`.
  - `after_resume_shifts`: `pause(2s)`, `resume(5s)`; `map(6s)` → `Keep(3s)`;
    `map(4s)` → `Drop` (llegó tarde, pero cae dentro de la pausa).
  - `two_pauses_accumulate`: pausas `[2s,5s)` y `[7s,8s)`; `map(10s)` → `Keep(6s)`.
  - `recorded_duration_excludes_pauses`: con las dos pausas, `recorded_duration(10s)` = 6s;
    con una pausa abierta desde 9s, `recorded_duration(12s)` = 5s.
  - `resume_without_pause_is_ignored` y `double_pause_keeps_first_start`.
  - `last_resume_output`: después de `[2s,5s)`, `Some(2s)`.
- [ ] **Paso 2–4:** fallan, implementar, pasan.

### Tarea 3.2 · Compuerta con un pipeline real

**Archivos:** `src/pause_gate.rs`.

```rust
/// Shared between the GTK thread (pause/resume) and streaming threads (probes).
#[derive(Debug, Clone, Default)]
pub struct PauseGate(Arc<Mutex<GateState>>);

struct GateState {
    timeline: PauseTimeline,
    flash: Option<FlashState>, // set only when pause-flash is enabled
}

impl PauseGate {
    pub fn pause(&self, running_time: gst::ClockTime);
    pub fn resume(&self, running_time: gst::ClockTime);
    pub fn is_paused(&self) -> bool;
    pub fn recorded_duration(&self, running_time: gst::ClockTime) -> gst::ClockTime;
    /// Installs a buffer probe on `pad` that drops or re-stamps PTS and DTS.
    pub fn attach(&self, pad: &gst::Pad);
    /// Enables the resume flash driven by `videobalance` (task 3.3).
    pub fn set_flash_element(&self, videobalance: &gst::Element);
}
```

El probe (fable r2 #9):
- tipo `gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST`; una lista se recorre
  buffer por buffer con la misma regla, sacando los que caen en una pausa;
- la compuerta compara en **tiempo de ejecución**: convierte el PTS con el segmento del pad
  (guardado de un probe `EVENT_DOWNSTREAM` sobre `Segment`, con `segment.to_running_time(pts)`),
  aplica `PauseTimeline::map`, y vuelve a posición con `segment.position_from_running_time`.
  Así vale aunque el segmento no arranque en 0;
- si mapea a `Drop` devuelve `gst::PadProbeReturn::Drop`; si mapea a `Keep(t)`, pone el PTS
  convertido y corre el DTS (si tiene) en la misma cantidad. Un buffer sin PTS pasa sin cambios.

Los tests leen los PTS con un probe sobre el pad `sink` de un `fakesink sync=false`, que guarda
cada PTS en un `Arc<Mutex<Vec<_>>>`: sin `gstreamer-app` (que no está en `Cargo.toml`).

**Dónde va** (en `PipelineBuilder::build`, `src/pipeline.rs`): un elemento `identity` llamado
`kooha-video-gate` entre la salida de video (`videocrop` o `videosrc_bin`) y
`kooha-videoenc-queue`; otro `identity` `kooha-audio-gate` entre `audiosrc_bin` y
`kooha-audioenc-queue`. El probe va en el **pad `src`** de cada `identity`. `PipelineBuilder`
recibe el `PauseGate` con un método `pause_gate(&mut self, gate: PauseGate)` y lo engancha.

- [ ] **Paso 1: test de integración que falla**, en `src/pause_gate.rs`, con un pipeline real:
  `videotestsrc is-live=true ! video/x-raw,framerate=30/1 ! identity name=gate ! fakesink name=sink sync=false`.
  El test (sin GTK, con `gst::init()`) arranca el pipeline, espera 300 ms, `gate.pause(rt_pause)`,
  espera 300 ms, `gate.resume(rt_resume)`, espera 300 ms, toma `rt_final`, manda EOS y junta
  los PTS. Afirma:
  - los PTS son crecientes;
  - ningún salto entre PTS consecutivos supera dos cuadros (66 ms) — criterio C2;
  - el último PTS es ≤ `rt_final − (rt_resume − rt_pause)` + un cuadro, con los tiempos de
    ejecución que tomó el propio test (fable r2 #9a): no depende de cuánto duraron los `sleep`.
- [ ] **Paso 1b:** test `segment_not_starting_at_zero`: con un segmento de `start` 10 s
  (empujado con un `identity` y un evento `Segment` propio, o probando la conversión pura con
  un `gst::FormattedSegment` armado a mano), la compuerta descarta y corre igual que con
  `start` 0.
- [ ] **Paso 2:** lo mismo con `audiotestsrc is-live=true` (criterio C2 para el audio).
- [ ] **Paso 2b: video y audio saltan juntos** (codex r1 #14): un solo pipeline con las dos
  ramas, `videotestsrc` y `audiotestsrc` en vivo, cada una con su `identity` y su `fakesink` con probe,
  y **una** compuerta enganchada a los dos pads. Después de pausar 300 ms y reanudar, el primer
  PTS de video posterior a la reanudación y el primero de audio difieren en menos de 50 ms, y
  el último PTS de cada rama difiere del otro en menos de 50 ms.
- [ ] **Paso 2c: EOS estando en pausa**: pausar, mandar EOS sin reanudar; los dos `fakesink`
  reciben EOS en menos de 2 s (el test lo mide y lo afirma), y no llegó ningún buffer con PTS
  dentro de la pausa.
- [ ] **Paso 3–4:** falla, implementar, pasa.

### Tarea 3.3 · Destello de reanudación

**Archivos:** `data/io.github.seadve.Kooha.gschema.xml.in`, `src/pause_gate.rs`,
`src/pipeline.rs`.

- [ ] **Paso 1:** clave `<key type="b" name="pause-flash"><default>false</default></key>`.
  `gsettings-macro` genera `pause_flash()`, `set_pause_flash()` y `bind_pause_flash()`.
- [ ] **Paso 2: test que falla:** pipeline real
  `videotestsrc is-live=true pattern=black ! video/x-raw,format=BGRx,framerate=30/1 ! identity name=gate ! videobalance name=flash ! fakesink`
  (los píxeles se leen en el probe del pad `sink` del `fakesink`);
  con el flash habilitado, pausar y reanudar; los buffers con PTS de salida en
  `[resume_out, resume_out + 150ms)` tienen el primer píxel en blanco
  (`>= 0xF0` en B, G y R), y el primer buffer posterior a esa ventana vuelve a negro — C3.
- [ ] **Paso 3:** implementar. En el probe de video, cuando el PTS de salida cae en la ventana
  de 150 ms desde `last_resume_output()`, poner en el `videobalance` `saturation = 0.0` y
  `brightness = 1.0`; fuera de la ventana, `saturation = 1.0` y `brightness = 0.0`. El probe
  del `identity` corre en el mismo hilo que el `videobalance` que le sigue (no hay `queue` en el
  medio), así que el cambio llega antes que el buffer.
- [ ] **Paso 4:** en `PipelineBuilder`, cuando `pause_flash` es `true`, insertar
  `videobalance` `kooha-pause-flash` entre `kooha-video-gate` y `kooha-videoenc-queue`, y
  llamar `gate.set_flash_element`. Con `false`, no se inserta.

### Tarea 3.4 · La grabación pausa con la compuerta

**Archivos:** `src/recording.rs`.

**Orden TDD** (codex r4 #2): escribir primero los tests del paso 4b (`flushing_progress` y el
pipeline atrasado) y verlos fallar; después los pasos 1 a 5.

- [ ] **Paso 1:** `imp::Recording` guarda un `PauseGate`. `start_inner` se lo pasa al
  `PipelineBuilder` y, si la foto de configuración lo dice, activa el destello: agregar
  `pause_flash: bool` a `CaptureConfig` (tarea 1.3).
- [ ] **Paso 2:** `pause()`: exige estado `Recording`, llama `gate.pause(pipeline.current_running_time())`
  y `self.set_state(RecordingState::Paused)`. `resume()`: exige `Paused`,
  `gate.resume(...)`, `set_state(RecordingState::Recording)`. El pipeline no cambia de estado.
  Si `current_running_time()` es `None`, devolver error con contexto.
- [ ] **Paso 3:** en `handle_bus_message`, `StateChanged` del pipeline: sacar el mapeo
  `gst::State::Paused => RecordingState::Paused`; sólo `Playing` → `Recording`, y sólo si el
  estado actual no es `Paused`.
- [ ] **Paso 4:** `update_duration`: durante `Recording` y `Paused`, la duración es
  `gate.recorded_duration(pipeline.current_running_time())`; en `Flushing` sigue usando
  `query_position` como hoy, que es lo que el pipeline ya procesó. Borrar el
  `// TODO Handle effect on estimated final duration`.
- [ ] **Paso 4b: progreso del guardado** (codex r1 #8). Hoy `stop()` pone `Flushing` y llama
  enseguida `update_flushing_progress`, que divide `self.duration()` por
  `estimated_final_duration`. Con la duración mostrada en tiempo efectivo, las dos serían casi
  iguales y el progreso saltaría a ~100 % antes de vaciar las colas. Cambios:
  - Test primero de una función pura
    `fn flushing_progress(processed: gst::ClockTime, estimated: gst::ClockTime) -> u8`
    (0 con `estimated` cero; redondeo; tope 100) y moverle la cuenta de hoy.
  - Test con un pipeline sintético **atrasado** (codex r3 #3), sin portal:
    `videotestsrc is-live=true ! identity name=gate ! queue ! identity sleep-time=40000 ! fakesink sync=false`
    (cada buffer tarda 40 ms en pasar, más que los 33 ms entre cuadros, así la cola se llena).
    Grabar 1 s, detener (EOS) y muestrear `flushing_progress(query_position, estimated)` cada
    50 ms hasta el EOS: el primer valor es menor que 100 y los valores no bajan. La última
    muestra periódica puede quedar abajo de 100 (Codex midió 93–96 % en tres corridas); la
    aserción final es sobre el procesamiento del EOS, que hoy fija el progreso en 100
    (`src/recording.rs` línea 525) y tiene que seguir haciéndolo (codex r4 #3). Lo mismo
    deteniendo en pausa.
  - `stop()`: `estimated_final_duration = gate.recorded_duration(current_running_time)`;
    después de `set_state(Flushing)`, llamar `update_duration()` (que en `Flushing` ya lee
    `query_position`) **antes** de `update_flushing_progress()`.
  - `update_flushing_progress` usa `flushing_progress(self.duration(), estimated)`.
- [ ] **Paso 5:** `stop()` en `Paused` funciona: el EOS fluye porque el pipeline sigue en
  `PLAYING` y la compuerta deja pasar los eventos (el probe es sólo de buffers).

### Tarea 3.5 · Botón, atajo, bandeja y Preferencias

**Archivos:** `data/resources/ui/window.ui`, `src/application.rs`,
`data/resources/ui/preferences_dialog.ui`, `src/preferences_dialog.rs`, `src/tray/model.rs`,
`src/application.rs`.

- [ ] En `window.ui`, `pause_record_button`: sacar `<property name="visible">False</property>`.
- [ ] En `application.rs`, descomentar `set_accels_for_action("win.toggle-pause", &["<Control>k"])`
  y borrar el comentario del issue 112. En `data/resources/ui/shortcuts.ui`, descomentar el
  `AdwShortcutsItem` de `win.toggle-pause` (líneas 48–53, hoy entre `<!--` y `-->`) y comprobar
  que se ve en la ventana de atajos.
- [ ] En Preferencias, grupo nuevo «Recording» después de «General», con `AdwSwitchRow`
  `pause_flash_row` («Flash on Resume», subtítulo «Mark each resume with a short white
  flash»), ligado con `bind_pause_flash(&row, "active")`.
- [ ] Bandeja: `TrayCommand::TogglePause`. **Test primero:** en `Recording`, después de
  «Stop Recording» viene `Button("Pause", TogglePause)`; en `Paused`, `Button("Resume")`; en
  `Idle` no está. El despacho: `window.activate_action("win.toggle-pause", None)`.

### Tarea 3.6 · Prueba de punta a punta del archivo (criterio C1)

- [ ] Test de integración `#[ignore]` (necesita codificadores; se corre a mano con
  `cargo test -- --ignored pause_gate::tests::file_duration`): un pipeline con
  `videotestsrc is-live=true` y `audiotestsrc is-live=true`, las dos compuertas y el perfil
  `webm-vp8` de Kooha (`Profile::from_id("webm-vp8")` y `attach`, igual que `PipelineBuilder`),
  graba 3 s, pausa 2 s, graba 3 s, EOS, y mide la duración del archivo con
  `gst_pbutils::Discoverer` (agregar `gstreamer-pbutils` sólo como `dev-dependency`). Afirma
  6 s ± 0,3 s.
- [ ] Correrlo también con los perfiles `mp4` y `matroska-h264` si `x264enc` está instalado,
  con una pausa de 4 s, más larga que la latencia de 2 s del `audiomixer`
  (`src/pipeline.rs` línea 393), porque `mp4mux` en vivo trabaja con plazos de reloj (fable r2 #9).
- [ ] Pegar la salida en el informe del chunk.

### Tarea 3.7 · Traducciones, verificación y commit

- [ ] `es.po`: «Pause» → «Pausar», «Resume» → «Reanudar», «Recording» (título del grupo) →
  «Grabación», «Flash on Resume» → «Destello al reanudar», su subtítulo, y «Pause Recording»
  si cambió.
- [ ] Suite, clippy, fmt, ninja, msgfmt. Aplastar y commitear.

---

## Chunk 4: PiP de cámara

Commit: `feat: add a draggable camera picture-in-picture overlay`.

### Tarea 4.0 · Feature `layer-shell` y módulo de capas

**Archivos:** `Cargo.toml`, `meson_options.txt`, `meson.build`, `src/meson.build`, crear
`src/overlay.rs`, `mod overlay;` en `src/main.rs`.

- [ ] `Cargo.toml`: `gtk4-layer-shell = { version = "0.8", optional = true }` y
  `[features] layer-shell = ["dep:gtk4-layer-shell"]`. Verificado el 17/11/2026: el crate
  0.8.1 usa gtk4 0.11, y el binario enlaza `libgtk4-layer-shell.so.0` antes que
  `libwayland-client.so.0`.
- [ ] `meson.build` (raíz): `layer_shell_dep = dependency('gtk4-layer-shell-0', required: get_option('layer-shell'))`,
  con `meson_options.txt`: `option('layer-shell', type: 'feature', value: 'auto')`.
- [ ] `src/meson.build`: si `layer_shell_dep.found()`, agregar `--features`, `layer-shell` a
  `cargo_options` (vale para build, test y clippy).
- [ ] `src/overlay.rs`:

```rust
/// Why overlays (camera PiP, area border) cannot be shown, if they cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayUnavailable {
    NotCompiled,
    CompositorUnsupported,
    /// Compiled in, but there is no display to ask the compositor (`kooha --check`
    /// from a plain terminal). Only the requirements report uses it.
    NoDisplay,
}

/// `NotCompiled` is decided first, without touching GTK, so it is reported even
/// without a display (codex r1 #12).
pub fn availability() -> Result<(), OverlayUnavailable>;

/// Turns `window` into an overlay layer surface on `monitor`.
/// Only callable when `availability()` is Ok.
pub fn init_overlay(window: &gtk::Window, monitor: &gdk::Monitor, namespace: &str);
pub fn set_margins(window: &gtk::Window, left: i32, top: i32);
pub fn anchor_all_edges(window: &gtk::Window);
```

  Con el feature apagado, `availability()` devuelve `Err(NotCompiled)` y las otras funciones no
  existen (`#[cfg(feature = "layer-shell")]`); el código que las usa también va con `cfg`. Con
  el feature: si `gtk::is_initialized_main_thread()` es falso o `gdk::Display::default()` es
  `None`, `Err(NoDisplay)`; si no, `gtk4_layer_shell::is_supported()` decide entre `Ok` y
  `Err(CompositorUnsupported)`. `init_overlay`: `init_layer_shell`, `set_layer(Layer::Overlay)`,
  `set_namespace`, `set_monitor(Some(monitor))`, `set_keyboard_mode(KeyboardMode::None)`,
  `set_exclusive_zone(-1)`, anclas `Top` y `Left`.
- [ ] Test: `availability_without_feature` con `#[cfg(not(feature = "layer-shell"))]` afirma
  `Err(NotCompiled)`.
- [ ] `meson setup --reconfigure _build` y `ninja -C _build`: el log de Meson dice que encontró
  `gtk4-layer-shell-0` (la biblioteca 1.3.0 ya está instalada en la máquina de Sebastián).

### Tarea 4.1 · Geometría y estado del PiP, puros

**Archivos:** crear `src/camera_pip/mod.rs`, `src/camera_pip/geometry.rs`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipShape { Circle, RoundedRectangle }

pub const MIN_SIZE: i32 = 96;
pub const MAX_SIZE: i32 = 640;
pub const EDGE_MARGIN: i32 = 24;

impl PipShape {
    pub fn toggled(self) -> Self;
    /// Width for a given height; the rectangle is 16:9.
    pub fn width_for(self, height: i32) -> i32;
    pub fn corner_radius(self, height: i32) -> f32; // circle: height/2, rect: height*0.12
}

// The `camera-shape` key has <choices>, so gsettings-macro generates the enum
// `settings::CameraShape` and `camera_shape()` / `set_camera_shape()` use it
// (gsettings-macro 0.2.3, src/generators/string.rs; codex r1 #4).
impl From<CameraShape> for PipShape { /* Circle <-> Circle, RoundedRectangle <-> RoundedRectangle */ }
impl From<PipShape> for CameraShape { /* inverse */ }

/// Scroll up grows 10 %, scroll down shrinks 10 %, clamped to MIN_SIZE..=MAX_SIZE.
pub fn resized(size: i32, scroll_dy: f64) -> i32;
/// Largest height <= `size` whose PiP (in `shape`) fits inside the monitor with
/// EDGE_MARGIN on every side; never below MIN_SIZE (codex r1 #7).
pub fn fitted_size(shape: PipShape, size: i32, monitor: (i32, i32)) -> i32;
/// Clamps a top-left position so the whole PiP stays inside the monitor.
pub fn clamp_position(pos: (i32, i32), pip: (i32, i32), monitor: (i32, i32)) -> (i32, i32);
/// Bottom-right with EDGE_MARGIN.
pub fn default_position(pip: (i32, i32), monitor: (i32, i32)) -> (i32, i32);
/// P13: mirrored only while no recording session is running.
pub fn should_mirror(recording_active: bool) -> bool;
```

- [ ] **Tests primero:**
  - `shape_toggles_both_ways`, `shape_converts_to_and_from_setting_enum`, `circle_is_square`,
    `rectangle_is_16_9` (`width_for(180)` = 320).
  - `fitted_size_limits_wide_rectangle`: rectángulo pedido de alto 640 en un monitor de
    1024×768 → `(1024 - 48) * 9 / 16` = 549; en 1920×1080 → 640 sin cambios. Círculo pedido de
    640 en 800×600 → `600 - 48` = 552.
  - `persistence_roundtrip` (con `setup_schema()`): `set_camera_shape(PipShape::RoundedRectangle.into())`
    y `PipShape::from(settings.camera_shape())` devuelve `RoundedRectangle`.
  - `scroll_up_grows_ten_percent` (`resized(200, -1.0)` = 220), `scroll_down_shrinks`
    (`resized(200, 1.0)` = 182 redondeando), `resize_clamps` (96 y 640 en los extremos).
  - `clamp_keeps_inside` (posición negativa → 0; pasada del borde → `monitor - pip`),
    `default_is_bottom_right` (`default_position((200,200),(1920,1080))` = `(1696, 856)`).
  - `mirror_only_without_recording`: `should_mirror(false)` → `true`, `should_mirror(true)` →
    `false`.
- [ ] Implementar hasta que pasen.

### Tarea 4.2 · Fuentes de cámara

**Archivos:** crear `src/camera_pip/devices.rs`.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraSource { pub id: String, pub name: String }

/// Lists `Video/Source` devices. The id is the device's `device.path` property
/// when present, else its display name.
pub fn list() -> Vec<CameraSource>;
/// Picks the configured id, or the first available, or None.
pub fn choose<'a>(sources: &'a [CameraSource], wanted: &str) -> Option<&'a CameraSource>;
/// Creates the source element for `id` with `gst::Device::create_element`.
pub fn create_element(id: &str) -> anyhow::Result<gst::Element>;
```

`list()` usa `gst::DeviceMonitor` con el filtro `Video/Source`, `start()`, `devices()`,
`stop()` — mismo patrón que `src/device.rs`. La misma webcam puede aparecer dos veces, por
PipeWire y por V4L2 (fable r2 #10; no verificado): `list()` pasa el resultado por la función
pura `dedupe(Vec<(CameraSource, Option<String> /* device path */)>) -> Vec<CameraSource>`, que
se queda con la primera de cada ruta, con test.

- [ ] **Tests primero** de `choose` (id configurado presente, ausente → primera, lista vacía →
  `None`). `list()` y `create_element` se prueban en la etapa 8 con la webcam real
  (`/dev/video0` existe en la máquina de Sebastián).

### Tarea 4.3 · Widget de la cámara

**Archivos:** crear `src/camera_pip/view.rs`.

`PipView`, subclase de `gtk::Widget`, con un `gdk::Paintable` (el de `gtk4paintablesink`, que ya
registra `src/main.rs` con `gstgtk4::plugin_register_static`), una `PipShape` y un alto. En
`measure` devuelve `width_for(height)` × `height`. En `snapshot`: `push_rounded_clip` con
`corner_radius`, dibuja el paintable recortado al centro para llenar el rectángulo (*cover*:
escala por el lado que falta y centra), `pop`. Un borde de 2 px `rgba(255,255,255,0.85)`
alrededor.

- [ ] **Invalidaciones** (codex r1 #6), con el mismo patrón que `src/area_selector/view_port.rs`
  (línea 374): al fijar el paintable, conectar `invalidate-contents` → `queue_draw` e
  `invalidate-size` → `queue_resize`, guardar los `SignalHandlerId` y desconectarlos al
  reemplazar el paintable o en `dispose`.
- [ ] Tests con `#[gtk::test]`: `measure`
  devuelve `(200, 200)` con círculo y alto 200, y `(356, 200)` con rectángulo;
  `replacing_paintable_disconnects`: con dos paintables de prueba, después de reemplazar el
  primero por el segundo, invalidar el primero no llega al widget y el segundo sí (contar con
  un `Cell` en un handler de prueba conectado al mismo camino que `queue_draw`).
- [ ] El recorte *cover* sale de una función pura
  `cover_rect(paintable: (f64, f64), target: (f64, f64)) -> graphene::Rect`, con test.

### Tarea 4.4 · Controlador del PiP

**Archivos:** `src/camera_pip/mod.rs`.

`CameraPip` (objeto GLib) dueño de: la ventana-capa, el `PipView`, y un pipeline
`<fuente> ! videoconvert ! gtk4paintablesink`.

La ventana-capa se crea **sin** la propiedad `application` (nunca es la ventana activa de la
app, que `Application::window()` baja a `Window` con `unwrap`), sin decoración, con la clase
CSS `camera-pip` y `window.camera-pip { background: none; }` en `style.css`. Su región de
entrada sigue la forma del PiP: `relayout()` llama
`surface.set_input_region(Some(&region))` con una región armada por la función pura
`shape_input_rects(shape, w, h) -> Vec<(i32, i32, i32, i32)>`, que aproxima el círculo o las
esquinas redondeadas con una franja por fila de 2 px (fable r2 #10). Test: para un círculo de
200, el punto `(100, 100)` está dentro de algún rectángulo y `(2, 2)` no.

- **Prender** (`camera-enabled` pasa a `true`, o al arrancar la app con la clave en `true`):
  si `overlay::availability()` es `Err`, no hace nada. Si no hay fuentes, pone
  `camera-enabled` en `false` y manda la notificación «No camera found». Si hay: arma el
  pipeline con `devices::choose(list(), settings.camera_device())`, lo pone en `PLAYING`, crea
  la ventana con `overlay::init_overlay` en el monitor que resuelve
  `resolve_monitor(saved_connector, kooha_window_monitor, monitors)` — función pura con test:
  el de `camera-monitor` si está conectado; si no, el monitor donde está la ventana de Kooha
  (la ventana se lo pasa con `CameraPip::set_kooha_monitor(Option<gdk::Monitor>)` desde
  `surface.connect_enter_monitor`, gdk4 0.11.2 `src/auto/surface.rs` línea 274: en Wayland el
  compositor avisa en qué monitor está la superficie recién después del primer commit, así que
  consultarlo en `map` daría el monitor del origen; fable r3 #1); si no, el primero de la
  lista —, posición. Si `camera-monitor` está vacío y el PiP ya está prendido cuando llega
  `set_kooha_monitor` (Kooha abrió con la cámara prendida desde `startup`, antes de que exista
  la ventana), el `CameraPip` recrea la ventana-capa en ese monitor y hace `relayout()`
  (fable r4 #2) —
  `camera-position` (si es `(-1,-1)`, `default_position`), `clamp_position`, y `present()`.
- **Apagar**: pipeline a `NULL`, `window.destroy()`.
- **Mover**: `GestureDrag` con botón 1. En cada `drag-update`, como la superficie se mueve con el
  puntero, el `offset` que reporta GTK es relativo a la superficie ya movida: la posición nueva
  es `clamp_position(current + offset)` (función pura `drag_step(current, offset)` con test:
  `drag_step((100,100),(5,-3))` = `(105, 97)`). En `drag-end`, guardar `camera-position` y el
  conector del monitor. **Riesgo:** puede temblar si el compositor aplica los márgenes con
  atraso; si en la etapa 8 tiembla, cambiar a mover sólo en `drag-end` con un marco fantasma.
- **Tamaño**: `EventControllerScroll` vertical → `resized`, guardar `camera-size`, y
  `relayout()`.
- **Forma**: `GestureClick` con botón 3 → `toggled`, guardar `camera-shape`, y `relayout()`.
- **`relayout()`** (codex r1 #7): alto efectivo = `fitted_size(shape, camera-size, monitor)`,
  posición = `clamp_position(posición actual, tamaño efectivo, monitor)`, márgenes con
  `overlay::set_margins`, `queue_resize` del `PipView`. Se llama al prender, después de cada
  cambio de tamaño, forma o monitor, y cuando cambia la geometría del monitor
  (`connect_geometry_notify`). Si la posición cambió, se guarda.
- **Espejo** (P13; codex r1 #5 y r2 #5). El espejo lo hace el `PipView` al dibujar
  (`snapshot.translate` + `snapshot.scale(-1.0, 1.0)` cuando `mirrored` es `true`), no un
  `videoflip` en el pipeline de la cámara. Así cambiarlo no espera ningún cuadro de la cámara:
  basta con que GTK vuelva a dibujar la ventana.
  - `should_mirror(recording_active: bool) -> bool`.
  - `CameraPip::prepare_for_recording(&self) -> impl Future<Output = anyhow::Result<()>>`:
    pone `mirrored = false`, llama `pip_view.queue_draw()` y espera una **barrera de
    presentación** (codex r3 #1 y r4 #1): anota el `frame_counter()` del `gdk::FrameClock` de
    la ventana del PiP en el momento del cambio; en cada `after-paint` pendiente vuelve a llamar
    `pip_view.queue_draw()` **y** `frame_clock.request_phase(gdk::FrameClockPhase::PAINT)` (son
    cosas distintas: una invalida el dibujo, la otra hace correr el reloj; Gtk-4.0.gir línea
    179740, Gdk-4.0.gir línea 11549); y resuelve **sólo** cuando `frame_clock.timings(n)` de un
    cuadro `n` posterior al cambio tiene `is_complete()` y `presentation_time() > 0`, es decir,
    cuando el compositor confirmó que lo presentó (`FrameTimings::presentation_time`, gdk4
    0.11.2, `src/auto/frame_timings.rs` línea 44). Un cuadro completo con
    `presentation_time() == 0` es presentación desconocida (Gdk-4.0.gir línea 12060) y la
    barrera sigue en `Pending`.
  - La decisión vive en una máquina de estados pura, `PresentationBarrier`, que recibe
    `(frame_counter, complete, presentation_time)` y contesta `Pending` o `Done`. **Tests
    primero:** cuadros anteriores al cambio no la resuelven; un cuadro posterior completo pero
    con `presentation_time` 0 tampoco, ni aunque lleguen varios seguidos; uno presentado sí;
    un cuadro presentado que llega tarde (después de varios `Pending`) también.
  - **Tope de 1 s.** Si vence con el PiP mapeado, `prepare_for_recording` devuelve `Err`, y el
    gancho lo propaga: `start_inner` falla con el contexto «The camera overlay did not refresh
    in time», la grabación no arranca y se muestra el diálogo de error. Es la alternativa que
    pide la ronda 3: nunca se captura un cuadro sin haber confirmado el desespejado. Si el PiP
    no está mapeado, devuelve `Ok` enseguida, porque no hay nada en pantalla.
  - **Cuándo se llama:** dentro de `Recording::start_inner`, **después** del portal y del
    selector de área y **antes** del `Timer` de la cuenta regresiva (o de poner el pipeline en
    `PLAYING` si no hay demora). Mientras espera el portal, el estado sigue en `Init` y el PiP
    sigue espejado, como pide la spec. `Recording` recibe el gancho con
    `Recording::set_before_capture(f: Box<dyn Fn() -> LocalBoxFuture<'static, anyhow::Result<()>>>)`, que
    `Window::toggle_record` fija **después** de guardar la grabación en `imp.recording` (un
    segundo Ctrl+R en ese rato ve la grabación y la detiene, como hoy).
  - `CameraPip::set_recording_active(bool)`: lo llama la ventana con `false` en
    `handle_recording_finished` (vuelve a espejar) y con `true` desde el gancho. El `CameraPip`
    nunca le pregunta a la ventana (fable r2 #1).
  - Sin PiP prendido, `prepare_for_recording` vuelve enseguida.
  - **Test** `#[gtk::test] mirror_flag_flips_drawing`: un `PipView` con un paintable de prueba
    de un píxel rojo a la izquierda y uno azul a la derecha; con `mirrored = true`, el
    `gtk::Snapshot` resultante dibuja el rojo a la derecha (se compara el nodo con
    `snapshot.to_node()` renderizado con `gsk::CairoRenderer` a una textura y leído).
  - La prueba del primer cuadro sin cuenta regresiva va en la etapa 8: grabar con demora 0 y
    mirar el primer cuadro del archivo.
- **Fuente que desaparece**: un error en el bus del pipeline de la cámara apaga el PiP
  (`camera-enabled` = `false`) y notifica «Camera disconnected».
- **Reaccionar a Preferencias**: `connect_camera_enabled_changed`,
  `connect_camera_device_changed` (reinicia el pipeline), `connect_camera_shape_changed` y
  `connect_camera_size_changed` (redibujan).

- [ ] Las claves nuevas del schema: `camera-enabled` (`b`, `false`), `camera-device` (`s`,
  `""`), `camera-shape` (`s` con `<choices>` `circle` y `rounded-rectangle`, `"circle"`),
  `camera-size` (`u`, `200`), `camera-position` (`(ii)`, `(-1, -1)`), `camera-monitor` (`s`,
  `""`). Si `gsettings-macro` no genera getters para `(ii)`, saltearla con
  `#[gen_settings_skip]` y escribir `camera_position()`/`set_camera_position()` a mano como
  `framerate`.
- [ ] Test de `drag_step`. El resto de esta tarea se verifica en la etapa 8 (requiere un
  compositor con capas).
- [ ] `Application` crea el `CameraPip` en `startup` y lo guarda en un `OnceCell`. En
  `startup` no hay ventana: el `CameraPip` arranca sin consultar a nadie, espejado, y se entera
  del monitor de la ventana y de las grabaciones por los setters que llama `Window`.
- [ ] **Cambiar de monitor** (fable r2 #3): una capa no se puede arrastrar de un monitor a otro.
  Preferencias suma la fila «Camera Monitor» (un `AdwComboRow` con el nombre y el conector de
  cada monitor) ligada a `camera-monitor`; al cambiar, el `CameraPip` recrea la ventana en ese
  monitor y hace `relayout()`. La etapa 8 prueba el PiP en los tres monitores de Sebastián.

### Tarea 4.5 · Controles en la ventana, Preferencias y bandeja

- [ ] **Ventana** (`window.ui`, `src/window/mod.rs`): junto a los tres `ToggleButton` de la
  página principal, un `GtkMenuButton id="camera_button"` con ícono `camera-web-symbolic` y un
  popover con un interruptor «Show Camera» (acción `win.camera-enabled`, de
  `settings.create_camera_enabled_action()`) y una sección de radio con las fuentes
  (acción con estado `win.camera-device`, string). La lista de fuentes se refresca al abrir el
  popover. Si `overlay::availability()` es `Err`, el botón queda deshabilitado con tooltip
  «Camera overlay unavailable».
- [ ] **Preferencias**: grupo «Camera & Overlays» con `AdwSwitchRow` «Show Camera»,
  `AdwComboRow` «Camera Source» y `AdwComboRow` «Camera Shape» (subtítulo «Right-click the
  camera to switch»). Primera fila del grupo: un `AdwActionRow` `overlays_warning_row` con la
  clase CSS `warning` y el ícono `dialog-warning-symbolic`, visible sólo sin capas: título
  «Overlays unavailable», subtítulo el
  motivo (`NotCompiled` → «Kooha was built without gtk4-layer-shell.»;
  `CompositorUnsupported` → «Your desktop does not support screen overlays.»), y un botón
  «Open Report». Con el aviso, las filas del grupo quedan `sensitive = false`. El botón «Open
  Report» se conecta en el chunk 6; hasta entonces queda oculto (P16 lo pide junto al reporte).
- [ ] **Bandeja**: `TrayModel.camera_enabled` y `camera_available`; `TrayCommand::ToggleCamera`.
  **Test primero:** la `Check("Camera")` aparece antes de «Show Window», habilitada aun
  grabando, y deshabilitada si `camera_available` es `false`.

### Tarea 4.6 · Traducciones, verificación y commit

- [ ] `es.po`: «Camera Monitor» → «Monitor de la cámara», «Show Camera» → «Mostrar cámara», «Camera» → «Cámara», «Camera Source» →
  «Fuente de la cámara», «Camera Shape» → «Forma de la cámara», «Circle» → «Círculo»,
  «Rounded Rectangle» → «Rectángulo redondeado», «Right-click the camera to switch» →
  «Clic derecho sobre la cámara para cambiarla», «Camera & Overlays» → «Cámara y capas»,
  «Overlays unavailable» → «Capas no disponibles», los dos motivos, «Open Report» → «Abrir
  reporte», «No camera found» → «No se encontró ninguna cámara», «Camera disconnected» →
  «La cámara se desconectó», «Camera overlay unavailable» → «La cámara en pantalla no está
  disponible».
- [ ] Suite, clippy (con y sin `--features layer-shell`), fmt, ninja, msgfmt. Aplastar y
  commitear.

---

## Chunk 5: Borde del área

Commit: `feat: draw a dashed border outside the recorded area`.

### Tarea 5.1 · Recorte puro, compartido

**Archivos:** `src/pipeline.rs`.

- [ ] **Paso 1: test que falla** en `pipeline.rs`: `compute_crop` con un `SelectAreaData` de
  ejemplo (selección `(110, 60, 400, 300)` sobre un `paintable_rect` `(10, 10, 960, 540)` y
  `stream_size` `(1920, 1080)`) devuelve `Crop { top: 100, left: 200, right: 920, bottom: 380 }`
  (escala 2 en los dos ejes; la selección menos el origen del paintable es `(100, 50, 400, 300)`,
  que escalada da `(200, 100, 800, 600)`). El test fija el comportamiento actual. Para armar la
  `Selection`, hacer `pub(crate)` el constructor privado `Selection::from_rect` de
  `src/area_selector/view_port.rs`.
- [ ] **Paso 2:** extraer de `make_videocrop` una función pura
  `pub fn compute_crop(data: &SelectAreaData) -> Crop` con
  `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Crop { pub top: i32, pub left: i32, pub right: i32, pub bottom: i32, pub frame: (i32, i32) }`,
  donde `frame` es el tamaño del cuadro **que entra a `videocrop`**: `(round_to_even(w), round_to_even(h))`
  de `data.stream_size`, porque `videoscale` lo lleva a dimensiones pares antes del recorte
  (`src/pipeline.rs`, línea 107; codex r1 #13). `make_videocrop` la usa; el comportamiento del
  recorte no cambia.
- [ ] **Paso 3:** test `odd_stream_size_uses_even_frame`: con `stream_size` `(1921, 1081)`,
  `frame` es `(1920, 1080)`: `round_to_even` es `number / 2 * 2` y redondea hacia abajo
  (`src/pipeline.rs` línea 417; codex r2 #3, fable r2 #5). El test afirma además que `frame` es
  igual a las dimensiones de las caps que `PipelineBuilder::build` le pone al `videoscale`
  (extraer esas caps a una función pura `scaled_caps_size(stream_size)` que usen los dos).

### Tarea 5.2 · Geometría del borde

**Archivos:** crear `src/area_border/mod.rs`, `src/area_border/geometry.rs`.

```rust
pub const LINE_WIDTH: f64 = 1.0;  // logical px (mock-feedback.md)
pub const GAP: f64 = 2.0;         // logical px between the crop and the line
pub const DASH: f64 = 4.0;
pub const GRAY: (f64, f64, f64) = (0xc8 as f64 / 255.0, 0xc8 as f64 / 255.0, 0xc8 as f64 / 255.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderGeometry {
    /// The cropped area in monitor-local logical coordinates.
    pub area: (f64, f64, f64, f64), // x, y, w, h
    /// Which sides have room outside the area: top, right, bottom, left.
    pub sides: [bool; 4],
}

/// Only for a stream that captures a whole monitor; `monitor` is that monitor's
/// logical geometry (x, y, w, h) from `gdk::Monitor::geometry()`.
pub fn border_geometry(crop: Crop, monitor: (i32, i32, i32, i32)) -> Option<BorderGeometry>;
```

La escala va **por eje** (fable r2 #6a): `sx = monitor.w / crop.frame.0`, `sy = monitor.h / crop.frame.1`.
El tamaño lógico sale de la geometría del monitor de GDK, no del `size()` del portal (fable r2
#6b): para un stream de monitor entero son el mismo rectángulo. Área local =
`(crop.left * sx, crop.top * sy, (frame.0 - left - right) * sx, (frame.1 - top - bottom) * sy)`. Un lado se dibuja sólo si entre el
área y el borde del monitor hay por lo menos `GAP + LINE_WIDTH`. Devuelve `None` si la escala
no es finita o el área queda vacía.

- [ ] **Tests primero:**
  - `scale_one_centered`: crop `{100,200,920,380}` con `frame` 1920×1080, monitor
    `(0,0,1920,1080)` → área `(200,100,800,600)`, cuatro lados.
  - `hidpi_scale_two`: `frame` 3840×2160 sobre un monitor de 1920×1080 → el área se divide por 2.
  - `second_monitor_offset`: monitor `(1920,0,1920,1080)` → el área es local al monitor, sin el
    desplazamiento.
  - Los monitores reales de Sebastián (`kscreen-doctor -o`, 17/11/2026): `edp_scale_1_75`,
    `frame` 2560×1600 sobre `eDP-1` 1463×915 en `(457,1080)`, escalas por eje distintas
    (0,57148 y 0,57188); y `dp3_scale_1_05`, `frame` 1680×1050 sobre `DP-3` 1600×1000 en
    `(1920,454)`.
  - `touching_left_edge_skips_left`: `left: 0` → `sides[3] == false` (criterio E2).
  - `fractional_scale`: cuadro 2880×1620 con tamaño lógico 1920×1080 (escala 1,5) y un recorte
    de `left` 300 → `x` del área = 200.
  - `odd_frame`: `crop.frame` 1920×1080 (de un stream de 1921×1081) sobre un monitor lógico
    de 1921×1081; el área no se corre más de un píxel lógico respecto del cálculo con números
    reales.
  - `line_stays_outside_crop`: para cualquier área, el rectángulo interior de la línea
    (`area` expandida en `GAP`) contiene el área entera (criterio E1 en la geometría).

### Tarea 5.3 · La capa del borde

**Archivos:** crear `src/area_border/view.rs`; modificar `src/recording.rs`,
`src/application.rs`.

- [ ] `AreaBorder`: ventana-capa con `overlay::init_overlay` en el monitor, anclada a los cuatro
  bordes (`overlay::anchor_all_edges`), con `decorated = false`, sin región de entrada (en `realize`,
  `surface.set_input_region(Some(&cairo::Region::create()))` — la firma en gdk4 0.11.2 recibe
  `Option<&cairo::Region>`, `src/auto/surface.rs` línea 252 —, así el mouse la atraviesa; se
  comprueba en la etapa 8 haciendo click a través del borde), fondo
  transparente por CSS (`window.area-border { background: none; }`). Un `gtk::DrawingArea` que,
  para cada lado habilitado, dibuja primero el segmento entero en gris `GRAY` y encima los
  guiones blancos (`set_dash(&[DASH, DASH], 0.0)`), con `line_width = LINE_WIDTH`, en el
  rectángulo `area` expandido en `GAP + LINE_WIDTH / 2`.
- [ ] `Recording`: después de `AreaSelector::select`, calcular el borde **sólo** si hay un
  solo stream y `stream.source_type() == Some(SourceType::MONITOR)` (fable r2 #6c: un stream
  de ventana no tiene borde). El monitor es el de `gdk::Display::default().monitors()` cuya
  geometría tiene como origen `stream.position()`; si `position()` es `None` y hay un solo
  monitor, ese; si no, no hay borde y se loguea. Si `stream.size()` existe y difiere del tamaño
  del monitor, se loguea con `tracing::warn!` y se usa el del monitor. Guardar el resultado y el
  monitor en `area_border: RefCell<Option<(BorderGeometry, gdk::Monitor)>>` con getter.
  La ventana-capa del borde se crea sin la propiedad `application`.
- [ ] `Window` (handler de estado): con `Delayed`, `Recording` o `Paused` y un borde calculado,
  mostrar el `AreaBorder` (crearlo una vez por grabación); con cualquier otro estado,
  destruirlo. Si `overlay::availability()` es `Err`, no se crea.
- [ ] Prueba visual en la etapa 8.

### Tarea 5.4 · Verificación y commit

- [ ] Sin textos nuevos visibles. Suite, clippy (con y sin feature), fmt, ninja. Aplastar y
  commitear.

---

## Chunk 6: Reporte de requisitos

Commit: `feat: add a requirements report`.

### Tarea 6.1 · Modelo y formato, puros

**Archivos:** crear `src/doctor/mod.rs`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status { Ok, Warning, Failed }

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub problem: String,
    pub impact: String,
    pub fix_command: Option<String>,
    pub fix_note: Option<String>,   // e.g. "Then rebuild Kooha."
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub title: String,
    pub status: Status,
    pub detail: Option<String>,     // shown under the title, e.g. "Version 5"
    pub failure: Option<Failure>,   // Some only when status != Ok
}

pub struct Report(pub Vec<CheckResult>);

impl Report {
    pub fn has_failures(&self) -> bool;          // Warning does not count
    pub fn to_text(&self) -> String;             // for Copy Report and --check
}
```

- [ ] **Tests primero:** `to_text` marca cada línea con `[OK]`, `[WARN]` o `[FAIL]`; escribe
  problema, impacto y comando **sólo** debajo de los `Warning` y `Failed`;
  `has_failures` es `false` con sólo `Warning`.

### Tarea 6.2 · Los ocho chequeos

**Archivos:** crear `src/doctor/checks.rs`.

Cada chequeo es una función que recibe lo que consulta, para poder testear la decisión sin el
sistema real:

| Función | Consulta real | Decisión testeable |
|---|---|---|
| `gstreamer_elements(find: impl Fn(&str) -> bool, profiles: &[ProfileElements])` | `gst::ElementFactory::find(name).is_some()`; `ProfileElements { id, name, experimental, elements: Profile::required_elements() }` | Cualquier elemento que falte → `Failed`, con **el nombre de cada elemento** y el paquete de la tabla de abajo (codex r1 #11). Los perfiles experimentales no cuentan, porque Kooha los oculta salvo con `KOOHA_EXPERIMENTAL`; si el perfil activo es experimental, sí cuenta |
| `portal(result: Result<(u32, CursorMode)>, desktop: &str)` | `screencast_portal::Proxy::new().await`, `version()`, `available_cursor_modes()` | `Err` → `Failed` con `sudo apt install xdg-desktop-portal xdg-desktop-portal-kde` (`-gnome` con `GNOME`, `-wlr` en otro caso); `Ok` → detalle «Version N · pointer: …» |
| `pipewire(socket_exists: bool, version: Option<String>)` | `$XDG_RUNTIME_DIR/pipewire-0`; versión como en `src/about.rs` | Sin socket → `Failed`, `systemctl --user enable --now pipewire pipewire-pulse wireplumber` |
| `audio(sink: bool, source: bool)` | `device::find_default(DeviceClass::Sink / Source)` | Falta uno → `Failed`; impacto: «Desktop audio cannot be recorded» o «The microphone cannot be recorded» |
| `cameras(count: usize)` | `camera_pip::devices::list().len()` | 0 → `Warning`, sin comando; impacto: el PiP |
| `overlays(av: Result<(), OverlayUnavailable>)` | `overlay::availability()` | `NotCompiled` → `Failed`, `sudo apt install libgtk4-layer-shell-dev` y nota «Then rebuild Kooha.» (también sin display: F1); `CompositorUnsupported` → `Failed` sin comando, problema «Your desktop does not support screen overlays (wlr-layer-shell)»; `NoDisplay` → `Warning` «Could not ask the desktop without a display» |
| `tray(watcher: bool, desktop: &str)` | `NameHasOwner("org.kde.StatusNotifierWatcher")` en el bus de sesión | `false` → `Failed`; en GNOME el comando es `sudo apt install gnome-shell-extension-appindicator` |
| `recordings_folder(path, exists: bool, writable: bool)` | `settings.saving_location_unchecked()`, un getter nuevo que devuelve la ruta guardada o la de por defecto **sin** `create_dir_all` (el `saving_location()` de hoy la crea, `src/settings.rs` líneas 55–76); escribir y borrar un archivo temporal sólo si existe | No existe → `Failed`, `mkdir -p -- <ruta escapada>`; no se puede escribir → `Failed`, `ls -ld -- <ruta escapada>` y nota de permisos. La ruta pasa por `shell_quote` (codex r1 #10) |

Paquetes por elemento (Ubuntu), verificados el 17/11/2026 con el registro de GStreamer y
`dpkg -S` sobre el archivo de cada plugin (fable r2 #8):

| Paquete | Elementos |
|---|---|
| `libgstreamer1.0-0` | `queue`, `capsfilter`, `identity`, `filesink` |
| `gstreamer1.0-pipewire` | `pipewiresrc` |
| `gstreamer1.0-plugins-base` | `videoconvert`, `videoscale`, `videorate`, `compositor`, `audioconvert`, `audioresample`, `audiomixer`, `audiorate`, `opusenc` |
| `gstreamer1.0-plugins-good` | `pulsesrc`, `videocrop`, `videobalance`, `videoflip`, `v4l2src`, `vp8enc`, `vp9enc`, `webmmux`, `matroskamux`, `mp4mux`, `lamemp3enc`, `mpegaudioparse` |
| `gstreamer1.0-plugins-ugly` | `x264enc` |
| `gstreamer1.0-plugins-bad` | `h264parse`, `av1enc` |
| `gstreamer1.0-plugins-extra` | `vapostproc`, `vah264enc` (los dos salen de `libgstva.so`) |
| Viene dentro de Kooha | `gifenc`, `gtk4paintablesink` (los registra `src/main.rs` de forma estática) |

Un elemento que no está en la tabla da el problema «Unknown package for <elemento>» sin
comando. **Test:** recorre `Profile::all()` (con `init_resources()`) y los elementos fijos del
pipeline, y falla si alguno cae fuera de la tabla.

- [ ] **`shell_quote(&str) -> String`**: envuelve en comillas simples y escribe cada `'`
  interna como `'\''`. **Tests primero**, sin ninguna carga destructiva (codex r2 #4): cada test
  crea un directorio temporal propio `T` y usa rutas con espacios, con `'`, con `"`, que
  empiezan con `-`, con `$(touch T/marker1)` y con `` `touch T/marker2` ``. Corre
  `sh -c "printf '%s\0' <ruta escapada>"` y afirma dos cosas: la salida es exactamente la ruta
  original seguida de un solo `\0` (un único argumento literal), y ni `T/marker1` ni
  `T/marker2` existen.
- [ ] **`Profile::required_elements(&self) -> Vec<String>`** en `src/profile.rs`: los nombres de
  fábrica de cada segmento de `videoenc_bin_str`, `audioenc_bin_str` y `muxer_bin_str` (primer
  token de cada parte separada por `!`, sin las caps ni las propiedades). Test con el perfil
  `webm-vp8` de `data/resources/profiles.yml`: incluye `vp8enc` y `webmmux`.
- [ ] **Tests primero**, uno por fila de la tabla: el caso que falla, con su comando exacto, y
  el caso `Ok`.
- [ ] `pub async fn run(settings: &Settings, overlays: Result<(), OverlayUnavailable>) -> Report`
  junta los ocho en el orden de la spec. `overlays` llega como argumento: la ventana pasa
  `overlay::availability()`, y `--check` también, que sin display devuelve `NotCompiled` o
  `NoDisplay` según el feature.

### Tarea 6.3 · Diálogo

**Archivos:** crear `src/doctor/dialog.rs`.

- [ ] `RequirementsDialog` (`adw::Dialog`, título «Requirements Report»): un
  `gtk::ListBox` `boxed-list`; cada chequeo es una fila con ícono a la izquierda
  (`object-select-symbolic` en verde `@success_color` para `Ok`, `window-close-symbolic` en
  rojo `@error_color` para `Failed`, `dialog-warning-symbolic` en `@warning_color` para
  `Warning`), título y detalle. **Sólo** las filas `Failed` y `Warning` llevan debajo los tres
  bloques «Problem», «What stops working» y «How to fix it», este con el comando en monoespaciada
  y un botón de copiar (mismo patrón que `present_recording_error_dialog`). Pie: botón
  «Copy Report» que copia `report.to_text()`.
- [ ] Mientras corre `run`, un `adw::Spinner` en el lugar de la lista.

### Tarea 6.4 · Puntos de entrada

- [ ] **Menú principal** (`window.ui`, `primary_menu`): ítem «_Requirements Report» con
  `app.show-requirements-report`, después de «_Preferences». La acción en
  `Application::setup_gactions`.
- [ ] **Error de grabación** (`present_recording_error_dialog`): respuesta extra
  «Open Requirements Report» que abre el diálogo.
- [ ] **Preferencias**: mostrar el botón «Open Report» del aviso de capas (chunk 4) y
  conectarlo a la misma acción.
- [ ] **`kooha --check`** (`src/main.rs`): antes de `Application::new()`, si
  `std::env::args().any(|a| a == "--check")`: `gst::init()`, intentar `gtk::init()` (si falla,
  no pasa nada: `overlay::availability()` contesta `NotCompiled` o `NoDisplay`), correr
  `doctor::run` con
  `glib::MainContext::default().block_on`, imprimir `to_text()` por `stdout` y salir con 0 o 1
  según `has_failures()`. Agregar `#[allow(clippy::print_stdout)]` local, porque `main.rs`
  lo prohíbe a nivel de crate. Prueba (criterio F2), con el ejecutor aislado y sin display
  (codex r3 #2, fable r3 #2):
  `env -u DISPLAY WAYLAND_DISPLAY=kooha-no-display /home/sebas/robotin/apps/kooha/.workspace/t-1088-first-improvements/run-dev.sh <worktree> --check`
  (sin `WAYLAND_DISPLAY`, libwayland prueba `wayland-0`, que es la sesión de Sebastián; con un
  nombre de socket inexistente GDK no consigue display; fable r4 #1)
  imprime el reporte, no abre ventanas, y el chequeo «Screen overlays» sale `[FAIL]` compilado
  sin el feature y `[WARN]` con el feature. Las dos variantes se compilan y se prueban.
- [ ] La ayuda de `--check` en `--help`: si `GApplication` rechaza la opción desconocida,
  registrarla con `app.add_main_option` además del atajo anterior.

### Tarea 6.5 · Traducciones, verificación y commit

- [ ] `es.po`: títulos de los ocho chequeos, «Problem» → «Problema», «What stops working» →
  «Qué deja de funcionar», «How to fix it» → «Cómo arreglarlo», «Copy Report» → «Copiar
  reporte», «Requirements Report» → «Reporte de requisitos», «Open Requirements Report» →
  «Abrir el reporte de requisitos», y cada problema e impacto.
- [ ] Suite, clippy (con y sin feature), fmt, ninja, msgfmt. Aplastar y commitear.

---

## Etapa 8 · Prueba en la sesión real

La captura de pantalla pasa por el portal de KDE y las capas necesitan KWin, así que la prueba
de punta a punta no se puede hacer en un Xvfb. Se hace en la sesión de Sebastián con el
ejecutor de desarrollo (`run-dev.sh`), que usa los recursos, el schema y un archivo de
preferencias propios del worktree, así que no pisa la configuración, la selección ni el token
del portal del Kooha instalado (codex r1 #1 y #3). Recorrido: los criterios A1 a F2 de la spec, con
capturas de pantalla de cada mejora y un video corto grabado con pausa y PiP. Casos que se
prueban además a propósito: el PiP en cada uno de los tres monitores; la primera vez con
Kooha en `eDP-1`, `camera-monitor` vacío y la cámara prendida desde el arranque; una
grabación con demora 0 mirando el primer cuadro del archivo; otra con la cámara tapada o
trabada; y el borde en `eDP-1` (escala 1,75).

## Riesgos

| Riesgo | Mitigación |
|---|---|
| El arrastre del PiP tiembla porque el `offset` se mide contra una superficie que se mueve | `drag_step` encapsulado; plan B en la tarea 4.4 |
| `videobalance` no acepta el formato que entrega `pipewiresrc` | El test 3.3 usa `BGRx`, el formato habitual; si falla en la sesión real, agregar `videoconvert` antes del `videobalance` |
| `gsettings-macro` no genera `(ii)` | Saltear la clave y escribir los métodos a mano (tarea 4.4) |
| El portal de KDE no informa `position()` del stream | Con un solo monitor se usa ese; con varios, no hay borde y se loguea (Sebastián tiene tres: la etapa 8 lo comprueba) |
| La captura toma un cuadro del PiP todavía espejado | El espejo se hace al dibujar, y `prepare_for_recording` espera la presentación confirmada antes de la cuenta regresiva; si no llega en 1 s, la grabación no arranca (tarea 4.4) |
| Un compositor sin el protocolo de tiempos de presentación nunca confirma el cuadro | Con el PiP prendido, la grabación no arranca y el error lo explica. KWin lo informa; la etapa 8 lo comprueba. Si fallara, se decide con Sebastián |
