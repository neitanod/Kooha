//! The camera picture-in-picture: a camera source shown in a screen overlay,
//! recorded because it is on screen (ADR 0002).

pub mod caps;
mod changed_control;
pub mod devices;
pub mod geometry;
pub mod v4l2;
pub mod view;

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use gst::prelude::*;
use gtk::{
    cairo, gdk, gio,
    glib::{self, clone},
    prelude::*,
    subclass::prelude::*,
};

use self::{
    caps::{CameraFailure, camera_failure, preferred_caps},
    changed_control::ChangedControl,
    devices::{CameraSource, DeviceCache},
    geometry::{PipShape, clamp_position, default_position, fitted_size, resized, should_mirror},
    view::PipView,
};
use crate::{overlay, settings::Settings};

/// How long `prepare_for_recording` waits for the compositor to present the
/// unmirrored `PiP`.
const PRESENTATION_TIMEOUT: Duration = Duration::from_secs(1);

/// How many frames back `FrameClock::timings` is looked up.
const TIMINGS_LOOKBACK: i64 = 16;

const LAYER_NAMESPACE: &str = "kooha-camera";

mod imp {
    use std::cell::{Cell, OnceCell};

    use gst::bus::BusWatchGuard;

    use super::*;

    #[derive(Debug, Default)]
    pub struct CameraPip {
        pub(super) settings: OnceCell<Settings>,
        pub(super) view: OnceCell<PipView>,
        /// The camera sources, while overlays are available.
        pub(super) devices: OnceCell<DeviceCache>,
        /// The layer surface, while the `PiP` is on.
        pub(super) window: RefCell<Option<gtk::Window>>,
        /// The monitor of `window`, with its `geometry` handler.
        pub(super) monitor: RefCell<Option<(gdk::Monitor, [glib::SignalHandlerId; 2])>>,
        /// The last monitor the Kooha window entered.
        pub(super) kooha_monitor: RefCell<Option<gdk::Monitor>>,
        pub(super) pipeline: RefCell<Option<(gst::Pipeline, BusWatchGuard)>>,
        /// Top-left corner of the `PiP` in its monitor.
        pub(super) position: Cell<(i32, i32)>,
        /// Whether the Kooha window monitor already placed the `PiP` since it
        /// was turned on.
        pub(super) placed_by_kooha_monitor: Cell<bool>,
        pub(super) recording_active: Cell<bool>,
        /// Whether the camera pipeline reached `Playing`: an error before
        /// that is a camera that could not be opened.
        pub(super) camera_playing: Cell<bool>,
        pub(super) prepare_pending: Cell<bool>,
        /// The V4L2 node of the running camera, if it has one.
        pub(super) camera_node: RefCell<Option<PathBuf>>,
        /// The node and the `exposure_dynamic_framerate` value to write back
        /// when the camera stops.
        pub(super) restore_control: RefCell<Option<ChangedControl>>,
        pub(super) drag: RefCell<DragTracker>,
        /// Applies the drag once per frame while dragging.
        pub(super) drag_tick: RefCell<Option<gtk::TickCallbackId>>,
        /// Fired when the window goes away or is replaced.
        pub(super) window_watchers: RefCell<Vec<futures_channel::oneshot::Sender<()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CameraPip {
        const NAME: &'static str = "KoohaCameraPip";
        type Type = super::CameraPip;
    }

    impl ObjectImpl for CameraPip {
        fn dispose(&self) {
            self.obj().turn_off();
        }
    }
}

glib::wrapper! {
    pub struct CameraPip(ObjectSubclass<imp::CameraPip>);
}

impl CameraPip {
    /// Starts mirrored and idle; it learns about the Kooha window monitor and
    /// the recordings through `set_kooha_monitor` and `set_recording_active`.
    pub fn new(settings: &Settings) -> Self {
        let obj = glib::Object::new::<Self>();
        let imp = obj.imp();

        imp.settings.set(settings.clone()).unwrap();
        imp.view.set(PipView::new()).unwrap();
        // A Kooha that died with the camera on may have left it changed.
        changed_control::restore_leftover(
            &changed_control::default_path(),
            set_dynamic_framerate,
        );
        if overlay::availability().is_ok() {
            imp.devices.set(DeviceCache::start()).unwrap();
        }

        obj.setup_controllers();
        obj.setup_settings();
        obj.setup_monitors();

        if settings.camera_enabled() {
            obj.turn_on();
        }

        obj
    }

    fn settings(&self) -> &Settings {
        self.imp().settings.get().unwrap()
    }

    fn view(&self) -> &PipView {
        self.imp().view.get().unwrap()
    }

    /// The camera sources, from the device monitor that this keeps running.
    pub fn sources(&self) -> Vec<CameraSource> {
        self.imp()
            .devices
            .get()
            .map(DeviceCache::sources)
            .unwrap_or_default()
    }

    fn is_on(&self) -> bool {
        self.imp().pipeline.borrow().is_some()
    }

    /// The monitor the Kooha window entered. Without a saved `camera-monitor`,
    /// moves the `PiP` there once per power-on, never while recording.
    pub fn set_kooha_monitor(&self, monitor: Option<gdk::Monitor>) {
        let imp = self.imp();

        imp.kooha_monitor.replace(monitor.clone());

        let Some(monitor) = monitor else {
            return;
        };

        if !moves_to_kooha_monitor(
            self.settings().camera_monitor().is_empty(),
            self.is_on(),
            imp.placed_by_kooha_monitor.get(),
            imp.recording_active.get(),
            imp.prepare_pending.get(),
        ) {
            return;
        }

        imp.placed_by_kooha_monitor.set(true);

        if self.current_monitor().as_ref() != Some(&monitor) {
            self.show_on(&monitor);
        }
    }

    /// Mirrors the `PiP` only while no recording is running (P13).
    pub fn set_recording_active(&self, active: bool) {
        self.imp().recording_active.set(active);
        self.view().set_mirrored(should_mirror(active));
    }

    /// Unmirrors the `PiP` and waits until the compositor presents it, so no
    /// recorded frame shows it mirrored. Fails if that takes more than
    /// `PRESENTATION_TIMEOUT` with the `PiP` on screen.
    pub async fn prepare_for_recording(&self) -> Result<()> {
        let imp = self.imp();

        self.view().set_mirrored(false);

        let view = self.view().downgrade();
        imp.prepare_pending.set(true);
        let res = wait_across_windows(
            || {
                imp.window
                    .borrow()
                    .clone()
                    .filter(|window| window.is_mapped())
                    .and_then(|window| window.frame_clock())
            },
            || self.watch_window(),
            Rc::new(move || {
                if let Some(view) = view.upgrade() {
                    view.queue_draw();
                }
            }),
        )
        .await;
        imp.prepare_pending.set(false);

        res
    }

    /// A receiver that fires when the current window goes away or is
    /// replaced.
    fn watch_window(&self) -> futures_channel::oneshot::Receiver<()> {
        let (sender, receiver) = futures_channel::oneshot::channel();
        self.imp().window_watchers.borrow_mut().push(sender);
        receiver
    }

    fn turn_on(&self) {
        let imp = self.imp();

        if self.is_on() {
            return;
        }

        if let Err(reason) = overlay::availability() {
            tracing::debug!(
                "Not showing the camera: overlays unavailable ({:?})",
                reason
            );
            return;
        }

        let settings = self.settings();

        let sources = self.sources();
        let wanted = settings.camera_device();
        let Some(source) = devices::choose(&sources, &wanted) else {
            settings.set_camera_enabled(false);
            send_notification(&missing_source(&wanted).message());
            return;
        };

        if let Err(err) = self.start_pipeline(&source.id) {
            tracing::error!("Failed to start the camera: {:?}", err);
            settings.set_camera_enabled(false);
            send_notification(&CameraFailure::OpenFailed.message());
            return;
        }

        self.view()
            .set_mirrored(should_mirror(imp.recording_active.get()));

        let kooha_monitor = imp.kooha_monitor.borrow().clone();
        let Some(monitor) = resolve_monitor(
            &settings.camera_monitor(),
            kooha_monitor.as_ref(),
            &monitors(),
        ) else {
            tracing::warn!("Not showing the camera: there is no monitor");
            self.turn_off();
            return;
        };

        imp.placed_by_kooha_monitor
            .set(kooha_monitor.as_ref() == Some(&monitor));

        self.show_on(&monitor);
    }

    fn turn_off(&self) {
        let imp = self.imp();

        self.stop_pipeline();
        self.destroy_window();
        imp.placed_by_kooha_monitor.set(false);
    }

    fn start_pipeline(&self, id: &str) -> Result<()> {
        let device = self
            .imp()
            .devices
            .get()
            .and_then(|devices| devices.device(id))
            .with_context(|| format!("No camera with id `{id}`"))?;
        let source = devices::create_element(&device)?;
        let sink = gst::ElementFactory::make("gtk4paintablesink").build()?;
        let pipeline = camera_pipeline(
            &source,
            &device.caps().unwrap_or_else(gst::Caps::new_any),
            &sink,
        )?;
        self.imp().camera_playing.set(false);

        let bus_watch_guard = pipeline
            .bus()
            .context("Camera pipeline has no bus")?
            .add_watch_local(clone!(
                #[weak(rename_to = obj)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move |_, message| obj.handle_bus_message(message)
            ))?;

        if let Err(err) = pipeline.set_state(gst::State::Playing) {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(err).context("Failed to play the camera pipeline");
        }

        let paintable = sink.property::<gdk::Paintable>("paintable");
        self.view().set_paintable(Some(&paintable));

        self.imp()
            .pipeline
            .replace(Some((pipeline, bus_watch_guard)));

        self.imp().camera_node.replace(camera_node(&device));
        self.keep_camera_framerate();

        Ok(())
    }

    fn stop_pipeline(&self) {
        if let Some((pipeline, _bus_watch_guard)) = self.imp().pipeline.take()
            && let Err(err) = pipeline.set_state(gst::State::Null)
        {
            tracing::warn!("Failed to stop the camera pipeline: {:?}", err);
        }

        self.restore_camera_controls();
        self.imp().camera_node.take();

        if let Some(view) = self.imp().view.get() {
            view.set_paintable(None);
        }
    }

    /// Writes 0 to `exposure_dynamic_framerate` of the running camera, if
    /// `camera-keep-framerate` asks for it, so that in low light the picture
    /// gets darker instead of choppy (P15).
    fn keep_camera_framerate(&self) {
        let imp = self.imp();

        self.restore_camera_controls();

        let Some(node) = imp.camera_node.borrow().clone() else {
            return;
        };
        let current = v4l2::get_control(&node, v4l2::EXPOSURE_DYNAMIC_FRAMERATE)
            .inspect_err(|err| {
                tracing::debug!(
                    "No `exposure_dynamic_framerate` on {}: {:?}",
                    node.display(),
                    err
                );
            })
            .ok();
        let Some(restore_to) =
            keep_framerate_plan(self.settings().camera_keep_framerate(), current)
        else {
            return;
        };

        let control = ChangedControl {
            node,
            value: restore_to,
        };
        if changed_control::change(
            &changed_control::default_path(),
            &control,
            set_dynamic_framerate,
        ) {
            imp.restore_control.replace(Some(control));
        }
    }

    /// Writes back the `exposure_dynamic_framerate` value that
    /// `keep_camera_framerate` replaced, if any. Stopping the camera does it;
    /// the app also calls it when it quits, which may not dispose of the
    /// `PiP`. If Kooha dies before, the next `new` does it.
    pub fn restore_camera_controls(&self) {
        if let Some(control) = self.imp().restore_control.take() {
            changed_control::write_back(
                &changed_control::default_path(),
                &control,
                set_dynamic_framerate,
            );
        }
    }

    fn handle_bus_message(&self, message: &gst::Message) -> glib::ControlFlow {
        match message.view() {
            gst::MessageView::StateChanged(changed)
                if changed.current() == gst::State::Playing
                    && message
                        .src()
                        .is_some_and(|src| src.type_() == gst::Pipeline::static_type()) =>
            {
                self.imp().camera_playing.set(true);
            }
            gst::MessageView::Error(err) => {
                tracing::warn!(
                    "Camera error from {:?}: {} ({:?})",
                    err.src().map(|src| src.path_string()),
                    err.error(),
                    err.debug()
                );

                // A camera that could not be opened or shown is reported as
                // such, not as missing (codex code r1 #3).
                let flow_return = err
                    .details()
                    .and_then(|details| details.get::<i32>("flow-return").ok());
                let failure =
                    camera_failure(&err.error(), flow_return, self.imp().camera_playing.get());

                // Turning off removes this watch, so do it outside of it.
                glib::idle_add_local_once(clone!(
                    #[weak(rename_to = obj)]
                    self,
                    move || {
                        if obj.is_on() {
                            obj.settings().set_camera_enabled(false);
                            send_notification(&failure.message());
                        }
                    }
                ));

                return glib::ControlFlow::Break;
            }
            _ => {}
        }

        glib::ControlFlow::Continue
    }

    fn current_monitor(&self) -> Option<gdk::Monitor> {
        self.imp()
            .monitor
            .borrow()
            .as_ref()
            .map(|(monitor, _)| monitor.clone())
    }

    /// (Re)creates the layer surface on `monitor`.
    fn show_on(&self, monitor: &gdk::Monitor) {
        let imp = self.imp();

        self.destroy_window();

        // Never a window of the app: `Application::window()` assumes that
        // every app window is a `Window`.
        let window = gtk::Window::builder()
            .decorated(false)
            .resizable(false)
            .css_classes(["camera-pip"])
            .build();
        #[cfg(feature = "layer-shell")]
        overlay::init_overlay(&window, monitor, LAYER_NAMESPACE);
        #[cfg(not(feature = "layer-shell"))]
        let _ = LAYER_NAMESPACE;
        window.set_child(Some(self.view()));

        let geometry_handler_id = monitor.connect_geometry_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.relayout()
        ));
        let invalidate_handler_id = monitor.connect_invalidate(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.handle_monitors_changed()
        ));
        imp.monitor.replace(Some((
            monitor.clone(),
            [geometry_handler_id, invalidate_handler_id],
        )));
        imp.window.replace(Some(window.clone()));

        let saved_position = self.settings().camera_position();
        imp.position.set(if saved_position == (-1, -1) {
            let monitor_size = monitor_size(monitor);
            let shape = PipShape::from(self.settings().camera_shape());
            let height = fitted_size(shape, self.camera_size(), monitor_size);
            default_position((shape.width_for(height), height), monitor_size)
        } else {
            saved_position
        });

        self.relayout();
        window.present();
        self.update_input_region();
    }

    fn destroy_window(&self) {
        let imp = self.imp();

        // A drag cancelled by the window going away never emits `drag-end`
        // (fable code r2 NIT).
        if let Some(tick_id) = imp.drag_tick.take() {
            tick_id.remove();
        }

        if let Some(window) = imp.window.take() {
            window.set_child(None::<&gtk::Widget>);
            window.destroy();
        }
        for watcher in imp.window_watchers.take() {
            let _ = watcher.send(());
        }

        if let Some((monitor, handler_ids)) = imp.monitor.take() {
            for handler_id in handler_ids {
                monitor.disconnect(handler_id);
            }
        }
    }

    fn camera_size(&self) -> i32 {
        i32::try_from(self.settings().camera_size()).unwrap_or(geometry::MAX_SIZE)
    }

    /// Fits the `PiP` size and position to its monitor (codex r1 #7).
    fn relayout(&self) {
        let imp = self.imp();

        let Some(window) = imp.window.borrow().clone() else {
            return;
        };
        let Some(monitor) = self.current_monitor() else {
            return;
        };

        let monitor_size = monitor_size(&monitor);
        let shape = PipShape::from(self.settings().camera_shape());
        let height = fitted_size(shape, self.camera_size(), monitor_size);

        let view = self.view();
        view.set_shape(shape);
        view.set_height(height);

        let position = clamp_position(imp.position.get(), view.size(), monitor_size);
        if imp.position.replace(position) != position {
            self.settings().set_camera_position(position);
        }
        place(&window, position);

        self.update_input_region();
    }

    fn update_input_region(&self) {
        let Some(surface) = self
            .imp()
            .window
            .borrow()
            .as_ref()
            .and_then(|window| window.surface())
        else {
            return;
        };

        let view = self.view();
        let (width, height) = view.size();

        let region = cairo::Region::create();
        for (x, y, width, height) in shape_input_rects(view.shape(), width, height) {
            if let Err(err) = region.union_rectangle(&cairo::RectangleInt::new(x, y, width, height))
            {
                tracing::warn!("Failed to build the camera input region: {:?}", err);
                return;
            }
        }
        surface.set_input_region(Some(&region));
    }

    fn handle_drag_begin(&self) {
        let imp = self.imp();

        imp.drag.borrow_mut().begin(imp.position.get());

        // At most one move per frame (fable code r1 #6).
        let tick_id = self.view().add_tick_callback(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move |_, _| {
                obj.apply_drag_frame();
                glib::ControlFlow::Continue
            }
        ));
        if let Some(previous) = imp.drag_tick.replace(Some(tick_id)) {
            previous.remove();
        }
    }

    fn handle_drag_update(&self, offset: (f64, f64)) {
        self.imp().drag.borrow_mut().update(offset);
    }

    /// Moves the `PiP` by the drag updates since the last frame.
    fn apply_drag_frame(&self) {
        let imp = self.imp();

        let Some(window) = imp.window.borrow().clone() else {
            return;
        };
        let Some(monitor) = self.current_monitor() else {
            return;
        };

        let size = self.view().size();
        let monitor_size = monitor_size(&monitor);
        let monitors = monitors();
        let rects = monitors
            .iter()
            .map(|(_, monitor)| {
                let geometry = monitor.geometry();
                (
                    geometry.x(),
                    geometry.y(),
                    geometry.width(),
                    geometry.height(),
                )
            })
            .collect::<Vec<_>>();
        let current = monitors.iter().position(|(_, other)| *other == monitor);

        let mut crossing = None;
        let position = imp.drag.borrow_mut().frame_position(|position| {
            crossing =
                current.and_then(|current| monitor_crossing(position, size, current, &rects));
            clamp_position(position, size, monitor_size)
        });

        if let Some((index, position)) = crossing {
            let (connector, monitor) = &monitors[index];
            self.move_while_dragging(connector.as_deref(), monitor, position);
        } else if let Some(position) = position {
            imp.position.set(position);
            place(&window, position);
        }
    }

    /// Moves the dragged `PiP` over to `monitor`, at `position` on it. The
    /// layer surface is recreated there, which ends the drag: the user picks
    /// the `PiP` up again on the new monitor.
    fn move_while_dragging(
        &self,
        connector: Option<&str>,
        monitor: &gdk::Monitor,
        position: (i32, i32),
    ) {
        let imp = self.imp();

        if let Some(tick_id) = imp.drag_tick.take() {
            tick_id.remove();
        }
        imp.position.set(position);

        // `show_on` places the PiP at the saved position, so save it first.
        let settings = self.settings();
        settings.set_camera_position(position);
        match connector {
            // Following `camera-monitor` recreates the surface on `monitor`.
            Some(connector) => settings.set_camera_monitor(connector),
            None => self.show_on(monitor),
        }
    }

    fn handle_drag_end(&self) {
        let imp = self.imp();

        self.apply_drag_frame();
        if let Some(tick_id) = imp.drag_tick.take() {
            tick_id.remove();
        }

        let settings = self.settings();

        settings.set_camera_position(self.imp().position.get());

        if let Some(connector) = self
            .current_monitor()
            .and_then(|monitor| monitor.connector())
        {
            settings.set_camera_monitor(&connector);
        }
    }

    fn setup_controllers(&self) {
        let view = self.view();

        let drag = gtk::GestureDrag::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        drag.connect_drag_begin(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _| obj.handle_drag_begin()
        ));
        drag.connect_drag_update(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, x, y| obj.handle_drag_update((x, y))
        ));
        drag.connect_drag_end(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _| obj.handle_drag_end()
        ));
        view.add_controller(drag);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, dy| {
                let size = resized(obj.view().height(), dy);
                obj.settings().set_camera_size(size.unsigned_abs());
                glib::Propagation::Stop
            }
        ));
        view.add_controller(scroll);

        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        click.connect_pressed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _, _| {
                let settings = obj.settings();
                let shape = PipShape::from(settings.camera_shape()).toggled();
                settings.set_camera_shape(shape.into());
            }
        ));
        view.add_controller(click);
    }

    fn setup_settings(&self) {
        let settings = self.settings();

        settings.connect_camera_enabled_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |settings| {
                if settings.camera_enabled() {
                    obj.turn_on();
                } else {
                    obj.turn_off();
                }
            }
        ));
        settings.connect_camera_device_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.restart_pipeline()
        ));
        settings.connect_camera_shape_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.relayout()
        ));
        settings.connect_camera_size_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.relayout()
        ));
        settings.connect_camera_monitor_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.follow_camera_monitor()
        ));
        settings.connect_camera_keep_framerate_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |settings| {
                if settings.camera_keep_framerate() {
                    obj.keep_camera_framerate();
                } else {
                    obj.restore_camera_controls();
                }
            }
        ));
    }

    fn restart_pipeline(&self) {
        if !self.is_on() {
            return;
        }

        self.stop_pipeline();

        let settings = self.settings();
        let sources = self.sources();
        let wanted = settings.camera_device();
        let Some(source) = devices::choose(&sources, &wanted) else {
            settings.set_camera_enabled(false);
            send_notification(&missing_source(&wanted).message());
            return;
        };

        if let Err(err) = self.start_pipeline(&source.id) {
            tracing::error!("Failed to switch the camera: {:?}", err);
            settings.set_camera_enabled(false);
            send_notification(&CameraFailure::OpenFailed.message());
        }
    }

    fn setup_monitors(&self) {
        if let Some(display) = gdk::Display::default() {
            display.monitors().connect_items_changed(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _, _, _| obj.handle_monitors_changed()
            ));
        }
    }

    /// When its monitor is disconnected, the compositor closes the layer
    /// while the camera keeps capturing, so the `PiP` moves to another
    /// monitor, or turns off without one (fable code r1 #8).
    fn handle_monitors_changed(&self) {
        if !self.is_on() {
            return;
        }

        let current = self.current_monitor();
        let kooha_monitor = self.imp().kooha_monitor.borrow().clone();
        match after_monitors_changed(
            current.as_ref(),
            &self.settings().camera_monitor(),
            kooha_monitor.as_ref(),
            &monitors(),
        ) {
            MonitorAction::Keep => {}
            MonitorAction::MoveTo(monitor) => self.show_on(&monitor),
            MonitorAction::TurnOff => {
                tracing::warn!("Turning the camera off: there is no monitor");
                self.settings().set_camera_enabled(false);
            }
        }
    }

    /// A layer cannot be dragged to another monitor, so Preferences picks it
    /// (fable r2 #3).
    fn follow_camera_monitor(&self) {
        if !self.is_on() {
            return;
        }

        let kooha_monitor = self.imp().kooha_monitor.borrow().clone();
        let Some(monitor) = resolve_monitor(
            &self.settings().camera_monitor(),
            kooha_monitor.as_ref(),
            &monitors(),
        ) else {
            return;
        };

        if self.current_monitor().as_ref() != Some(&monitor) {
            self.show_on(&monitor);
        }
    }
}

/// What the presentation barrier needs of a `gdk::FrameClock`; the tests
/// use a clock of their own.
pub trait PresentationClock {
    type Handler;

    fn frame_counter(&self) -> i64;
    /// Whether `frame` is complete, and when it was presented (0: unknown),
    /// while the clock still keeps it.
    fn timings(&self, frame: i64) -> Option<(bool, i64)>;
    fn request_paint(&self);
    fn connect_after_paint(&self, f: impl Fn(&Self) + 'static) -> Self::Handler;
    fn disconnect(&self, handler: Self::Handler);
}

impl PresentationClock for gdk::FrameClock {
    type Handler = glib::SignalHandlerId;

    fn frame_counter(&self) -> i64 {
        gdk::FrameClock::frame_counter(self)
    }

    fn timings(&self, frame: i64) -> Option<(bool, i64)> {
        gdk::FrameClock::timings(self, frame)
            .map(|timings| (timings.is_complete(), timings.presentation_time()))
    }

    fn request_paint(&self) {
        self.request_phase(gdk::FrameClockPhase::PAINT);
    }

    fn connect_after_paint(&self, f: impl Fn(&Self) + 'static) -> Self::Handler {
        gdk::FrameClock::connect_after_paint(self, f)
    }

    fn disconnect(&self, handler: Self::Handler) {
        glib::prelude::ObjectExt::disconnect(self, handler);
    }
}

/// How one wait on one window ended, if not by the timeout.
enum WaitEnd {
    Presented,
    /// The window went away or was replaced.
    WindowChanged,
}

/// Waits until the compositor presents an unmirrored frame of the `PiP`
/// window that is current at each moment. `current_clock` gives the clock of
/// that window, None once it is gone; `watch_window` a receiver that fires
/// when it goes away or is replaced. A `PiP` turned off ends the wait well,
/// and a replaced window restarts it on the new one (codex code r1 #5).
async fn wait_across_windows<C: PresentationClock>(
    mut current_clock: impl FnMut() -> Option<C>,
    mut watch_window: impl FnMut() -> futures_channel::oneshot::Receiver<()>,
    redraw: Rc<dyn Fn()>,
) -> Result<()> {
    loop {
        let Some(clock) = current_clock() else {
            return Ok(());
        };
        let window_changed = watch_window();

        match wait_for_presentation(&clock, &redraw, window_changed).await? {
            WaitEnd::Presented => return Ok(()),
            WaitEnd::WindowChanged => {}
        }
    }
}

/// Waits until the compositor presents a frame of `clock` drawn after now
/// (codex r3 #1 and r4 #1), or until `window_changed` fires.
async fn wait_for_presentation<C: PresentationClock>(
    clock: &C,
    redraw: &Rc<dyn Fn()>,
    window_changed: futures_channel::oneshot::Receiver<()>,
) -> Result<WaitEnd> {
    let changed_at = clock.frame_counter();
    let barrier = RefCell::new(PresentationBarrier::new(changed_at));
    let (sender, receiver) = futures_channel::oneshot::channel();
    let sender = RefCell::new(Some(sender));

    let handler = clock.connect_after_paint({
        let redraw = Rc::clone(redraw);
        move |clock| {
            let current = clock.frame_counter();
            let first = (changed_at + 1).max(current - TIMINGS_LOOKBACK);
            let is_done = (first..=current).any(|frame| {
                clock
                    .timings(frame)
                    .is_some_and(|(complete, presentation_time)| {
                        barrier
                            .borrow_mut()
                            .observe(frame, complete, presentation_time)
                            == BarrierState::Done
                    })
            });

            if is_done {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(());
                }
            } else {
                // One invalidates the drawing, the other runs the clock.
                redraw();
                clock.request_paint();
            }
        }
    });

    redraw();
    clock.request_paint();

    let ended = futures_util::future::select(receiver, window_changed);
    let res = glib::future_with_timeout(PRESENTATION_TIMEOUT, ended).await;
    clock.disconnect(handler);

    match res {
        Ok(futures_util::future::Either::Left((Ok(()), _))) => Ok(WaitEnd::Presented),
        // Fired, or dropped by `destroy_window`.
        Ok(futures_util::future::Either::Right(_)) => Ok(WaitEnd::WindowChanged),
        Ok(futures_util::future::Either::Left((Err(_), _))) => {
            bail!("The camera overlay went away while waiting for it")
        }
        Err(_) => bail!(
            "No unmirrored frame of the camera overlay was presented within {:?}",
            PRESENTATION_TIMEOUT
        ),
    }
}

/// Why no source matches `camera-device` = `wanted`: with a saved camera,
/// that it is gone; without one, that there are none.
pub fn missing_source(wanted: &str) -> CameraFailure {
    if wanted.is_empty() {
        CameraFailure::NotFound
    } else {
        CameraFailure::Disconnected
    }
}

/// `source ! capsfilter ! decodebin ! videoconvert ! sink`, asking `source`
/// for a fluid mode, raw or JPEG, which `decodebin` turns into raw frames
/// (fable code r1 #2, codex code r1 #3).
fn camera_pipeline(
    source: &gst::Element,
    device_caps: &gst::Caps,
    sink: &gst::Element,
) -> Result<gst::Pipeline> {
    let caps = preferred_caps(device_caps);
    tracing::debug!("Camera caps: {}", caps);

    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()?;
    let decodebin = gst::ElementFactory::make("decodebin").build()?;
    let convert = gst::ElementFactory::make("videoconvert").build()?;

    let pipeline = gst::Pipeline::new();
    pipeline.add_many([source, &capsfilter, &decodebin, &convert, sink])?;
    gst::Element::link_many([source, &capsfilter, &decodebin])?;
    convert.link(sink)?;
    decodebin.connect_pad_added(clone!(
        #[weak]
        convert,
        move |_, pad| link_video_pad(pad, &convert)
    ));

    Ok(pipeline)
}

/// What to do with `exposure_dynamic_framerate` when the camera starts, from
/// the `camera-keep-framerate` setting and the control's value, if the camera
/// has it. With `Some(restore_to)`, Kooha writes 0, so that the camera keeps
/// its frame rate in low light, and writes `restore_to` back when it stops.
fn keep_framerate_plan(enabled: bool, current: Option<i32>) -> Option<i32> {
    current.filter(|&value| enabled && value != 0)
}

/// Writes `exposure_dynamic_framerate` of the camera at `node`.
fn set_dynamic_framerate(node: &Path, value: i32) -> std::io::Result<()> {
    v4l2::set_control(node, v4l2::EXPOSURE_DYNAMIC_FRAMERATE, value)
}

/// The V4L2 node of `device`, if it has one.
fn camera_node(device: &gst::Device) -> Option<PathBuf> {
    let path = devices::device_path(device)?;
    v4l2::video_node(&path).map(Path::to_path_buf)
}

/// Links the first video pad of `decodebin` to `convert`.
fn link_video_pad(pad: &gst::Pad, convert: &gst::Element) {
    let Some(sink_pad) = convert.static_pad("sink") else {
        return;
    };
    let is_video = pad
        .current_caps()
        .unwrap_or_else(|| pad.query_caps(None))
        .structure(0)
        .is_some_and(|structure| structure.name().starts_with("video/"));

    if is_video
        && !sink_pad.is_linked()
        && let Err(err) = pad.link(&sink_pad)
    {
        tracing::warn!("Failed to link the decoded camera video: {:?}", err);
    }
}

fn place(window: &gtk::Window, position: (i32, i32)) {
    #[cfg(feature = "layer-shell")]
    overlay::set_margins(window, position.0, position.1);
    #[cfg(not(feature = "layer-shell"))]
    let _ = (window, position);
}

fn monitor_size(monitor: &gdk::Monitor) -> (i32, i32) {
    let geometry = monitor.geometry();
    (geometry.width(), geometry.height())
}

/// The connected monitors, each with its connector.
fn monitors() -> Vec<(Option<String>, gdk::Monitor)> {
    gdk::Display::default()
        .map(|display| {
            display
                .monitors()
                .iter::<gdk::Monitor>()
                .filter_map(Result::ok)
                .filter(|monitor| monitor.is_valid())
                .map(|monitor| (monitor.connector().map(Into::into), monitor))
                .collect()
        })
        .unwrap_or_default()
}

/// The camera sources of the app's `CameraPip`; none without it.
pub fn app_sources() -> Vec<CameraSource> {
    gio::Application::default()
        .and_downcast::<crate::application::Application>()
        .and_then(|app| app.camera_pip().map(CameraPip::sources))
        .unwrap_or_default()
}

fn send_notification(title: &str) {
    if let Some(app) = gio::Application::default() {
        let notification = gio::Notification::new(title);
        app.send_notification(Some("camera"), &notification);
    }
}

/// Rows per strip of the input region.
const INPUT_STRIP_HEIGHT: i32 = 2;

/// The input region of a `PiP` of `width` × `height` in `shape`, as `(x, y,
/// width, height)` rectangles: one horizontal strip per two rows, so that the
/// clicks outside the circle or the rounded corners reach what is below
/// (fable r2 #10).
pub fn shape_input_rects(shape: PipShape, width: i32, height: i32) -> Vec<(i32, i32, i32, i32)> {
    let radius = f64::from(shape.corner_radius(height)).min(f64::from(width) / 2.0);

    (0..height)
        .step_by(INPUT_STRIP_HEIGHT as usize)
        .filter_map(|y| {
            let strip_height = INPUT_STRIP_HEIGHT.min(height - y);
            let center = f64::from(y) + f64::from(strip_height) / 2.0;

            let distance_into_corner = if center < radius {
                radius - center
            } else if center > f64::from(height) - radius {
                center - (f64::from(height) - radius)
            } else {
                0.0
            };
            let inset = (radius
                - (radius.powi(2) - distance_into_corner.powi(2))
                    .max(0.0)
                    .sqrt())
            .floor() as i32;

            let strip_width = width - 2 * inset;
            (strip_width > 0).then_some((inset, y, strip_width, strip_height))
        })
        .collect()
}

/// The monitor to show the `PiP` on: the one saved in `camera-monitor` if it
/// is connected; else the one of the Kooha window; else the first one.
/// `monitors` pairs each monitor with its connector.
pub fn resolve_monitor<M: Clone + PartialEq>(
    saved_connector: &str,
    kooha_window_monitor: Option<&M>,
    monitors: &[(Option<String>, M)],
) -> Option<M> {
    let saved = (!saved_connector.is_empty())
        .then(|| {
            monitors
                .iter()
                .find(|(connector, _)| connector.as_deref() == Some(saved_connector))
        })
        .flatten()
        .map(|(_, monitor)| monitor.clone());

    saved
        .or_else(|| kooha_window_monitor.cloned())
        .or_else(|| monitors.first().map(|(_, monitor)| monitor.clone()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorAction<M> {
    Keep,
    MoveTo(M),
    TurnOff,
}

/// What the `PiP` on `current` does when the `monitors` change: stays while
/// its monitor is there; else goes where `resolve_monitor` says, among the
/// monitors still there; else turns off.
pub fn after_monitors_changed<M: Clone + PartialEq>(
    current: Option<&M>,
    saved_connector: &str,
    kooha_window_monitor: Option<&M>,
    monitors: &[(Option<String>, M)],
) -> MonitorAction<M> {
    let is_connected = |wanted: &M| monitors.iter().any(|(_, monitor)| monitor == wanted);

    if current.is_some_and(is_connected) {
        return MonitorAction::Keep;
    }

    resolve_monitor(
        saved_connector,
        kooha_window_monitor.filter(|monitor| is_connected(monitor)),
        monitors,
    )
    .map_or(MonitorAction::TurnOff, MonitorAction::MoveTo)
}

/// Where a dragged `PiP` goes when its center leaves its monitor for
/// another one: a layer surface lives on one monitor, so it cannot be
/// dragged across, and has to be moved over instead.
///
/// `position` is the unclamped top-left of the `PiP` on `monitors[current]`,
/// and every monitor is its logical rectangle `(x, y, width, height)`.
/// Returns the index of the monitor that now holds the center, and the
/// top-left on it, clamped to fit; `None` while the center stays on the
/// current monitor or falls where no monitor is.
pub fn monitor_crossing(
    position: (i32, i32),
    pip: (i32, i32),
    current: usize,
    monitors: &[(i32, i32, i32, i32)],
) -> Option<(usize, (i32, i32))> {
    let &(origin_x, origin_y, ..) = monitors.get(current)?;
    let top_left = (origin_x + position.0, origin_y + position.1);
    let center = (top_left.0 + pip.0 / 2, top_left.1 + pip.1 / 2);

    let contains = |&(x, y, width, height): &(i32, i32, i32, i32)| {
        (x..x + width).contains(&center.0) && (y..y + height).contains(&center.1)
    };
    if contains(&monitors[current]) {
        return None;
    }

    let (index, &(x, y, width, height)) = monitors
        .iter()
        .enumerate()
        .find(|(_, monitor)| contains(monitor))?;
    let local = clamp_position((top_left.0 - x, top_left.1 - y), pip, (width, height));
    Some((index, local))
}

/// The `PiP` position after a drag update. The surface moves with the
/// pointer, so `offset` is relative to the already moved surface.
pub fn drag_step(current: (i32, i32), offset: (f64, f64)) -> (i32, i32) {
    (
        current.0 + offset.0.round() as i32,
        current.1 + offset.1.round() as i32,
    )
}

/// Turns the drag updates into at most one position per frame (fable code
/// r1 #6).
///
/// `GestureDrag` gives each offset from where the drag began, measured on
/// the surface, which moves with the pointer: every update of a frame is
/// relative to where the surface was when the frame began. So a frame moves
/// the `PiP` once, from that position by the latest offset; adding up the
/// offsets of one frame would run ahead of the pointer. Starting every
/// frame from where the drag began would instead move the `PiP` at half the
/// speed of the pointer, as the offsets shrink while the surface follows.
#[derive(Debug, Default)]
pub struct DragTracker {
    /// Where the surface was placed by the last frame.
    frame_start: (i32, i32),
    /// The latest offset since the last frame.
    pending_offset: Option<(f64, f64)>,
}

impl DragTracker {
    pub fn begin(&mut self, position: (i32, i32)) {
        self.frame_start = position;
        self.pending_offset = None;
    }

    pub fn update(&mut self, offset: (f64, f64)) {
        self.pending_offset = Some(offset);
    }

    /// The position for this frame, through `clamp`, if an update arrived
    /// since the last frame.
    pub fn frame_position(
        &mut self,
        clamp: impl FnOnce((i32, i32)) -> (i32, i32),
    ) -> Option<(i32, i32)> {
        let offset = self.pending_offset.take()?;
        let position = clamp(drag_step(self.frame_start, offset));
        self.frame_start = position;
        Some(position)
    }
}

/// Whether the Kooha window entering a monitor moves the `PiP` there: only
/// without a saved `camera-monitor`, once per power-on, and never while a
/// recording runs or is being prepared, where a move would show the `PiP`
/// somewhere else than where it was confirmed unmirrored.
pub fn moves_to_kooha_monitor(
    no_saved_monitor: bool,
    is_on: bool,
    placed_by_kooha_monitor: bool,
    recording_active: bool,
    prepare_pending: bool,
) -> bool {
    no_saved_monitor && is_on && !placed_by_kooha_monitor && !recording_active && !prepare_pending
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierState {
    Pending,
    Done,
}

/// Waits for the compositor to confirm that a frame drawn after a change
/// was presented (codex r3 #1 and r4 #1).
#[derive(Debug)]
pub struct PresentationBarrier {
    changed_at: i64,
    state: BarrierState,
}

impl PresentationBarrier {
    /// `changed_at` is the frame counter when the change was made.
    pub fn new(changed_at: i64) -> Self {
        Self {
            changed_at,
            state: BarrierState::Pending,
        }
    }

    /// Takes the timings of `frame`. A complete frame with a presentation
    /// time of 0 was presented at an unknown time, which does not count.
    pub fn observe(&mut self, frame: i64, complete: bool, presentation_time: i64) -> BarrierState {
        if frame > self.changed_at && complete && presentation_time > 0 {
            self.state = BarrierState::Done;
        }

        self.state
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, time::Instant};

    use super::{geometry::PipShape, *};

    fn contains(rects: &[(i32, i32, i32, i32)], point: (i32, i32)) -> bool {
        rects.iter().any(|&(x, y, width, height)| {
            (x..x + width).contains(&point.0) && (y..y + height).contains(&point.1)
        })
    }

    #[test]
    fn circle_input_region() {
        let rects = shape_input_rects(PipShape::Circle, 200, 200);

        assert!(contains(&rects, (100, 100)));
        assert!(contains(&rects, (1, 100)));
        assert!(!contains(&rects, (2, 2)));
        assert!(!contains(&rects, (197, 197)));
    }

    #[test]
    fn rounded_rectangle_input_region() {
        let rects = shape_input_rects(PipShape::RoundedRectangle, 356, 200);

        assert!(contains(&rects, (1, 100)));
        assert!(contains(&rects, (178, 1)));
        assert!(contains(&rects, (354, 100)));
        assert!(!contains(&rects, (1, 1)));
        assert!(!contains(&rects, (355, 199)));
        assert!(!contains(&rects, (356, 100)));
    }

    /// A JPEG camera, played by `videotestsrc ! jpegenc`, reaches the sink
    /// as raw frames.
    #[test]
    fn jpeg_camera_is_decoded() {
        gst::init().unwrap();
        for element in ["jpegenc", "jpegdec", "decodebin"] {
            if gst::ElementFactory::find(element).is_none() {
                eprintln!("Skipped: no {element}");
                return;
            }
        }

        let source = gst::parse::bin_from_description(
            "videotestsrc num-buffers=5 ! video/x-raw,width=1280,height=720,framerate=30/1 ! jpegenc",
            true,
        )
        .unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();
        let device_caps = gst::Caps::builder("image/jpeg")
            .field("width", 1280)
            .field("height", 720)
            .field("framerate", gst::Fraction::new(30, 1))
            .build();

        let pipeline = camera_pipeline(source.upcast_ref(), &device_caps, &sink).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let message = pipeline
            .bus()
            .unwrap()
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(5),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("no EOS within 5 s");
        let caps = sink.static_pad("sink").unwrap().current_caps();
        pipeline.set_state(gst::State::Null).unwrap();

        assert_eq!(message.type_(), gst::MessageType::Eos, "{message:?}");
        assert_eq!(
            caps.unwrap().structure(0).unwrap().name().as_str(),
            "video/x-raw"
        );
    }

    #[test]
    fn missing_source_failure() {
        assert_eq!(missing_source(""), CameraFailure::NotFound);
        assert_eq!(missing_source("/dev/video2"), CameraFailure::Disconnected);
        assert_eq!(CameraFailure::NotFound.message(), "No camera found");
        assert_eq!(CameraFailure::Disconnected.message(), "Camera disconnected");
    }

    /// A disconnected monitor takes the `PiP` layer with it, while the
    /// camera keeps capturing (fable code r1 #8).
    #[test]
    fn monitors_change_moves_or_turns_off() {
        let monitors = [
            (Some("eDP-1".to_string()), 0),
            (Some("DP-1".to_string()), 1),
        ];

        // Its monitor is still there.
        assert_eq!(
            after_monitors_changed(Some(&1), "DP-1", Some(&0), &monitors),
            MonitorAction::Keep
        );
        // Gone: the saved one, else the Kooha window's, else the first.
        let without_dp = &monitors[..1];
        assert_eq!(
            after_monitors_changed(Some(&1), "DP-1", Some(&0), without_dp),
            MonitorAction::MoveTo(0)
        );
        assert_eq!(
            after_monitors_changed(Some(&2), "HDMI-1", Some(&1), &monitors),
            MonitorAction::MoveTo(1)
        );
        // The Kooha window's monitor went away too.
        assert_eq!(
            after_monitors_changed(Some(&1), "", Some(&1), without_dp),
            MonitorAction::MoveTo(0)
        );
        // None left.
        assert_eq!(
            after_monitors_changed(Some(&1), "DP-1", Some(&1), &[]),
            MonitorAction::TurnOff
        );
    }

    #[test]
    fn resolve_monitor_order() {
        let monitors = [
            (Some("eDP-1".to_string()), 0),
            (Some("DP-1".to_string()), 1),
            (None, 2),
        ];

        // The saved monitor, when connected.
        assert_eq!(resolve_monitor("DP-1", Some(&2), &monitors), Some(1));
        // Else the one of the Kooha window.
        assert_eq!(resolve_monitor("HDMI-1", Some(&2), &monitors), Some(2));
        assert_eq!(resolve_monitor("", Some(&1), &monitors), Some(1));
        // Else the first one.
        assert_eq!(resolve_monitor("HDMI-1", None, &monitors), Some(0));
        assert_eq!(resolve_monitor("", None, &[]), None::<i32>);
    }

    /// One position per frame, from the latest offset only: the offsets of
    /// a frame are all relative to where the surface was when it began, so
    /// adding them up runs ahead of the pointer (fable code r1 #6).
    #[test]
    fn drag_tracker_applies_latest_offset_once_per_frame() {
        let no_clamp = |position| position;
        let mut tracker = DragTracker::default();

        tracker.begin((100, 100));
        assert_eq!(tracker.frame_position(no_clamp), None);

        tracker.update((5.0, 0.0));
        tracker.update((8.0, -2.0));
        assert_eq!(tracker.frame_position(no_clamp), Some((108, 98)));
        // No new update, nothing to move.
        assert_eq!(tracker.frame_position(no_clamp), None);

        tracker.update((3.0, 1.0));
        assert_eq!(tracker.frame_position(no_clamp), Some((111, 99)));
    }

    #[test]
    fn drag_tracker_continues_from_clamped_position() {
        let clamp = |(x, y): (i32, i32)| (x.min(500), y);
        let mut tracker = DragTracker::default();

        tracker.begin((100, 100));
        tracker.update((1000.0, 0.0));
        assert_eq!(tracker.frame_position(clamp), Some((500, 100)));

        // The surface stopped at the edge, so the next offset is relative
        // to it.
        tracker.update((-10.0, 0.0));
        assert_eq!(tracker.frame_position(clamp), Some((490, 100)));
    }

    // Sebastián's layout (`kscreen-doctor -o`, 17/11/2026), as logical
    // rectangles: HDMI-A-5 on top, eDP-1 below it, DP-3 to the right.
    const HDMI: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
    const EDP: (i32, i32, i32, i32) = (457, 1080, 1463, 915);
    const DP: (i32, i32, i32, i32) = (1920, 454, 1600, 1000);

    #[test]
    fn crossing_up_moves_to_the_monitor_above() {
        // A 200×200 PiP dragged from eDP-1 to 150 px above its top edge:
        // its center (local y = -150 + 100) is now inside HDMI-A-5.
        let crossing = monitor_crossing((400, -150), (200, 200), 1, &[HDMI, EDP, DP]);

        // Global top-left (857, 930) is local (857, 930) on HDMI-A-5,
        // clamped so the whole PiP fits: y = 1080 - 200.
        assert_eq!(crossing, Some((0, (857, 880))));
    }

    #[test]
    fn crossing_right_moves_to_the_monitor_beside() {
        // From HDMI-A-5 past its right edge, at a height DP-3 covers.
        let crossing = monitor_crossing((1850, 600), (200, 200), 0, &[HDMI, EDP, DP]);

        // Global (1850, 600) is local (-70, 146) on DP-3, clamped to x = 0.
        assert_eq!(crossing, Some((2, (0, 146))));
    }

    #[test]
    fn no_crossing_while_the_center_stays_home() {
        // Partly past the top edge of eDP-1, but the center is still on it.
        assert_eq!(
            monitor_crossing((400, -50), (200, 200), 1, &[HDMI, EDP, DP]),
            None
        );
    }

    #[test]
    fn no_crossing_into_empty_space() {
        // Past the left edge of eDP-1, where no monitor is: it stays put,
        // and the caller clamps it to its own monitor.
        assert_eq!(
            monitor_crossing((-300, 400), (200, 200), 1, &[HDMI, EDP, DP]),
            None
        );
    }

    #[test]
    fn keep_framerate_plan_writes_zero_only_when_needed() {
        assert_eq!(keep_framerate_plan(false, Some(1)), None);
        assert_eq!(keep_framerate_plan(true, None), None);
        assert_eq!(keep_framerate_plan(true, Some(0)), None);
        assert_eq!(keep_framerate_plan(true, Some(1)), Some(1));
    }

    #[test]
    fn drag_step_adds_offset() {
        assert_eq!(drag_step((100, 100), (5.0, -3.0)), (105, 97));
        assert_eq!(drag_step((100, 100), (0.6, -0.6)), (101, 99));
    }

    #[test]
    fn kooha_monitor_moves_once_per_power_on() {
        // camera-monitor empty, PiP on, not placed yet, idle: moves.
        assert!(moves_to_kooha_monitor(true, true, false, false, false));
        // A saved camera-monitor wins.
        assert!(!moves_to_kooha_monitor(false, true, false, false, false));
        // PiP off: nothing to move.
        assert!(!moves_to_kooha_monitor(true, false, false, false, false));
        // Already placed by the Kooha monitor since it was turned on.
        assert!(!moves_to_kooha_monitor(true, true, true, false, false));
        // Never while recording, nor while preparing a recording.
        assert!(!moves_to_kooha_monitor(true, true, false, true, false));
        assert!(!moves_to_kooha_monitor(true, true, false, false, true));
    }

    /// A frame clock that paints a frame every 16 ms once asked, and whose
    /// frames are presented at a known time only if `presents`.
    #[derive(Clone)]
    struct FakeClock(Rc<FakeClockInner>);

    struct FakeClockInner {
        presents: bool,
        counter: Cell<i64>,
        painting: Cell<bool>,
        paints: Cell<u32>,
        next_handler: Cell<u32>,
        #[allow(clippy::type_complexity)]
        handlers: RefCell<Vec<(u32, Rc<dyn Fn(&FakeClock)>)>>,
    }

    impl FakeClock {
        fn new(presents: bool) -> Self {
            Self(Rc::new(FakeClockInner {
                presents,
                counter: Cell::new(100),
                painting: Cell::new(false),
                paints: Cell::new(0),
                next_handler: Cell::new(0),
                handlers: RefCell::new(Vec::new()),
            }))
        }

        fn paints(&self) -> u32 {
            self.0.paints.get()
        }
    }

    impl PresentationClock for FakeClock {
        type Handler = u32;

        fn frame_counter(&self) -> i64 {
            self.0.counter.get()
        }

        fn timings(&self, frame: i64) -> Option<(bool, i64)> {
            (frame <= self.0.counter.get())
                .then_some((true, if self.0.presents { 1000 } else { 0 }))
        }

        fn request_paint(&self) {
            if self.0.painting.replace(true) {
                return;
            }
            let clock = self.clone();
            // On the context of the test, which `timeout_add_local` would
            // not use.
            glib::MainContext::ref_thread_default().spawn_local(async move {
                glib::timeout_future(Duration::from_millis(16)).await;
                clock.0.painting.set(false);
                clock.0.counter.set(clock.0.counter.get() + 1);
                clock.0.paints.set(clock.0.paints.get() + 1);
                let handlers = clock
                    .0
                    .handlers
                    .borrow()
                    .iter()
                    .map(|(_, handler)| Rc::clone(handler))
                    .collect::<Vec<_>>();
                for handler in handlers {
                    handler(&clock);
                }
            });
        }

        fn connect_after_paint(&self, f: impl Fn(&Self) + 'static) -> u32 {
            let id = self.0.next_handler.get();
            self.0.next_handler.set(id + 1);
            self.0.handlers.borrow_mut().push((id, Rc::new(f)));
            id
        }

        fn disconnect(&self, handler: u32) {
            self.0
                .handlers
                .borrow_mut()
                .retain(|(id, _)| *id != handler);
        }
    }

    /// Runs `future` on a main context of its own, as the tests run in
    /// parallel.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| context.block_on(future))
            .unwrap()
    }

    fn no_redraw() -> Rc<dyn Fn()> {
        Rc::new(|| {})
    }

    type Watcher = Rc<RefCell<Vec<futures_channel::oneshot::Sender<()>>>>;

    /// `watch_window` for `wait_across_windows`: the senders stay in
    /// `watchers` until a test fires them.
    fn watch(watchers: &Watcher) -> impl FnMut() -> futures_channel::oneshot::Receiver<()> {
        let watchers = Rc::clone(watchers);
        move || {
            let (sender, receiver) = futures_channel::oneshot::channel();
            watchers.borrow_mut().push(sender);
            receiver
        }
    }

    fn fire(watchers: &Watcher) {
        for sender in watchers.take() {
            let _ = sender.send(());
        }
    }

    /// The timeout that aborts a recording cuts the wait at 1 s (fable code
    /// r1 #9).
    #[test]
    fn wait_fails_when_nothing_is_presented() {
        let clock = FakeClock::new(false);
        let watchers = Watcher::default();

        let started = Instant::now();
        let res = block_on(wait_across_windows(
            || Some(clock.clone()),
            watch(&watchers),
            no_redraw(),
        ));
        let elapsed = started.elapsed();

        assert!(res.is_err());
        assert!(
            (Duration::from_secs(1)..Duration::from_millis(1500)).contains(&elapsed),
            "{elapsed:?}"
        );
        assert!(clock.paints() > 10, "{}", clock.paints());
    }

    #[test]
    fn wait_ends_when_presented() {
        let clock = FakeClock::new(true);
        let watchers = Watcher::default();

        let started = Instant::now();
        let res = block_on(wait_across_windows(
            || Some(clock.clone()),
            watch(&watchers),
            no_redraw(),
        ));

        assert!(res.is_ok());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    /// Turning the `PiP` off while waiting is no reason to abort the
    /// recording: nothing mirrored is left on screen (codex code r1 #5).
    #[test]
    fn wait_ends_well_when_the_pip_goes_away() {
        let clock = FakeClock::new(false);
        let watchers = Watcher::default();
        let window_gone = Rc::new(Cell::new(false));

        let started = Instant::now();
        let res = block_on(async {
            let trigger = async {
                glib::timeout_future(Duration::from_millis(200)).await;
                window_gone.set(true);
                fire(&watchers);
            };
            let wait = wait_across_windows(
                || (!window_gone.get()).then(|| clock.clone()),
                watch(&watchers),
                no_redraw(),
            );
            futures_util::future::join(trigger, wait).await.1
        });
        let elapsed = started.elapsed();

        assert!(res.is_ok());
        assert!(
            (Duration::from_millis(200)..Duration::from_millis(600)).contains(&elapsed),
            "{elapsed:?}"
        );
    }

    /// Moving the `PiP` to another monitor replaces its window, and the
    /// wait goes on with the new one (codex code r1 #5).
    #[test]
    fn wait_follows_a_replaced_window() {
        let old = FakeClock::new(false);
        let new = FakeClock::new(true);
        let watchers = Watcher::default();
        let replaced = Rc::new(Cell::new(false));

        let started = Instant::now();
        let res = block_on(async {
            let trigger = async {
                glib::timeout_future(Duration::from_millis(200)).await;
                replaced.set(true);
                fire(&watchers);
            };
            let wait = wait_across_windows(
                || {
                    Some(if replaced.get() {
                        new.clone()
                    } else {
                        old.clone()
                    })
                },
                watch(&watchers),
                no_redraw(),
            );
            futures_util::future::join(trigger, wait).await.1
        });
        let elapsed = started.elapsed();

        assert!(res.is_ok());
        assert!(
            (Duration::from_millis(200)..Duration::from_millis(600)).contains(&elapsed),
            "{elapsed:?}"
        );
        assert!(new.paints() > 0);
    }

    #[test]
    fn barrier_ignores_frames_before_change() {
        let mut barrier = PresentationBarrier::new(10);

        assert_eq!(barrier.observe(9, true, 1000), BarrierState::Pending);
        assert_eq!(barrier.observe(10, true, 1000), BarrierState::Pending);
    }

    #[test]
    fn barrier_needs_presentation_time() {
        let mut barrier = PresentationBarrier::new(10);

        assert_eq!(barrier.observe(11, true, 0), BarrierState::Pending);
        assert_eq!(barrier.observe(12, true, 0), BarrierState::Pending);
        assert_eq!(barrier.observe(13, true, 0), BarrierState::Pending);
        // Not complete yet, even with a time.
        assert_eq!(barrier.observe(14, false, 5000), BarrierState::Pending);
    }

    #[test]
    fn barrier_done_when_presented() {
        let mut barrier = PresentationBarrier::new(10);

        assert_eq!(barrier.observe(11, true, 5000), BarrierState::Done);
        // Stays done.
        assert_eq!(barrier.observe(12, false, 0), BarrierState::Done);
    }

    #[test]
    fn barrier_done_when_presented_late() {
        let mut barrier = PresentationBarrier::new(10);

        assert_eq!(barrier.observe(11, false, 0), BarrierState::Pending);
        assert_eq!(barrier.observe(12, false, 0), BarrierState::Pending);
        assert_eq!(barrier.observe(11, true, 0), BarrierState::Pending);
        assert_eq!(barrier.observe(12, true, 7000), BarrierState::Done);
    }

    /// Shows a real layer surface, so it runs only inside an isolated
    /// compositor (`KOOHA_TEST_ISOLATED_COMPOSITOR=1`), never on the session.
    /// The presentation barrier needs a compositor that presents frames,
    /// which a virtual one may not; the real session checks it.
    #[cfg(feature = "layer-shell")]
    #[gtk::test]
    #[ignore = "needs an isolated Wayland compositor with layer-shell"]
    fn overlay_is_placed_on_isolated_compositor() {
        use std::time::Instant;

        use gtk4_layer_shell::{Edge, LayerShell};

        use crate::settings::test_utils::{settings_lock, setup_schema};

        // Fails instead of passing without having tested anything (fable
        // code r1 #9).
        assert_eq!(
            std::env::var("KOOHA_TEST_ISOLATED_COMPOSITOR").as_deref(),
            Ok("1"),
            "run it inside an isolated compositor, with KOOHA_TEST_ISOLATED_COMPOSITOR=1"
        );
        assert_ne!(
            std::env::var("WAYLAND_DISPLAY").as_deref(),
            Ok("wayland-0"),
            "never on the session compositor"
        );

        let _lock = settings_lock();
        setup_schema();
        gst::init().unwrap();

        let settings = Settings::default();
        for key in [
            "camera-enabled",
            "camera-shape",
            "camera-size",
            "camera-position",
        ] {
            settings.reset(key);
        }

        assert_eq!(overlay::availability(), Ok(()));

        let camera_pip = CameraPip::new(&settings);
        let (_, monitor) = monitors().into_iter().next().expect("a monitor");
        camera_pip.show_on(&monitor);
        let window = camera_pip.imp().window.borrow().clone().unwrap();

        let context = glib::MainContext::default();
        let started = Instant::now();
        while !window.is_mapped() && started.elapsed() < Duration::from_secs(3) {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(window.is_mapped());

        // Bottom-right by default.
        let expected = default_position((200, 200), monitor_size(&monitor));
        assert_eq!(
            (window.margin(Edge::Left), window.margin(Edge::Top)),
            expected
        );

        camera_pip.handle_drag_begin();
        camera_pip.handle_drag_update((5.0, -3.0));
        camera_pip.apply_drag_frame();
        assert_eq!(
            (window.margin(Edge::Left), window.margin(Edge::Top)),
            (expected.0 + 5, expected.1 - 3)
        );
        camera_pip.handle_drag_update((100_000.0, 100_000.0));
        camera_pip.handle_drag_end();
        let size = monitor_size(&monitor);
        assert_eq!(
            (window.margin(Edge::Left), window.margin(Edge::Top)),
            (size.0 - 200, size.1 - 200)
        );

        camera_pip.turn_off();
        assert!(camera_pip.imp().window.borrow().is_none());
        for key in [
            "camera-enabled",
            "camera-shape",
            "camera-size",
            "camera-position",
        ] {
            settings.reset(key);
        }
    }
}
