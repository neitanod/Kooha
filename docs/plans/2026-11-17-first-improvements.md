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
| P16 | Ocho chequeos | Chunk 6, tarea 6.2 |
| P17 | Checklist con detalle sólo en las fallas, copiar, `--check`, botón en el error | Chunk 6 |
| P18 | gtk4-layer-shell opcional; aviso en Preferencias | Chunk 4, tareas 4.0 y 4.5 |
| P20 | Inglés en el código y traducción en `po/es.po` | Todos los chunks, último paso |
| P21 | Fork limpio: un commit por mejora, estilo de Kooha | Todos los chunks |
| P22 | Todo en esta Task, en orden, commits encadenados | Grafo de chunks |

---

## Comandos de verificación

Todos desde el repo de trabajo. `CARGO_HOME` y `--target-dir` reusan la caché del build de
Meson.

```bash
export CARGO_HOME=$PWD/_build/cargo-home
cargo test --target-dir _build/src                         # suite
cargo clippy --target-dir _build/src -- -D warnings        # linter
cargo fmt --check                                          # formato
ninja -C _build                                            # build completo con Meson
```

Estado de partida (`d5e68e3`): 42 tests pasan; `settings::tests::default_profile` aborta porque
busca el schema en `data/`, donde sólo está el `.gschema.xml.in`. Lo arregla la tarea 1.1.

**Si Meson no está configurado en el worktree**:
`meson setup _build --prefix=/usr/local -Dprofile=development`.

## Grafo de chunks

P22 pide los commits encadenados en el orden de las mejoras, y casi todos los chunks tocan los
mismos archivos (`application.rs`, `window/mod.rs`, `window.ui`, el schema, `po/es.po`). Por eso
el grafo es una cadena: una tanda por chunk, un chunk por tanda.

| Chunk | Mejora | Depende de | Tanda | Archivos nuevos | Archivos que modifica |
|---|---|---|---|---|---|
| 1 | Avisos de captura | — | 1 | `src/capture_warnings.rs` | `src/settings.rs` (test), `src/recording.rs`, `src/window/mod.rs`, `data/resources/ui/window.ui`, `src/application.rs`, `src/main.rs`, `po/POTFILES.in`, `po/es.po` |
| 2 | Ícono de bandeja | 1 | 2 | `src/tray/mod.rs`, `src/tray/model.rs`, `src/tray/service.rs` | `Cargo.toml`, `Cargo.lock`, `src/application.rs`, `src/window/mod.rs`, `src/main.rs`, `po/*` |
| 3 | Pausa | 2 | 3 | `src/pause_gate.rs` | `src/pipeline.rs`, `src/recording.rs`, `src/window/mod.rs`, `data/resources/ui/window.ui`, `src/application.rs`, schema, `src/preferences_dialog.rs`, `data/resources/ui/preferences_dialog.ui`, `src/tray/*`, `po/*` |
| 4 | PiP de cámara | 3 | 4 | `src/overlay.rs`, `src/camera_pip/{mod,geometry,devices,view}.rs` | `Cargo.toml`, `meson.build`, `meson_options.txt`, `src/meson.build`, schema, `src/application.rs`, `src/window/mod.rs`, `window.ui`, `preferences_dialog.*`, `src/tray/*`, `po/*` |
| 5 | Borde del área | 4 | 5 | `src/area_border/{mod,geometry,view}.rs` | `src/pipeline.rs`, `src/recording.rs`, `src/application.rs`, `po/*` |
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

- [ ] **Paso 1:** correr `cargo test --target-dir _build/src settings::tests::default_profile`.
  Esperado: aborta con `Settings schema 'io.github.seadve.Kooha.Devel' is not installed`.
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
- [ ] **Paso 3:** `cargo test --target-dir _build/src`. Esperado: 43 tests pasan.

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

### Tarea 1.3 · La grabación conoce sus avisos

**Archivos:** modificar `src/recording.rs`.

- [ ] **Paso 1:** agregar al `imp::Recording` una propiedad de sólo lectura
  `#[property(get)] pub(super) capture_warnings: Cell<CaptureWarnings>` (con `Cell` funciona
  porque `CaptureWarnings` es `Copy`).
- [ ] **Paso 2:** en `start_inner`, después de obtener `profile` y **antes** de abrir la sesión
  del portal, calcular y guardar:

```rust
let warnings = CaptureWarnings::compute(
    profile.supports_audio(),
    settings.record_desktop_audio(),
    settings.record_microphone(),
    settings.show_pointer(),
);
imp.capture_warnings.set(warnings);
self.notify_capture_warnings();
```

  Así los avisos ya están en `Delayed` (cuenta regresiva).
- [ ] **Paso 3:** `cargo build --target-dir _build/src` compila.

### Tarea 1.4 · Carteles en la ventana

**Archivos:** modificar `data/resources/ui/window.ui` y `src/window/mod.rs`.

- [ ] **Paso 1:** en `window.ui`, dentro de `recording_page`, debajo del `GtkBox` del
  cronómetro, agregar `GtkFlowBox id="recording_warnings_box"` (`selection-mode` none,
  `halign` center, `max-children-per-line` 2, `visible` false). Lo mismo en `delay_page`,
  debajo de `delay_label`: `GtkFlowBox id="delay_warnings_box"`.
- [ ] **Paso 2:** en `src/window/mod.rs`, los `TemplateChild` de las dos cajas, y una función

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
        for label in warnings.labels() {
            let chip = gtk::Label::builder()
                .label(format!("⚠ {label}"))
                .css_classes(["capture-warning"])
                .build();
            flow_box.append(&chip);
        }
        flow_box.set_visible(!warnings.is_empty());
    }
}
```

  Llamarla desde `update_view` y desde un `recording.connect_capture_warnings_notify` en
  `toggle_record` (agregar el handler al vector `handler_ids`).
- [ ] **Paso 3:** en `data/resources/style.css` (existe, lo carga la app), la clase
  `.capture-warning`: fondo `alpha(@warning_color, 0.15)`, color `@warning_fg_color`, padding
  `2px 10px`, `border-radius: 999px`, `font-weight: bold`.
- [ ] **Paso 4:** `ninja -C _build` compila.

### Tarea 1.5 · Notificación al arrancar

**Archivos:** modificar `src/application.rs` y `src/window/mod.rs`.

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
- [ ] **Paso 2:** en `Window`, un `Cell<bool>` `warnings_notified` que se pone en `false` en
  `toggle_record` al crear la grabación. En el handler de `connect_state_notify`, cuando el
  estado pasa a `RecordingState::Recording` y `warnings_notified` es `false`, mandar la
  notificación y ponerlo en `true`.
- [ ] **Paso 3:** prueba manual en la tarea 1.6.

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
pub enum TrayRecordingState { Idle, Delayed, Recording, Paused, Flushing }

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
  - `delayed_and_flushing_disable_toggles`: con `Delayed` y `Flushing` las `Check` también están
    deshabilitadas, y en `Flushing` el botón de grabar está deshabilitado.
  - `audio_checks_disabled_without_audio_support`: con `audio_supported: false` las dos `Check`
    de audio quedan deshabilitadas aun en `Idle` (igual que `update_audio_actions` de la ventana).
  - `icon_follows_state`: `Idle` → `crate::config::APP_ID` (el ícono de la app, que en
    desarrollo es el `.Devel`), `Recording` →
    `"media-record-symbolic"`, `Paused` → `"media-playback-pause-symbolic"`.
  - `tooltip_lists_warnings`: el cuerpo del tooltip contiene cada aviso.
- [ ] **Paso 2:** fallan por no compilar.
- [ ] **Paso 3:** implementar `menu_entries`, `icon_name` y `tooltip`. Textos con `gettext`:
  «Ready», «Recording — {time}», «Paused — {time}», «Starting…» (para `Delayed`),
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
- [ ] **Paso 2–4:** falla, implementar, pasa.

### Tarea 2.4 · Cableado en la aplicación

**Archivos:** modificar `src/application.rs`, `src/window/mod.rs`, `src/main.rs` (`mod tray;`).

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

  Los tres toggles revisan antes que la ventana no esté ocupada (`window.is_busy()` o estado
  `Delayed`): el menú ya los deshabilita, y este chequeo cubre un click que llegó tarde.
- [ ] **Paso 2:** `Application::update_tray(&self)`: arma el `TrayModel` con
  `self.window()` (estado y duración de la grabación actual, avisos) y `self.settings()`, y
  llama `tray.update`. Para eso, en `Window`, agregar
  `pub fn recording_snapshot(&self) -> Option<(RecordingState, gst::ClockTime, CaptureWarnings)>`.
- [ ] **Paso 3:** llamar `update_tray` desde: `Window::update_view` (cambios de estado),
  el handler de `connect_duration_notify` (sólo si cambia el segundo entero, para no
  actualizar D-Bus cinco veces por segundo), `handle_recording_finished`, y los
  `connect_*_changed` de `record-desktop-audio`, `record-microphone`, `show-pointer` y
  `profile-id` en `Application::startup`.
- [ ] **Paso 4:** verificación manual con `dbus-run-session` (sin tocar la sesión de Sebastián):
  `dbus-run-session -- sh -c 'kooha & sleep 3; busctl --user list | grep StatusNotifier'`
  muestra que Kooha arranca aunque no haya watcher (B3). La prueba de punta a punta del menú se
  hace en la etapa 8.

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

El probe: `pad.add_probe(gst::PadProbeType::BUFFER, …)`. Con `info.buffer_mut()` y
`make_mut()`, si el PTS mapea a `Drop` devuelve `gst::PadProbeReturn::Drop`; si mapea a
`Keep(t)`, pone el PTS en `t` y corre el DTS (si tiene) en la misma cantidad. Un buffer sin PTS
pasa sin cambios.

**Dónde va** (en `PipelineBuilder::build`, `src/pipeline.rs`): un elemento `identity` llamado
`kooha-video-gate` entre la salida de video (`videocrop` o `videosrc_bin`) y
`kooha-videoenc-queue`; otro `identity` `kooha-audio-gate` entre `audiosrc_bin` y
`kooha-audioenc-queue`. El probe va en el **pad `src`** de cada `identity`. `PipelineBuilder`
recibe el `PauseGate` con un método `pause_gate(&mut self, gate: PauseGate)` y lo engancha.

- [ ] **Paso 1: test de integración que falla**, en `src/pause_gate.rs`, con un pipeline real:
  `videotestsrc is-live=true ! video/x-raw,framerate=30/1 ! identity name=gate ! appsink name=sink sync=false`.
  El test (sin GTK, con `gst::init()`) arranca el pipeline, espera 300 ms, `gate.pause(rt)`,
  espera 300 ms, `gate.resume(rt)`, espera 300 ms, manda EOS y junta los PTS del `appsink`.
  Afirma:
  - los PTS son crecientes;
  - ningún salto entre PTS consecutivos supera dos cuadros (66 ms) — criterio C2;
  - el último PTS es menor que 700 ms (sin la pausa, sería mayor que 850 ms).
  El tiempo de pared del test se mide y **no** se afirma: sólo se afirman los PTS.
- [ ] **Paso 2:** lo mismo con `audiotestsrc is-live=true` (criterio C2 para el audio).
- [ ] **Paso 3–4:** falla, implementar, pasa.

### Tarea 3.3 · Destello de reanudación

**Archivos:** `data/io.github.seadve.Kooha.gschema.xml.in`, `src/pause_gate.rs`,
`src/pipeline.rs`.

- [ ] **Paso 1:** clave `<key type="b" name="pause-flash"><default>false</default></key>`.
  `gsettings-macro` genera `pause_flash()`, `set_pause_flash()` y `bind_pause_flash()`.
- [ ] **Paso 2: test que falla:** pipeline real
  `videotestsrc is-live=true pattern=black ! video/x-raw,format=BGRx,framerate=30/1 ! identity name=gate ! videobalance name=flash ! appsink`;
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

- [ ] **Paso 1:** `imp::Recording` guarda un `PauseGate`. `start_inner` se lo pasa al
  `PipelineBuilder` y, si `settings.pause_flash()`, activa el destello.
- [ ] **Paso 2:** `pause()`: exige estado `Recording`, llama `gate.pause(pipeline.current_running_time())`
  y `self.set_state(RecordingState::Paused)`. `resume()`: exige `Paused`,
  `gate.resume(...)`, `set_state(RecordingState::Recording)`. El pipeline no cambia de estado.
  Si `current_running_time()` es `None`, devolver error con contexto.
- [ ] **Paso 3:** en `handle_bus_message`, `StateChanged` del pipeline: sacar el mapeo
  `gst::State::Paused => RecordingState::Paused`; sólo `Playing` → `Recording`, y sólo si el
  estado actual no es `Paused`.
- [ ] **Paso 4:** `update_duration`: durante `Recording` y `Paused`, la duración es
  `gate.recorded_duration(pipeline.current_running_time())`; en `Flushing` sigue usando
  `query_position` como hoy. `stop()`: `estimated_final_duration` pasa a ser
  `gate.recorded_duration(current_running_time)`, así el progreso del guardado no cuenta la
  pausa. Borrar el `// TODO Handle effect on estimated final duration`.
- [ ] **Paso 5:** `stop()` en `Paused` funciona: el EOS fluye porque el pipeline sigue en
  `PLAYING` y la compuerta deja pasar los eventos (el probe es sólo de buffers).

### Tarea 3.5 · Botón, atajo, bandeja y Preferencias

**Archivos:** `data/resources/ui/window.ui`, `src/application.rs`,
`data/resources/ui/preferences_dialog.ui`, `src/preferences_dialog.rs`, `src/tray/model.rs`,
`src/application.rs`.

- [ ] En `window.ui`, `pause_record_button`: sacar `<property name="visible">False</property>`.
- [ ] En `application.rs`, descomentar `set_accels_for_action("win.toggle-pause", &["<Control>k"])`
  y borrar el comentario del issue 112. Agregar el atajo a `data/resources/ui/shortcuts.ui`.
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
- [ ] Correrlo y pegar la salida en el commit (cuerpo del mensaje) o en el informe del chunk.

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
pub enum OverlayUnavailable { NotCompiled, CompositorUnsupported }

pub fn availability() -> Result<(), OverlayUnavailable>;

/// Turns `window` into an overlay layer surface on `monitor`.
/// Only callable when `availability()` is Ok.
pub fn init_overlay(window: &gtk::Window, monitor: &gdk::Monitor, namespace: &str);
pub fn set_margins(window: &gtk::Window, left: i32, top: i32);
pub fn anchor_all_edges(window: &gtk::Window);
```

  Con el feature apagado, `availability()` devuelve `Err(NotCompiled)` y las otras funciones no
  existen (`#[cfg(feature = "layer-shell")]`); el código que las usa también va con `cfg`. Con
  el feature, `availability()` llama `gtk4_layer_shell::is_supported()` (requiere GTK
  inicializado). `init_overlay`: `init_layer_shell`, `set_layer(Layer::Overlay)`,
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
    pub fn from_setting(s: &str) -> Self;      // unknown -> Circle
    pub fn as_setting(self) -> &'static str;   // "circle" | "rounded-rectangle"
    /// Width for a given height; the rectangle is 16:9.
    pub fn width_for(self, height: i32) -> i32;
    pub fn corner_radius(self, height: i32) -> f32; // circle: height/2, rect: height*0.12
}

/// Scroll up grows 10 %, scroll down shrinks 10 %, clamped.
pub fn resized(size: i32, scroll_dy: f64) -> i32;
/// Clamps a top-left position so the whole PiP stays inside the monitor.
pub fn clamp_position(pos: (i32, i32), pip: (i32, i32), monitor: (i32, i32)) -> (i32, i32);
/// Bottom-right with EDGE_MARGIN.
pub fn default_position(pip: (i32, i32), monitor: (i32, i32)) -> (i32, i32);
/// P13: mirrored only while no recording session is running.
pub fn should_mirror(state: Option<RecordingState>) -> bool;
```

- [ ] **Tests primero:**
  - `shape_toggles_both_ways`, `unknown_shape_setting_is_circle`, `circle_is_square`,
    `rectangle_is_16_9` (`width_for(180)` = 320).
  - `scroll_up_grows_ten_percent` (`resized(200, -1.0)` = 220), `scroll_down_shrinks`
    (`resized(200, 1.0)` = 182 redondeando), `resize_clamps` (96 y 640 en los extremos).
  - `clamp_keeps_inside` (posición negativa → 0; pasada del borde → `monitor - pip`),
    `default_is_bottom_right` (`default_position((200,200),(1920,1080))` = `(1696, 856)`).
  - `mirror_only_outside_recording`: `None`, `Init` y `Finished` → `true`; `Delayed`,
    `Recording`, `Paused` y `Flushing` → `false`.
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
`stop()` — mismo patrón que `src/device.rs`.

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

- [ ] Test (con `gtk::init()`; si no hay display, `return` temprano y loguear): `measure`
  devuelve `(200, 200)` con círculo y alto 200, y `(356, 200)` con rectángulo.
- [ ] El recorte *cover* sale de una función pura
  `cover_rect(paintable: (f64, f64), target: (f64, f64)) -> graphene::Rect`, con test.

### Tarea 4.4 · Controlador del PiP

**Archivos:** `src/camera_pip/mod.rs`.

`CameraPip` (objeto GLib) que dueño de: la ventana-capa, el `PipView`, y un pipeline
`<fuente> ! videoconvert ! videoflip name=mirror ! gtk4paintablesink`.

- **Prender** (`camera-enabled` pasa a `true`, o al arrancar la app con la clave en `true`):
  si `overlay::availability()` es `Err`, no hace nada. Si no hay fuentes, pone
  `camera-enabled` en `false` y manda la notificación «No camera found». Si hay: arma el
  pipeline con `devices::choose(list(), settings.camera_device())`, lo pone en `PLAYING`, crea
  la ventana con `overlay::init_overlay` en el monitor guardado (`camera-monitor`, buscado por
  `gdk::Monitor::connector()`; si no está, el primero de `display.monitors()`), posición
  `camera-position` (si es `(-1,-1)`, `default_position`), `clamp_position`, y `present()`.
- **Apagar**: pipeline a `NULL`, `window.destroy()`.
- **Mover**: `GestureDrag` con botón 1. En cada `drag-update`, como la superficie se mueve con el
  puntero, el `offset` que reporta GTK es relativo a la superficie ya movida: la posición nueva
  es `clamp_position(current + offset)` (función pura `drag_step(current, offset)` con test:
  `drag_step((100,100),(5,-3))` = `(105, 97)`). En `drag-end`, guardar `camera-position` y el
  conector del monitor. **Riesgo:** puede temblar si el compositor aplica los márgenes con
  atraso; si en la etapa 8 tiembla, cambiar a mover sólo en `drag-end` con un marco fantasma.
- **Tamaño**: `EventControllerScroll` vertical → `resized`, guardar `camera-size`, redibujar.
- **Forma**: `GestureClick` con botón 3 → `toggled`, guardar `camera-shape`.
- **Espejo**: `CameraPip::set_recording_state(Option<RecordingState>)` pone
  `video-direction` del `videoflip` en `horiz` si `should_mirror`, si no en `identity`.
  `Window` lo llama desde el handler de estado **y** desde `toggle_record` justo antes de
  `recording.start(...)`, para que quede sin espejar antes de que el pipeline de grabación
  arranque aunque no haya cuenta regresiva (P13). `handle_recording_finished` lo vuelve a
  espejado.
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
- [ ] `Application` crea el `CameraPip` en `startup` y lo guarda en un `OnceCell`.

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
  Report» se conecta en el chunk 6; hasta entonces queda oculto (P18 lo pide junto al reporte).
- [ ] **Bandeja**: `TrayModel.camera_enabled` y `camera_available`; `TrayCommand::ToggleCamera`.
  **Test primero:** la `Check("Camera")` aparece antes de «Show Window», habilitada aun
  grabando, y deshabilitada si `camera_available` es `false`.

### Tarea 4.6 · Traducciones, verificación y commit

- [ ] `es.po`: «Show Camera» → «Mostrar cámara», «Camera» → «Cámara», «Camera Source» →
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
  `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Crop { pub top: i32, pub left: i32, pub right: i32, pub bottom: i32 }`,
  y hacer que `make_videocrop` la use. El comportamiento del recorte no cambia.

### Tarea 5.2 · Geometría del borde

**Archivos:** crear `src/area_border/mod.rs`, `src/area_border/geometry.rs`.

```rust
pub const LINE_WIDTH: f64 = 2.0;  // logical px
pub const GAP: f64 = 2.0;         // logical px between the crop and the line
pub const DASH: f64 = 6.0;
pub const GRAY: (f64, f64, f64) = (0xc8 as f64 / 255.0, 0xc8 as f64 / 255.0, 0xc8 as f64 / 255.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderGeometry {
    /// The cropped area in monitor-local logical coordinates.
    pub area: (f64, f64, f64, f64), // x, y, w, h
    /// Which sides have room outside the area: top, right, bottom, left.
    pub sides: [bool; 4],
}

/// `stream_pixels` is the stream frame size; `stream_logical` is the stream's
/// portal position and size in compositor logical coordinates; `monitor` is the
/// monitor's logical geometry (x, y, w, h).
pub fn border_geometry(
    crop: Crop,
    stream_pixels: (i32, i32),
    stream_logical: ((i32, i32), (i32, i32)),
    monitor: (i32, i32, i32, i32),
) -> Option<BorderGeometry>;
```

Escala = `stream_logical.size / stream_pixels`. Área lógica en el monitor =
`stream_logical.position + crop * escala - monitor.origin`. Un lado se dibuja sólo si entre el
área y el borde del monitor hay por lo menos `GAP + LINE_WIDTH`. Devuelve `None` si la escala
no es finita o el área queda vacía.

- [ ] **Tests primero:**
  - `scale_one_centered`: crop `{100,200,920,380}`, stream 1920×1080, lógico `((0,0),(1920,1080))`,
    monitor `(0,0,1920,1080)` → área `(200,100,800,600)`, cuatro lados.
  - `hidpi_scale_two`: stream 3840×2160 con tamaño lógico 1920×1080 → el área se divide por 2.
  - `second_monitor_offset`: stream en `(1920,0)`, monitor `(1920,0,1920,1080)` → área local sin
    el desplazamiento.
  - `touching_left_edge_skips_left`: `left: 0` → `sides[3] == false` (criterio E2).
  - `line_stays_outside_crop`: para cualquier área, el rectángulo interior de la línea
    (`area` expandida en `GAP`) contiene el área entera (criterio E1 en la geometría).

### Tarea 5.3 · La capa del borde

**Archivos:** crear `src/area_border/view.rs`; modificar `src/recording.rs`,
`src/application.rs`.

- [ ] `AreaBorder`: ventana-capa con `overlay::init_overlay` en el monitor, anclada a los cuatro
  bordes (`overlay::anchor_all_edges`), sin región de entrada (en `realize`,
  `surface.set_input_region(&cairo::Region::create())`, así el mouse la atraviesa), fondo
  transparente por CSS (`window.area-border { background: none; }`). Un `gtk::DrawingArea` que,
  para cada lado habilitado, dibuja primero el segmento entero en gris `GRAY` y encima los
  guiones blancos (`set_dash(&[DASH, DASH], 0.0)`), con `line_width = LINE_WIDTH`, en el
  rectángulo `area` expandido en `GAP + LINE_WIDTH / 2`.
- [ ] `Recording`: después de `AreaSelector::select`, si hay un solo stream, calcular
  `border_geometry` con `compute_crop(&data)`, `data.stream_size`, `stream.position()` y
  `stream.size()` (del portal) y la geometría del monitor que contiene `stream.position()`
  (`gdk::Display::default().monitors()`); si `position()` es `None` y hay un solo monitor, usar
  ese. Guardar el resultado y el monitor en un campo `area_border: RefCell<Option<(BorderGeometry, gdk::Monitor)>>`
  con getter.
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
| `gstreamer_elements(find: impl Fn(&str) -> bool, profiles: &[(&str, bool)])` | `gst::ElementFactory::find(name).is_some()`; `Profile::all()` con `is_available()` | Falta un elemento base → `Failed` con el paquete de la tabla de abajo; perfil activo sin codificador → `Failed`; otro perfil sin codificador → `Warning` |
| `portal(result: Result<(u32, CursorMode)>, desktop: &str)` | `screencast_portal::Proxy::new().await`, `version()`, `available_cursor_modes()` | `Err` → `Failed` con `sudo apt install xdg-desktop-portal xdg-desktop-portal-kde` (`-gnome` con `GNOME`, `-wlr` en otro caso); `Ok` → detalle «Version N · pointer: …» |
| `pipewire(socket_exists: bool, version: Option<String>)` | `$XDG_RUNTIME_DIR/pipewire-0`; versión como en `src/about.rs` | Sin socket → `Failed`, `systemctl --user enable --now pipewire pipewire-pulse wireplumber` |
| `audio(sink: bool, source: bool)` | `device::find_default(DeviceClass::Sink / Source)` | Falta uno → `Failed`; impacto: «Desktop audio cannot be recorded» o «The microphone cannot be recorded» |
| `cameras(count: usize)` | `camera_pip::devices::list().len()` | 0 → `Warning`, sin comando; impacto: el PiP |
| `overlays(av: Result<(), OverlayUnavailable>)` | `overlay::availability()` | `NotCompiled` → `Failed`, `sudo apt install libgtk4-layer-shell-dev` y nota «Then rebuild Kooha.»; `CompositorUnsupported` → `Failed` sin comando, problema «Your desktop does not support screen overlays (wlr-layer-shell)». Sin display (CLI) → `Warning` «Could not check without a display» |
| `tray(watcher: bool, desktop: &str)` | `NameHasOwner("org.kde.StatusNotifierWatcher")` en el bus de sesión | `false` → `Failed`; en GNOME el comando es `sudo apt install gnome-shell-extension-appindicator` |
| `recordings_folder(path, exists: bool, writable: bool)` | `settings.saving_location()`; escribir y borrar un archivo temporal | No existe → `Failed`, `mkdir -p "<path>"`; no se puede escribir → `Failed`, `ls -ld "<path>"` y nota de permisos |

Paquetes por elemento (Ubuntu): `pipewiresrc` → `gstreamer1.0-pipewire`; `pulsesrc` →
`gstreamer1.0-pulseaudio`; `videoconvert`, `videoscale`, `audioconvert`, `audioresample` →
`gstreamer1.0-plugins-base`; `videocrop`, `videobalance`, `vp8enc`, `webmmux`,
`matroskamux`, `v4l2src` → `gstreamer1.0-plugins-good`; `x264enc` → `gstreamer1.0-plugins-ugly`;
cualquier otro → `gstreamer1.0-plugins-bad`.

- [ ] **Tests primero**, uno por fila: el caso que falla, con su comando exacto, y el caso
  `Ok`.
- [ ] `pub async fn run(settings: &Settings, overlays: Result<(), OverlayUnavailable>) -> Report`
  junta los ocho en el orden de la spec. `overlays` llega como argumento porque sin display no
  se puede consultar.

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
  `overlays` = estado «sin display»), correr `doctor::run` con
  `glib::MainContext::default().block_on`, imprimir `to_text()` por `stdout` y salir con 0 o 1
  según `has_failures()`. Agregar `#[allow(clippy::print_stdout)]` local, porque `main.rs`
  lo prohíbe a nivel de crate. Test: `kooha --check` con `cargo run -- --check` en la shell sin
  `WAYLAND_DISPLAY` ni `DISPLAY` imprime el reporte y no abre ventanas (criterio F2).
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
de punta a punta no se puede hacer en un Xvfb. Se hace en la sesión de Sebastián, sin
instalar: corriendo el binario del worktree con el schema del build
(`GSETTINGS_SCHEMA_DIR=_build/data _build/src/debug/kooha`), que usa el APP_ID `.Devel` y no
pisa la configuración del Kooha instalado. Recorrido: los criterios A1 a F2 de la spec, con
capturas de pantalla de cada mejora y un video corto grabado con pausa y PiP.

## Riesgos

| Riesgo | Mitigación |
|---|---|
| El arrastre del PiP tiembla porque el `offset` se mide contra una superficie que se mueve | `drag_step` encapsulado; plan B en la tarea 4.4 |
| `videobalance` no acepta el formato que entrega `pipewiresrc` | El test 3.3 usa `BGRx`, el formato habitual; si falla en la sesión real, agregar `videoconvert` antes del `videobalance` |
| `gsettings-macro` no genera `(ii)` | Saltear la clave y escribir los métodos a mano (tarea 4.4) |
| El portal de KDE no informa `position()` del stream | Con un solo monitor se usa ese; con varios, no hay borde y se loguea |
