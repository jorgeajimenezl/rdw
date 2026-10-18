use super::*;
use crate::{display::DisplayExt, picture::Picture};
use glib::{clone, subclass::Signal, SourceId};
use gtk::{graphene, subclass::prelude::*};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::HashSet,
    sync::OnceLock,
    time::Duration,
};

#[cfg(all(feature = "wayland", not(feature = "bindings")))]
mod wayland;

#[cfg(all(windows, not(feature = "bindings")))]
mod win32;

unsafe impl ClassStruct for RdwDisplayClass {
    type Type = Display;
}

unsafe impl InstanceStruct for RdwDisplay {
    type Type = Display;
}

#[derive(Default)]
pub struct Display {
    pub(crate) picture: Picture,

    pub(crate) scaling: Cell<bool>,
    pub(crate) remote_resize: Cell<bool>,
    pub(crate) show_local_cursor: Cell<bool>,
    pub(crate) read_only: Cell<bool>,
    // The remote display size, ex: 1024x768
    pub(crate) last_resize_request: Cell<Option<(u32, u32, u32, u32)>>,
    pub(crate) resize_timeout_id: Cell<Option<SourceId>>,
    // The currently defined cursor
    pub(crate) cursor: RefCell<Option<gdk::Cursor>>,
    pub(crate) mouse_absolute: Cell<bool>,
    // position of cursor when drawn by client
    pub(crate) cursor_position: Cell<Option<(i32, i32)>>,
    // the remote hid its cursor (absolute mode: show none instead of the defined shape)
    pub(crate) cursor_hidden: Cell<bool>,
    // press-and-release detection time in ms
    pub(crate) synthesize_delay: Cell<u32>,
    pub(crate) last_key_press: Cell<Option<(gdk::Key, u32)>>,
    pub(crate) last_key_press_timeout: Cell<Option<SourceId>>,
    pub(crate) keys_pressed: RefCell<HashSet<(gdk::Key, u32)>>,

    // the shortcut to ungrab key/mouse (to be configurable and extended with ctrl-alt)
    pub(crate) grab_shortcut: OnceCell<gtk::ShortcutTrigger>,
    pub(crate) delayed_grab_shortcut: Cell<bool>,
    pub(crate) grabbed: Cell<Grab>,
    pub(crate) shortcuts_inhibited_id: Cell<Option<glib::SignalHandlerId>>,

    #[cfg(feature = "wayland")]
    wayland: wayland::Helper,

    #[cfg(windows)]
    win32: win32::Helper,
}

#[glib::object_subclass]
impl ObjectSubclass for Display {
    const NAME: &'static str = "RdwDisplay";
    type Type = super::Display;
    type ParentType = gtk::Widget;
    type Class = RdwDisplayClass;
    type Instance = RdwDisplay;

    fn class_init(_klass: &mut Self::Class) {
        // Load GL pointers from epoxy (GL context management library used by GTK).
        {
            #[cfg(target_os = "macos")]
            let library =
                unsafe { libloading::os::unix::Library::new("libepoxy.0.dylib") }.unwrap();
            #[cfg(all(unix, not(target_os = "macos")))]
            let library = unsafe { libloading::os::unix::Library::new("libepoxy.so.0") }.unwrap();
            #[cfg(windows)]
            let library = libloading::os::windows::Library::open_already_loaded("libepoxy-0.dll")
                .or_else(|_| libloading::os::windows::Library::open_already_loaded("epoxy-0.dll"))
                .unwrap();

            epoxy::load_with(|name| {
                unsafe { library.get::<_>(name.as_bytes()) }
                    .map(|symbol| *symbol)
                    .unwrap_or(std::ptr::null())
            });
            gl::load_with(epoxy::get_proc_addr);
            //assert_eq!(unsafe { gl::GetError() }, gl::NO_ERROR);
        }
    }
}

impl ObjectImpl for Display {
    fn constructed(&self) {
        self.parent_constructed();
        self.obj().set_focusable(true);
        self.picture.set_hexpand(true);
        self.picture.set_vexpand(true);
        self.picture.set_parent(self.obj().as_ref());

        self.grab_shortcut.get_or_init(|| {
            gtk::ShortcutTrigger::parse_string("<Ctrl>Alt_L|<Alt>Control_L").unwrap()
        });

        let ec = gtk::EventControllerFocus::new();
        ec.connect_leave(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or_panic]
            move |_ec| {
                this.delayed_grab_shortcut.set(false);
                this.release_keys();
            }
        ));
        self.obj().add_controller(ec);

        let ec = gtk::EventControllerKey::new();
        ec.set_propagation_phase(gtk::PropagationPhase::Capture);
        ec.connect_key_pressed(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or_panic]
            move |ec, keyval, keycode, _state| {
                this.key_pressed(ec, keyval, keycode);
                glib::Propagation::Stop
            }
        ));
        ec.connect_key_released(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or_panic]
            move |ec, keyval, keycode, _state| {
                this.key_released(ec, keyval, keycode);
            }
        ));
        self.obj().add_controller(ec);

        let ec = gtk::EventControllerMotion::new();
        ec.connect_motion(clone!(
            #[weak(rename_to = this)]
            self,
            move |_, x, y| this.do_motion(x, y)
        ));
        ec.connect_enter(clone!(
            #[weak(rename_to = this)]
            self,
            move |_, x, y| this.do_motion(x, y)
        ));
        ec.connect_leave(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                log::debug!("leave -> ungrab mouse");
                this.ungrab_mouse();
            }
        ));
        self.picture.add_controller(ec);

        let ec = gtk::GestureClick::new();
        ec.set_button(0);
        ec.connect_pressed(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or_panic]
            move |gesture, _n_press, x, y| {
                let grabbed = this.try_grab();

                if grabbed.contains(Grab::MOUSE) {
                    log::debug!("Skipping mouse-press, since we took the grab");
                    return;
                }

                let button = gesture.current_button();
                this.do_motion(x, y);
                this.do_mouse_press(button);
            }
        ));
        ec.connect_released(clone!(
            #[weak(rename_to = this)]
            self,
            move |gesture, _n_press, x, y| {
                let button = gesture.current_button();
                this.do_motion(x, y);
                this.do_mouse_release(button);
            }
        ));
        ec.connect_cancel(clone!(
            #[weak(rename_to = this)]
            self,
            move |gesture, _| {
                let button = gesture.current_button();
                this.do_mouse_release(button);
            }
        ));
        self.picture.add_controller(ec);

        let ec = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        ec.connect_scroll(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or_panic]
            move |_, dx, dy| {
                if dy >= 1.0 {
                    this.do_scroll_discrete(Scroll::Down);
                } else if dy <= -1.0 {
                    this.do_scroll_discrete(Scroll::Up);
                }
                if dx >= 1.0 {
                    this.do_scroll_discrete(Scroll::Right);
                } else if dx <= -1.0 {
                    this.do_scroll_discrete(Scroll::Left);
                }
                glib::Propagation::Proceed
            }
        ));
        self.picture.add_controller(ec);
    }

    fn dispose(&self) {
        #[cfg(feature = "wayland")]
        self.wayland.dispose();

        while let Some(child) = self.obj().first_child() {
            child.unparent();
        }
    }

    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
        PROPERTIES
            .get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<gtk::ShortcutTrigger>("grab-shortcut")
                        .nick("Grab shortcut")
                        .blurb("Input devices grab/ungrab shortcut")
                        .build(),
                    glib::ParamSpecFlags::builder::<Grab>("grabbed")
                        .nick("grabbed")
                        .blurb("Grabbed")
                        .read_only()
                        .explicit_notify()
                        .default_value(Grab::empty())
                        .build(),
                    glib::ParamSpecUInt::builder("synthesize-delay")
                        .nick("Synthesize delay")
                        .blurb("Press-and-release synthesize maximum time in ms")
                        .default_value(100)
                        .construct()
                        .build(),
                    glib::ParamSpecBoolean::builder("mouse-absolute")
                        .nick("Mouse absolute")
                        .blurb("Whether the mouse is absolute or relative")
                        .construct()
                        .build(),
                    glib::ParamSpecBoolean::builder("read-only")
                        .nick("Read-only")
                        .blurb("Do no send input events")
                        .explicit_notify()
                        .default_value(false)
                        .construct()
                        .build(),
                    glib::ParamSpecBoolean::builder("show-local-cursor")
                        .nick("Show local cursor")
                        .blurb("Show local cursor")
                        .explicit_notify()
                        .default_value(false)
                        .construct()
                        .build(),
                    glib::ParamSpecBoolean::builder("scaling")
                        .nick("Scaling")
                        .blurb("Scale display")
                        .explicit_notify()
                        .default_value(true)
                        .construct()
                        .build(),
                    glib::ParamSpecBoolean::builder("remote-resize")
                        .explicit_notify()
                        .default_value(true)
                        .construct()
                        .build(),
                ]
            })
            .as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "grab-shortcut" => {
                let shortcut = value.get().unwrap();
                self.grab_shortcut.set(shortcut).unwrap();
            }
            "synthesize-delay" => {
                let delay = value.get().unwrap();
                self.synthesize_delay.set(delay);
            }
            "mouse-absolute" => {
                let absolute = value.get().unwrap();
                if absolute {
                    self.ungrab_mouse();
                    self.update_cursor();
                }

                self.mouse_absolute.set(absolute);
            }
            "read-only" => {
                let ro = value.get().unwrap();
                self.set_read_only(ro)
            }
            "show-local-cursor" => {
                let val = value.get().unwrap();
                self.set_show_local_cursor(val)
            }
            "scaling" => {
                let val = value.get().unwrap();
                self.set_scaling(val)
            }
            "remote-resize" => {
                let val = value.get().unwrap();
                self.set_remote_resize(val)
            }
            _ => unimplemented!(),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "grab-shortcut" => self.grab_shortcut.get().to_value(),
            "grabbed" => self.grabbed.get().to_value(),
            "synthesize-delay" => self.synthesize_delay.get().to_value(),
            "mouse-absolute" => self.mouse_absolute().to_value(),
            "read-only" => self.read_only().to_value(),
            "show-local-cursor" => self.show_local_cursor().to_value(),
            "scaling" => self.scaling().to_value(),
            "remote-resize" => self.remote_resize().to_value(),
            _ => unimplemented!(),
        }
    }

    fn signals() -> &'static [Signal] {
        static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
        SIGNALS
            .get_or_init(|| {
                vec![
                    Signal::builder("key-event")
                        .param_types([
                            u32::static_type(),
                            u32::static_type(),
                            KeyEvent::static_type(),
                        ])
                        .build(),
                    Signal::builder("motion")
                        .param_types([f64::static_type(), f64::static_type()])
                        .build(),
                    Signal::builder("motion-relative")
                        .param_types([f64::static_type(), f64::static_type()])
                        .build(),
                    Signal::builder("mouse-press")
                        .param_types([u32::static_type()])
                        .build(),
                    Signal::builder("mouse-release")
                        .param_types([u32::static_type()])
                        .build(),
                    Signal::builder("scroll-discrete")
                        .param_types([Scroll::static_type()])
                        .build(),
                    Signal::builder("resize-request")
                        .param_types([
                            u32::static_type(),
                            u32::static_type(),
                            u32::static_type(),
                            u32::static_type(),
                        ])
                        .build(),
                ]
            })
            .as_ref()
    }
}

impl WidgetImpl for Display {
    fn realize(&self) {
        self.parent_realize();

        #[cfg(feature = "wayland")]
        if let Ok(dpy) = self.obj().display().downcast::<gdk_wl::WaylandDisplay>() {
            self.wayland.realize(&self.obj(), &dpy);
        }

        #[cfg(windows)]
        if let Ok(dpy) = self.obj().display().downcast::<gdk_win32::Win32Display>() {
            self.win32.realize(&self.obj(), &dpy);
        }
    }

    fn unrealize(&self) {
        match self.obj().display().backend() {
            #[cfg(feature = "wayland")]
            gdk::Backend::Wayland => self.wayland.unrealize(),
            #[cfg(windows)]
            gdk::Backend::Win32 => self.win32.unrealize(),
            _ => (),
        }

        self.parent_unrealize();
    }

    fn request_mode(&self) -> gtk::SizeRequestMode {
        gtk::SizeRequestMode::HeightForWidth
    }

    fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
        self.picture.measure(orientation, for_size)
    }

    fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
        self.parent_size_allocate(width, height, baseline);

        if let Some(timeout_id) = self.resize_timeout_id.take() {
            timeout_id.remove();
        }
        self.resize_timeout_id.set(Some(glib::timeout_add_local(
            Duration::from_millis(500),
            clone!(
                #[weak(rename_to = this)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    let sf = this.obj().scale_factor() as u32;
                    let width = width as u32 * sf;
                    let height = height as u32 * sf;
                    let (w_mm, h_mm) = this
                        .surface()
                        .as_ref()
                        .and_then(|s| {
                            gdk::prelude::DisplayExt::monitor_at_surface(&this.obj().display(), s)
                        })
                        .map(|m| {
                            let (geom, wmm, hmm) =
                                (m.geometry(), m.width_mm() as u32, m.height_mm() as u32);
                            (
                                wmm * width / (geom.width() as u32),
                                hmm * height / geom.height() as u32,
                            )
                        })
                        .unwrap_or((0u32, 0u32));
                    this.do_resize_request(width, height, w_mm, h_mm);
                    this.resize_timeout_id.set(None);
                    glib::ControlFlow::Break
                }
            ),
        )));

        let (x, y, w, h) = self.paintable_area();
        self.picture
            .size_allocate(&gtk::Allocation::new(x, y, w, h), -1);
    }

    fn snapshot(&self, snapshot: &gtk::Snapshot) {
        self.obj().snapshot_child(&self.picture, snapshot);

        if self.obj().mouse_absolute() {
            return;
        }
        if !self.grabbed.get().contains(Grab::MOUSE) {
            return;
        }
        let Some(pos) = self.cursor_position.get() else {
            return;
        };
        let Some(cursor) = &*self.cursor.borrow() else {
            return;
        };
        let Some(texture) = cursor.texture() else {
            return;
        };
        let (x, y) = self.transform_pos_inv(
            // do not take hotspot into account, at least for qemu.. to check with spice/vnc/rdp..
            (pos.0, pos.1),
        );

        let sf = self.obj().scale_factor();

        snapshot.append_texture(
            &texture,
            &graphene::Rect::new(
                x as f32,
                y as f32,
                (texture.width() / sf) as f32,
                (texture.height() / sf) as f32,
            ),
        );
    }

    fn grab_focus(&self) -> bool {
        self.picture.grab_focus()
    }
}

impl Display {
    fn paintable_area(&self) -> (i32, i32, i32, i32) {
        let (width, height) = (self.obj().width() as f64, self.obj().height() as f64);
        let display_ratio = width / height;
        let ratio = self.picture.paintable().intrinsic_aspect_ratio();

        let (w, h) = if ratio > display_ratio {
            (width, width / ratio)
        } else {
            (height * ratio, height)
        };

        let x = (width - w.ceil()) / 2.0;
        let y = (height - h.ceil()).floor() / 2.0;
        (x as _, y as _, w as _, h as _)
    }

    fn set_scaling(&self, scaling: bool) {
        if scaling == self.scaling() {
            return;
        }
        if scaling {
            self.obj().set_size_request(-1, -1);
        } else if let Some((width, height)) = self.obj().display_size() {
            self.obj().set_size_request(
                width as i32 / self.obj().scale_factor(),
                height as i32 / self.obj().scale_factor(),
            );
        }
        self.scaling.set(scaling);
        self.obj().notify("scaling");
        self.obj().queue_resize();
    }

    fn scaling(&self) -> bool {
        self.scaling.get()
    }

    fn set_remote_resize(&self, remote_resize: bool) {
        let old_value = self.remote_resize.replace(remote_resize);
        if old_value != remote_resize {
            self.obj().notify("remote-resize");
        }
    }

    fn remote_resize(&self) -> bool {
        self.remote_resize.get()
    }

    pub(crate) fn update_cursor(&self) {
        if self.read_only() || self.show_local_cursor() {
            self.picture.set_cursor(None);
        } else if self.mouse_absolute() {
            let cursor = self.cursor.borrow();
            if self.cursor_hidden.get() && cursor.is_some() {
                self.picture.set_cursor_from_name(Some("none"));
            } else {
                self.picture.set_cursor(cursor.as_ref());
            }
        } else if self.grabbed.get().contains(Grab::MOUSE) {
            self.picture.set_cursor_from_name(Some("none"));
        } else {
            self.picture.set_cursor(None);
        }
        self.obj().queue_draw();
    }

    fn set_show_local_cursor(&self, show: bool) {
        if show == self.show_local_cursor() {
            return;
        }
        self.show_local_cursor.set(show);
        self.update_cursor();
        self.obj().notify("show-local-cursor");
    }

    fn show_local_cursor(&self) -> bool {
        self.show_local_cursor.get()
    }

    fn set_read_only(&self, ro: bool) {
        if ro == self.read_only() {
            return;
        }
        if ro {
            self.ungrab();
        }
        self.read_only.set(ro);
        self.update_cursor();
        self.obj().notify("read-only");
    }

    fn read_only(&self) -> bool {
        self.read_only.get()
    }

    fn mouse_absolute(&self) -> bool {
        self.mouse_absolute.get()
    }

    fn do_motion(&self, x: f64, y: f64) {
        if self.read_only() {
            return;
        }
        let (pw, ph) = self.picture.paintable().size();
        let (_, _, w, h) = self.paintable_area();
        let x = x * pw as f64 / w as f64;
        let y = y * ph as f64 / h as f64;
        self.obj().emit_by_name::<()>("motion", &[&x, &y]);
    }

    #[cfg(any(feature = "wayland", windows))]
    pub(crate) fn do_motion_relative(&self, dx: f64, dy: f64) {
        if self.read_only() {
            return;
        }
        self.obj()
            .emit_by_name::<()>("motion-relative", &[&dx, &dy]);
    }

    fn do_mouse_press(&self, button: u32) {
        if self.read_only() {
            return;
        }
        self.obj().emit_by_name::<()>("mouse-press", &[&button])
    }

    fn do_mouse_release(&self, button: u32) {
        if self.read_only() {
            return;
        }
        self.obj().emit_by_name::<()>("mouse-release", &[&button])
    }

    fn do_scroll_discrete(&self, dir: Scroll) {
        self.obj().emit_by_name::<()>("scroll-discrete", &[&dir])
    }

    pub(crate) fn do_key_press(&self, keyval: gdk::Key, keycode: u32) {
        if self.read_only() {
            return;
        }
        self.obj()
            .emit_by_name::<()>("key-event", &[&keyval, &keycode, &KeyEvent::PRESS])
    }

    pub(crate) fn do_key_release(&self, keyval: gdk::Key, keycode: u32) {
        if self.read_only() {
            return;
        }
        self.obj()
            .emit_by_name::<()>("key-event", &[&keyval, &keycode, &KeyEvent::RELEASE])
    }

    pub(crate) fn do_key_press_and_release(&self, keyval: gdk::Key, keycode: u32) {
        if self.read_only() {
            return;
        }
        self.obj().emit_by_name::<()>(
            "key-event",
            &[&keyval, &keycode, &(KeyEvent::PRESS | KeyEvent::RELEASE)],
        )
    }

    fn do_resize_request(&self, width: u32, height: u32, w_mm: u32, h_mm: u32) {
        // Only process the resize-request if not read-only and remote resizing is enabled.
        if self.read_only() || !self.remote_resize() {
            return;
        }

        let req = Some((width, height, w_mm, h_mm));
        if req == self.last_resize_request.get() {
            return;
        }
        self.last_resize_request.set(req);

        self.obj()
            .emit_by_name::<()>("resize-request", &[&width, &height, &w_mm, &h_mm]);
    }

    fn ungrab_keyboard(&self) {
        if !self.grabbed.get().contains(Grab::KEYBOARD) {
            return;
        }

        if let Some(toplevel) = self.toplevel() {
            toplevel.restore_system_shortcuts();
            #[cfg(windows)]
            self.win32.ungrab_keyboard();
            self.grabbed.set(self.grabbed.get() - Grab::KEYBOARD);
            self.obj().notify("grabbed");
        }
    }

    pub(crate) fn ungrab_mouse(&self) {
        if self.grabbed.get().contains(Grab::MOUSE) {
            #[cfg(feature = "wayland")]
            self.wayland.ungrab_mouse();

            #[cfg(windows)]
            self.win32.ungrab_mouse();
            self.restore_accel_mouse();

            self.grabbed.set(self.grabbed.get() - Grab::MOUSE);
            if !self.obj().mouse_absolute() {
                self.picture.set_cursor(None);
            }
            self.obj().queue_draw(); // update cursor
            self.obj().notify("grabbed");
        }
    }

    fn clear_last_key_press(&self) {
        self.last_key_press.set(None);
        if let Some(timeout_id) = self.last_key_press_timeout.take() {
            timeout_id.remove();
        }
    }

    fn release_keys(&self) {
        self.clear_last_key_press();
        for (keyval, keycode) in self.keys_pressed.take() {
            self.keys_pressed.borrow_mut().remove(&(keyval, keycode));
            self.do_key_release(keyval, keycode)
        }
        self.keys_pressed.borrow_mut().clear();
    }

    fn emit_last_key_press(&self) {
        if let Some((keyval, keycode)) = self.last_key_press.take() {
            self.keys_pressed.borrow_mut().insert((keyval, keycode));
            self.do_key_press(keyval, keycode)
        }

        self.clear_last_key_press();
    }

    pub(crate) fn ungrab(&self) {
        self.ungrab_keyboard();
        self.ungrab_mouse();
    }

    fn key_pressed(&self, ec: &gtk::EventControllerKey, keyval: gdk::Key, keycode: u32) {
        self.delayed_grab_shortcut.set(false);
        if let Some(ref e) = ec.current_event() {
            if self.grab_shortcut.get().unwrap().trigger(e, false) == gdk::KeyMatch::Exact {
                self.delayed_grab_shortcut.set(true);
            }
        }

        // flush pending key event
        self.emit_last_key_press();

        // synthesize press-and-release if within the synthesize-delay boundary, else emit
        self.last_key_press.set(Some((keyval, keycode)));
        self.last_key_press_timeout
            .set(Some(glib::timeout_add_local(
                Duration::from_millis(self.synthesize_delay.get() as _),
                glib::clone!(
                    #[weak(rename_to = this)]
                    self,
                    #[upgrade_or]
                    glib::ControlFlow::Break,
                    move || {
                        this.emit_last_key_press();
                        glib::ControlFlow::Break
                    }
                ),
            )));
    }

    fn key_released(&self, _ec: &gtk::EventControllerKey, keyval: gdk::Key, keycode: u32) {
        // It also gets releases of keys whose press it never saw (ex: ctrl-alt-down, the down
        // press taken by the compositor or an ancestor): those neither end the grab shortcut
        // nor reach the guest.
        let seen = self.last_key_press.get() == Some((keyval, keycode))
            || self.keys_pressed.borrow().contains(&(keyval, keycode));
        if self.delayed_grab_shortcut.take() && seen {
            if self.grabbed.get().is_empty() {
                self.try_grab();
            } else {
                self.ungrab();
            }
        }

        let mut emitted = false;
        if let Some((last_keyval, last_keycode)) = self.last_key_press.get() {
            if (last_keyval, last_keycode) == (keyval, keycode) {
                self.clear_last_key_press();
                self.do_key_press_and_release(keyval, keycode);
                emitted = true;
            }
        }

        // flush pending key event
        self.emit_last_key_press();

        self.keys_pressed.borrow_mut().remove(&(keyval, keycode));
        if seen && !emitted {
            self.do_key_release(keyval, keycode)
        }
    }

    fn try_grab_keyboard(&self) -> bool {
        if self.grabbed.get().contains(Grab::KEYBOARD) {
            return true;
        }

        let Some(toplevel) = self.toplevel() else {
            return false;
        };

        toplevel.inhibit_system_shortcuts(None::<&gdk::ButtonEvent>);
        // Apparently, inhibit-system is not implemented on win32 yet
        #[cfg(windows)]
        self.win32.grab_keyboard();

        let id = toplevel.connect_shortcuts_inhibited_notify(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or_panic]
            move |toplevel| {
                let inhibited = toplevel.is_shortcuts_inhibited();
                log::debug!("shortcuts-inhibited: {inhibited}");
                if !inhibited {
                    let id = this.shortcuts_inhibited_id.take();
                    toplevel.disconnect(id.unwrap());
                    this.ungrab_keyboard();
                }
            }
        ));
        self.shortcuts_inhibited_id.set(Some(id));
        true
    }

    fn try_grab_device(&self, _device: gdk::Device) -> bool {
        #[cfg(feature = "wayland")]
        return self.wayland.try_grab_device(&self.obj(), _device);

        #[cfg(windows)]
        return self.win32.try_grab_device(&self.obj(), _device);

        #[cfg(not(any(feature = "wayland", windows)))]
        false
    }

    fn try_grab_mouse(&self) -> bool {
        if self.obj().mouse_absolute() {
            // we could eventually grab the mouse in client mode, but what's the point?
            return false;
        }
        if self.obj().grabbed().contains(Grab::MOUSE) {
            return true;
        }

        if let Some(default_seat) = gdk::prelude::DisplayExt::default_seat(&self.obj().display()) {
            for device in default_seat.devices(gdk::SeatCapabilities::POINTER) {
                if !self.try_grab_device(device) {
                    return false;
                }
            }
        }

        self.save_accel_mouse();

        true
    }

    fn save_accel_mouse(&self) {
        #[cfg(windows)]
        self.win32.save_accel_mouse()
        // todo
    }

    fn restore_accel_mouse(&self) {
        #[cfg(windows)]
        self.win32.restore_accel_mouse()
        // todo
    }

    pub(crate) fn try_grab(&self) -> Grab {
        let mut grabbed = Default::default();
        self.picture.grab_focus();
        if self.try_grab_keyboard() {
            grabbed |= Grab::KEYBOARD;
        }
        if self.try_grab_mouse() {
            grabbed |= Grab::MOUSE;
        }
        self.grabbed.set(self.obj().grabbed() | grabbed);
        self.obj().notify("grabbed");
        if grabbed.contains(Grab::MOUSE) {
            self.update_cursor();
        }
        grabbed
    }

    // remote display pos -> widget pos
    fn transform_pos_inv(&self, pos: (i32, i32)) -> (f64, f64) {
        let (px, py, pw, ph) = self.paintable_area();
        let (w, h) = self.picture.paintable().size();
        let x = pos.0 as f64 * (pw as f64 / w as f64) + px as f64;
        let y = pos.1 as f64 * (ph as f64 / h as f64) + py as f64;
        (x, y)
    }

    fn toplevel(&self) -> Option<gdk::Toplevel> {
        self.obj()
            .root()
            .and_then(|r| r.native())
            .and_then(|n| n.surface())
            .and_then(|s| s.downcast::<gdk::Toplevel>().ok())
    }

    fn surface(&self) -> Option<gdk::Surface> {
        self.obj().native().and_then(|n| n.surface())
    }

    #[cfg(windows)]
    fn win32_handle(&self) -> Option<gdk_win32::HWND> {
        self.surface()
            .and_then(|s| s.downcast::<gdk_win32::Win32Surface>().ok())
            .map(|w| w.handle())
    }

    #[cfg(feature = "wayland")]
    fn wl_surface(&self) -> Option<gdk_wl::wayland_client::protocol::wl_surface::WlSurface> {
        use gdk_wl::prelude::WaylandSurfaceExtManual;

        self.surface()
            .and_then(|s| s.downcast::<gdk_wl::WaylandSurface>().ok())
            .and_then(|w| w.wl_surface())
    }
}
