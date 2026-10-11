use crate::{audio, clipboard, cursor};
use futures_util::StreamExt;
use glib::{clone, subclass::prelude::*, MainContext};
use gtk::glib;
#[cfg(windows)]
use qemu_display::ConsoleListenerD3d11Handler;
#[cfg(any(windows, unix))]
use qemu_display::ConsoleListenerMapHandler;
#[cfg(unix)]
use qemu_display::ConsoleListenerScanoutDmabuf2Handler;
use qemu_display::{Console, ConsoleListenerHandler};
use rdw::{gtk, DisplayExt};
use std::cell::Cell;
use std::sync::OnceLock;
use zbus::names::BusName;
use zbus::Connection;

const XRGB_FORMAT: pixman_sys::pixman_format_code_t =
    pixman_sys::pixman_format_code_t_PIXMAN_x8r8g8b8;

mod imp {
    use super::*;
    use gtk::subclass::prelude::*;
    use qemu_display::ScanoutMmap;
    #[cfg(any(windows, unix))]
    use std::cell::RefCell;

    #[repr(C)]
    pub struct RdwQemuDisplayClass {
        pub parent_class: rdw::RdwDisplayClass,
    }

    unsafe impl ClassStruct for RdwQemuDisplayClass {
        type Type = Display;
    }

    #[repr(C)]
    pub struct RdwQemuDisplay {
        parent: rdw::RdwDisplay,
    }

    impl std::fmt::Debug for RdwQemuDisplay {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.debug_struct("RdwQemuDisplay")
                .field("parent", &self.parent)
                .finish()
        }
    }

    unsafe impl InstanceStruct for RdwQemuDisplay {
        type Type = Display;
    }

    #[derive(Debug, Default)]
    pub struct Display {
        pub(crate) console: OnceLock<Console>,
        keymap: Cell<Option<&'static [u16]>>,
        scanout_map: RefCell<Option<ScanoutMmap>>,
        #[allow(unused)] // only here to keep it alive
        pub(crate) audio_handler: RefCell<Option<audio::Handler>>,
        #[allow(unused)] // only here to keep it alive
        pub(crate) clipboard_handler: RefCell<Option<clipboard::Handler>>,
        // last absolute position sent to the guest
        host_pos: Cell<Option<(i32, i32)>>,
        // hot spot of the guest's current cursor
        cursor_hot: Cell<(i32, i32)>,
        // pending hide of a cursor parked at the origin (see cursor.rs)
        cursor_hide: RefCell<Option<glib::SourceId>>,
        // the guest cursor is hidden (as last told to rdw)
        cursor_hidden: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Display {
        const NAME: &'static str = "RdwQemuDisplay";
        type Type = super::Display;
        type ParentType = rdw::Display;
        type Class = RdwQemuDisplayClass;
        type Instance = RdwQemuDisplay;
    }

    impl ObjectImpl for Display {
        fn constructed(&self) {
            self.parent_constructed();

            self.obj().set_mouse_absolute(false);

            self.obj().connect_key_event(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, keyval, keycode, event| {
                    let mapped = this
                        .keymap
                        .get()
                        .and_then(|m| m.get(keycode as usize))
                        .map(|x| *x as u32);
                    log::debug!("key-{event:?}: {keyval} {keycode} -> {mapped:?}");
                    if let Some(qnum) = mapped {
                        MainContext::default().spawn_local(clone!(
                            #[weak]
                            this,
                            async move {
                                if event.contains(rdw::KeyEvent::PRESS) {
                                    let _ = this.obj().console().keyboard.press(qnum).await;
                                }
                                if event.contains(rdw::KeyEvent::RELEASE) {
                                    let _ = this.obj().console().keyboard.release(qnum).await;
                                }
                            }
                        ));
                    }
                }
            ));

            self.obj().connect_motion(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, x, y| {
                    log::trace!("motion: {:?}", (x, y));
                    this.host_pos.set(Some((x as _, y as _)));
                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            if !this
                                .obj()
                                .console()
                                .mouse
                                .is_absolute()
                                .await
                                .unwrap_or(false)
                            {
                                return;
                            }
                            if let Err(e) = this
                                .obj()
                                .console()
                                .mouse
                                .set_abs_position(x as _, y as _)
                                .await
                            {
                                log::warn!("{e}");
                            }
                        }
                    ));
                }
            ));

            self.obj().connect_motion_relative(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, dx, dy| {
                    log::trace!("motion-relative: {:?}", (dx, dy));
                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            let _ = this
                                .obj()
                                .console()
                                .mouse
                                .rel_motion(dx.round() as _, dy.round() as _)
                                .await;
                        }
                    ));
                }
            ));

            self.obj().connect_mouse_press(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, button| {
                    log::debug!("mouse-press: {:?}", button);
                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            let button = from_gdk_button(button);
                            let _ = this.obj().console().mouse.press(button).await;
                        }
                    ));
                }
            ));

            self.obj().connect_mouse_release(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, button| {
                    log::debug!("mouse-release: {:?}", button);
                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            let button = from_gdk_button(button);
                            let _ = this.obj().console().mouse.release(button).await;
                        }
                    ));
                }
            ));

            self.obj().connect_scroll_discrete(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, scroll| {
                    use qemu_display::MouseButton;

                    log::debug!("scroll-discrete: {:?}", scroll);

                    let button = match scroll {
                        rdw::Scroll::Up => MouseButton::WheelUp,
                        rdw::Scroll::Down => MouseButton::WheelDown,
                        _ => {
                            log::warn!("not yet implemented");
                            return;
                        }
                    };
                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            let _ = this.obj().console().mouse.press(button).await;
                            let _ = this.obj().console().mouse.release(button).await;
                        }
                    ));
                }
            ));

            self.obj().connect_resize_request(clone!(
                #[weak(rename_to = this)]
                self,
                move |_, width, height, wmm, hmm| {
                    log::debug!("resize-request: {:?}", (width, height, wmm, hmm));
                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            let _ = this
                                .obj()
                                .console()
                                .proxy
                                .set_ui_info(wmm as _, hmm as _, 0, 0, width, height)
                                .await;
                        }
                    ));
                }
            ));
        }
    }

    impl Display {
        fn set_cursor_visible(&self, visible: bool) {
            if self.cursor_hidden.replace(!visible) == visible {
                log::debug!("guest cursor {}", if visible { "shown" } else { "hidden" });
            }
            self.obj().set_cursor_visible(visible);
        }

        fn update_cursor_visibility(&self, v: cursor::Visibility) {
            use cursor::Visibility::*;
            match v {
                Show | Hide => {
                    if let Some(id) = self.cursor_hide.take() {
                        id.remove();
                    }
                    self.set_cursor_visible(v == Show);
                }
                HideSoon => {
                    if self.cursor_hide.borrow().is_some() {
                        return;
                    }
                    let id = glib::timeout_add_local_once(
                        cursor::PARK_GRACE,
                        clone!(
                            #[weak(rename_to = this)]
                            self,
                            move || {
                                // Fired: forget the id (removing it now would fail).
                                this.cursor_hide.take();
                                log::debug!("guest cursor parked at the origin");
                                this.set_cursor_visible(false);
                            }
                        ),
                    );
                    self.cursor_hide.replace(Some(id));
                }
            }
        }
    }

    impl WidgetImpl for Display {
        fn realize(&self) {
            self.parent_realize();

            self.keymap.set(rdw::keymap_qnum());

            MainContext::default().spawn_local(clone!(
                #[weak(rename_to = this)]
                self,
                async move {
                    let console = this.console.get().unwrap();
                    // we have to use a channel, because widget is not Send..
                    let (sender, mut receiver) = futures::channel::mpsc::unbounded();
                    let handler = ConsoleHandler { sender };
                    console.register_listener(handler.clone()).await.unwrap();
                    #[cfg(any(windows, unix))]
                    console.set_map_listener(handler.clone()).await.unwrap();
                    #[cfg(windows)]
                    console.set_d3d11_listener(handler.clone()).await.unwrap();
                    #[cfg(unix)]
                    console
                        .set_scanout_dmabuf2_listener(handler.clone())
                        .await
                        .unwrap();

                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            while let Some(e) = receiver.next().await {
                                use ConsoleEvent::*;
                                match e {
                                    Scanout(s) => {
                                        if s.format != XRGB_FORMAT {
                                            log::warn!("Format not yet supported: {:X}", s.format);
                                            continue;
                                        }
                                        this.obj().set_display_pixel_format(rdw::PixelFormat::Bgrx);
                                        this.obj()
                                            .set_display_size(Some((s.width as _, s.height as _)));
                                        this.obj().update_area(
                                            0,
                                            0,
                                            s.width as _,
                                            s.height as _,
                                            s.stride as _,
                                            Some(&s.data),
                                        );
                                    }
                                    Update(u) => {
                                        if u.format != XRGB_FORMAT {
                                            log::warn!("Format not yet supported: {:X}", u.format);
                                            continue;
                                        }
                                        this.obj().update_area(
                                            u.x as _,
                                            u.y as _,
                                            u.w as _,
                                            u.h as _,
                                            u.stride as _,
                                            Some(&u.data),
                                        );
                                    }
                                    ScanoutMap { scanout, wait_tx } => {
                                        log::debug!("{scanout:?}");
                                        if scanout.format != XRGB_FORMAT {
                                            log::warn!(
                                                "Format not yet supported: {:X}",
                                                scanout.format
                                            );
                                            continue;
                                        }
                                        this.obj().set_display_pixel_format(rdw::PixelFormat::Bgrx);
                                        this.obj().set_display_size(Some((
                                            scanout.width as _,
                                            scanout.height as _,
                                        )));
                                        let map = match scanout.mmap() {
                                            Ok(map) => Some(map),
                                            Err(err) => {
                                                log::warn!("Failed to mmap: {}", err);
                                                continue;
                                            }
                                        };
                                        this.scanout_map.replace(map);
                                        let _ = wait_tx.send(());
                                    }
                                    UpdateMap(u) => {
                                        log::debug!("{u:?}");
                                        let scanout_map = this.scanout_map.borrow();
                                        let Some(map) = scanout_map.as_ref() else {
                                            log::warn!("No mapped scanout!");
                                            continue;
                                        };
                                        let bytes = map.as_ref();
                                        this.obj().update_area(
                                            u.x as _,
                                            u.y as _,
                                            u.w as _,
                                            u.h as _,
                                            map.stride() as _,
                                            Some(
                                                &bytes[u.y as usize * map.stride() as usize
                                                    + u.x as usize * 4..],
                                            ),
                                        );
                                    }
                                    #[cfg(windows)]
                                    ScanoutD3dTexture2d(s) => {
                                        log::debug!("{s:?}");
                                        this.obj().set_display_size(Some((s.w as _, s.h as _)));
                                        this.obj().set_d3d11_texture2d_scanout(Some(
                                            rdw::RdwD3d11Texture2dScanout {
                                                handle: s.handle as _,
                                                tex_width: s.tex_width,
                                                tex_height: s.tex_height,
                                                y0_top: s.y0_top,
                                                x: s.x,
                                                y: s.y,
                                                w: s.w,
                                                h: s.h,
                                            },
                                        ));
                                    }
                                    #[cfg(windows)]
                                    UpdateD3dTexture2d { wait_tx, update } => {
                                        this.obj().set_d3d11_texture2d_can_acquire(true);
                                        this.obj().update_area(
                                            update.x, update.y, update.w, update.h, 0, None,
                                        );
                                        this.obj().set_d3d11_texture2d_can_acquire(false);
                                        let _ = wait_tx.send(());
                                    }
                                    #[cfg(unix)]
                                    ScanoutDMABUF(s) => {
                                        log::trace!("{s:?}");
                                        this.obj()
                                            .set_display_size(Some((s.width as _, s.height as _)));
                                        this.obj().set_dmabuf_scanout(rdw::RdwDmabufScanout {
                                            width: s.width,
                                            height: s.height,
                                            offset: s.offset,
                                            stride: s.stride,
                                            num_planes: s.num_planes,
                                            fourcc: s.fourcc,
                                            y0_top: s.y0_top,
                                            modifier: s.modifier,
                                            fd: s.into_raw_fds(),
                                        });
                                    }
                                    #[cfg(unix)]
                                    UpdateDMABUF { wait_tx, update } => {
                                        this.obj().update_area(
                                            update.x, update.y, update.w, update.h, 0, None,
                                        );
                                        let _ = wait_tx.send(());
                                    }
                                    Disable => {
                                        log::warn!("Display disabled");
                                    }
                                    Disconnected => {
                                        log::warn!("Console disconnected");
                                    }
                                    CursorDefine(c) => {
                                        log::debug!("{c:?}");
                                        let cursor = rdw::Display::make_cursor(
                                            &c.data, c.width, c.height, c.hot_x, c.hot_y, 1,
                                        );
                                        this.obj().define_cursor(Some(cursor));
                                        this.cursor_hot.set((c.hot_x, c.hot_y));
                                    }
                                    MouseSet(m) => {
                                        log::trace!("{m:?}");
                                        if m.on != 0 {
                                            this.obj()
                                                .set_cursor_position(Some((m.x as _, m.y as _)));
                                        } else {
                                            this.obj().set_cursor_position(None);
                                        }
                                        this.update_cursor_visibility(cursor::on_mouse_set(
                                            m.x,
                                            m.y,
                                            m.on != 0,
                                            this.host_pos.get(),
                                            this.cursor_hot.get(),
                                        ));
                                    }
                                }
                            }
                        }
                    ));
                    let mut abs_changed = console.mouse.receive_is_absolute_changed().await;
                    this.obj()
                        .set_mouse_absolute(console.mouse.is_absolute().await.unwrap_or(false));
                    MainContext::default().spawn_local(clone!(
                        #[weak]
                        this,
                        async move {
                            while let Some(abs) = abs_changed.next().await {
                                if let Ok(abs) = abs.get().await {
                                    this.obj().set_mouse_absolute(abs);
                                }
                            }
                        }
                    ));
                }
            ));
        }
    }

    impl rdw::DisplayImpl for Display {}
}

glib::wrapper! {
    pub struct Display(ObjectSubclass<imp::Display>) @extends rdw::Display, gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Display {
    /// Create a new [`Display`]. This sets up the inner [`qemu_display::Display`] and
    /// clipboard + audio handling.
    ///
    /// The `on_disconnected` callback gets called when the display is disconnected via a D-Bus
    /// owner change.
    /// If listening for that owner change raises an error, `Err` is passed to the callback.
    pub async fn new<D, C>(
        conn: &Connection,
        dest: Option<D>,
        #[cfg(windows)] peer_pid: u32,
        on_disconnected: Option<C>,
    ) -> Result<Self, qemu_display::Error>
    where
        D: TryInto<BusName<'static>>,
        D::Error: Into<qemu_display::Error>,
        C: FnOnce(Result<(), qemu_display::Error>) + 'static,
    {
        let q_display = qemu_display::Display::new(
            conn,
            dest,
            #[cfg(windows)]
            peer_pid,
        )
        .await?;

        Self::new_with_qemu_display(&q_display, on_disconnected).await
    }

    /// Create a new [`Display`] from an existing [`qemu_display::Display`].
    /// This also sets up clipboard + audio handling.
    ///
    /// The `on_disconnected` callback gets called when the display is disconnected via a D-Bus
    /// owner change.
    /// If listening for that owner change raises an error, `Err` is passed to the callback.
    pub async fn new_with_qemu_display<C>(
        q_display: &qemu_display::Display<'static>,
        on_disconnected: Option<C>,
    ) -> Result<Self, qemu_display::Error>
    where
        C: FnOnce(Result<(), qemu_display::Error>) + 'static,
    {
        if let Some(on_disconnected) = on_disconnected {
            glib::spawn_future_local(glib::clone!(
                #[strong]
                q_display,
                async move {
                    match q_display.receive_owner_changed().await {
                        Ok(mut changed) => {
                            let _ = changed.next().await;
                            log::debug!("disconnected via owner change");
                            on_disconnected(Ok(()));
                        }
                        Err(err) => {
                            log::warn!("owner change raised error: {}", err);
                            on_disconnected(Err(err));
                        }
                    };
                }
            ));
        }

        let console = Console::new(
            q_display.connection(),
            0,
            #[cfg(windows)]
            q_display.peer_pid(),
        )
        .await?;

        let slf = Display::new_for_console(console);

        let audio_handler = if let Ok(Some(audio)) = q_display.audio().await {
            audio::Handler::new(audio)
                .await
                .inspect_err(|e| log::warn!("Failed to setup audio handler: {}", e))
                .ok()
        } else {
            None
        };

        let clipboard_handler = if let Ok(Some(clipboard)) = q_display.clipboard().await {
            clipboard::Handler::new(clipboard)
                .await
                .inspect_err(|e| log::warn!("Failed to setup clipboard handler: {}", e))
                .ok()
        } else {
            None
        };

        let imp = slf.imp();

        imp.audio_handler.replace(audio_handler);
        imp.clipboard_handler.replace(clipboard_handler);

        Ok(slf)
    }

    /// Create a new display from an existing [`Console`].
    /// This will only set up the display itself, not auxilary components such as clipboard
    /// or audio handling. To have [`Display`] set up everything, use [`Self::new`].
    pub fn new_for_console(console: Console) -> Self {
        let obj = glib::Object::builder().build();
        let self_ = imp::Display::from_obj(&obj);
        self_.console.set(console).unwrap();
        obj
    }

    pub(crate) fn console(&self) -> &Console {
        let self_ = imp::Display::from_obj(self);
        self_.console.get().unwrap()
    }
}

#[derive(Debug)]
enum ConsoleEvent {
    Scanout(qemu_display::Scanout),
    Update(qemu_display::Update),
    #[cfg(any(windows, unix))]
    ScanoutMap {
        scanout: qemu_display::ScanoutMap,
        wait_tx: futures::channel::oneshot::Sender<()>,
    },
    #[cfg(any(windows, unix))]
    UpdateMap(qemu_display::UpdateMap),
    #[cfg(windows)]
    ScanoutD3dTexture2d(qemu_display::ScanoutD3dTexture2d),
    #[cfg(windows)]
    UpdateD3dTexture2d {
        update: qemu_display::UpdateD3dTexture2d,
        wait_tx: futures::channel::oneshot::Sender<()>,
    },
    #[cfg(unix)]
    ScanoutDMABUF(qemu_display::ScanoutDMABUF),
    #[cfg(unix)]
    UpdateDMABUF {
        update: qemu_display::UpdateDMABUF,
        wait_tx: futures::channel::oneshot::Sender<()>,
    },
    Disable,
    MouseSet(qemu_display::MouseSet),
    CursorDefine(qemu_display::Cursor),
    Disconnected,
}

#[derive(Clone)]
struct ConsoleHandler {
    sender: futures::channel::mpsc::UnboundedSender<ConsoleEvent>,
}

impl ConsoleHandler {
    fn send(&self, event: ConsoleEvent) {
        if let Err(e) = self.sender.unbounded_send(event) {
            log::warn!("failed to send console event: {}", e);
        }
    }
}

#[cfg(any(windows, unix))]
#[async_trait::async_trait]
impl ConsoleListenerMapHandler for ConsoleHandler {
    async fn scanout_map(&mut self, scanout: qemu_display::ScanoutMap) {
        let (wait_tx, wait_rx) = futures::channel::oneshot::channel();
        self.send(ConsoleEvent::ScanoutMap { scanout, wait_tx });
        if let Err(e) = wait_rx.await {
            log::warn!("wait ScanoutMap failed: {}", e);
        }
    }

    async fn update_map(&mut self, update: qemu_display::UpdateMap) {
        self.send(ConsoleEvent::UpdateMap(update));
    }
}

#[cfg(windows)]
#[async_trait::async_trait]
impl ConsoleListenerD3d11Handler for ConsoleHandler {
    async fn scanout_texture2d(&mut self, scanout: qemu_display::ScanoutD3dTexture2d) {
        self.send(ConsoleEvent::ScanoutD3dTexture2d(scanout));
    }

    async fn update_texture2d(&mut self, update: qemu_display::UpdateD3dTexture2d) {
        let (wait_tx, wait_rx) = futures::channel::oneshot::channel();
        self.send(ConsoleEvent::UpdateD3dTexture2d { update, wait_tx });
        if let Err(e) = wait_rx.await {
            log::warn!("wait update d3d texture2d failed: {}", e);
        }
    }
}

#[async_trait::async_trait]
impl ConsoleListenerHandler for ConsoleHandler {
    async fn scanout(&mut self, scanout: qemu_display::Scanout) {
        self.send(ConsoleEvent::Scanout(scanout));
    }

    async fn update(&mut self, update: qemu_display::Update) {
        self.send(ConsoleEvent::Update(update));
    }

    #[cfg(unix)]
    async fn scanout_dmabuf(&mut self, scanout: qemu_display::ScanoutDMABUF) {
        self.send(ConsoleEvent::ScanoutDMABUF(scanout));
    }

    #[cfg(unix)]
    async fn update_dmabuf(&mut self, update: qemu_display::UpdateDMABUF) {
        let (wait_tx, wait_rx) = futures::channel::oneshot::channel();
        self.send(ConsoleEvent::UpdateDMABUF { update, wait_tx });
        if let Err(e) = wait_rx.await {
            log::warn!("wait update dmabuf failed: {}", e);
        }
    }

    async fn disable(&mut self) {
        self.send(ConsoleEvent::Disable);
    }

    async fn mouse_set(&mut self, set: qemu_display::MouseSet) {
        self.send(ConsoleEvent::MouseSet(set));
    }

    async fn cursor_define(&mut self, cursor: qemu_display::Cursor) {
        self.send(ConsoleEvent::CursorDefine(cursor));
    }

    fn disconnected(&mut self) {
        self.send(ConsoleEvent::Disconnected);
    }

    fn interfaces(&self) -> Vec<String> {
        // todo: find a clever way to query the object server for this?
        if cfg!(windows) {
            vec![
                "org.qemu.Display1.Listener.Win32.Map".to_string(),
                "org.qemu.Display1.Listener.Win32.D3d11".to_string(),
            ]
        } else if cfg!(unix) {
            vec![
                "org.qemu.Display1.Listener.Unix.Map".to_string(),
                "org.qemu.Display1.Listener.Unix.ScanoutDMABUF2".to_string(),
            ]
        } else {
            vec![]
        }
    }
}

fn from_gdk_button(button: u32) -> qemu_display::MouseButton {
    use qemu_display::MouseButton::*;

    match button {
        1 => Left,
        2 => Middle,
        3 => Right,
        _ => Extra,
    }
}

#[cfg(unix)]
#[async_trait::async_trait]
impl ConsoleListenerScanoutDmabuf2Handler for ConsoleHandler {
    async fn scanout_dmabuf(&mut self, scanout: qemu_display::ScanoutDMABUF) {
        self.send(ConsoleEvent::ScanoutDMABUF(scanout));
    }
}
