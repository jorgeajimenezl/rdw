use glib::{signal::SignalHandlerId, subclass::prelude::*, translate::*};
use gtk::{gdk, glib, prelude::*, subclass::prelude::WidgetImpl};

#[cfg(windows)]
use crate::RdwD3d11Texture2dScanout;
#[cfg(unix)]
use crate::RdwDmabufScanout;
use crate::{Grab, KeyEvent, PixelFormat, Scroll};

#[repr(C)]
pub struct RdwDisplayClass {
    pub parent_class: gtk::ffi::GtkWidgetClass,
}

#[repr(C)]
pub struct RdwDisplay {
    parent: gtk::ffi::GtkWidget,
}

impl std::fmt::Debug for RdwDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("RdwDisplay")
            .field("parent", &self.parent)
            .finish()
    }
}

#[cfg(not(feature = "bindings"))]
pub mod imp;

impl Display {
    pub fn make_cursor(
        data: &[u8],
        width: i32,
        height: i32,
        hot_x: i32,
        hot_y: i32,
        scale: i32,
    ) -> gdk::Cursor {
        assert!(scale > 0);
        let pb = gdk::gdk_pixbuf::Pixbuf::from_mut_slice(
            data.to_vec(),
            gdk::gdk_pixbuf::Colorspace::Rgb,
            true,
            8,
            width,
            height,
            width * 4,
        );
        let pb = pb
            .scale_simple(
                width * scale,
                height * scale,
                gdk::gdk_pixbuf::InterpType::Bilinear,
            )
            .unwrap();
        let tex = gdk::Texture::for_pixbuf(&pb);
        gdk::Cursor::from_texture(&tex, hot_x * scale, hot_y * scale, None)
    }

    pub fn shot_widget(widget: &gtk::Widget) -> Option<gdk::gdk_pixbuf::Pixbuf> {
        use gdk::gdk_pixbuf::*;

        let snap = gtk::Snapshot::new();
        let parent = widget.parent()?;
        let native = widget.native()?;
        parent.snapshot_child(widget, &snap);
        let node = snap.to_node()?;
        let renderer = native.renderer()?;
        let texture = renderer.render_texture(node, None);
        let mut down = gdk::TextureDownloader::new(&texture);
        down.set_format(gdk::MemoryFormat::R8g8b8a8);
        let (bytes, stride) = down.download_bytes();

        Some(Pixbuf::from_bytes(
            &bytes,
            Colorspace::Rgb,
            true,
            8,
            texture.width(),
            texture.height(),
            stride as _,
        ))
    }
}

/// cbindgen:ignore
pub const NONE_DISPLAY: Option<&Display> = None;

pub trait DisplayExt: 'static {
    fn read_only(&self) -> bool;

    fn send_keys(&self, keyvals: &[gdk::Key]);

    fn display_size(&self) -> Option<(usize, usize)>;

    fn set_display_size(&self, size: Option<(usize, usize)>);
    fn display_pixel_format(&self) -> PixelFormat;

    fn set_display_pixel_format(&self, format: PixelFormat);

    fn define_cursor(&self, cursor: Option<gdk::Cursor>);

    fn mouse_absolute(&self) -> bool;

    fn set_mouse_absolute(&self, absolute: bool);

    fn set_cursor_position(&self, pos: Option<(i32, i32)>);

    /// Whether the remote shows its cursor. In absolute mode a hidden cursor shows no pointer
    /// over the display, instead of the last defined shape.
    fn set_cursor_visible(&self, visible: bool);

    fn grab_shortcut(&self) -> gtk::ShortcutTrigger;

    fn grabbed(&self) -> Grab;

    fn update_area(&self, x: i32, y: i32, w: i32, h: i32, stride: i32, data: Option<&[u8]>);

    #[cfg(windows)]
    fn set_d3d11_texture2d_scanout(&self, s: Option<RdwD3d11Texture2dScanout>);

    #[cfg(windows)]
    fn set_d3d11_texture2d_can_acquire(&self, can_acquire: bool);

    #[cfg(unix)]
    fn set_dmabuf_scanout(&self, s: RdwDmabufScanout);

    fn set_alternative_text(&self, alt_text: &str);

    fn remote_resize(&self) -> bool;

    fn set_remote_resize(&self, remote_resize: bool);

    fn try_grab(&self) -> Grab;

    fn ungrab(&self);

    fn connect_key_event<F: Fn(&Self, u32, u32, KeyEvent) + 'static>(
        &self,
        f: F,
    ) -> SignalHandlerId;

    fn connect_motion<F: Fn(&Self, f64, f64) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_motion_relative<F: Fn(&Self, f64, f64) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_mouse_press<F: Fn(&Self, u32) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_mouse_release<F: Fn(&Self, u32) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_scroll_discrete<F: Fn(&Self, Scroll) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_property_read_only_notify<F: Fn(&Self) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_property_grabbed_notify<F: Fn(&Self) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_remote_resize_notify<F: Fn(&Self) + 'static>(&self, f: F) -> SignalHandlerId;

    fn connect_resize_request<F: Fn(&Self, u32, u32, u32, u32) + 'static>(
        &self,
        f: F,
    ) -> SignalHandlerId;
}

impl<O: IsA<Display> + IsA<gtk::Widget> + IsA<gtk::Accessible>> DisplayExt for O {
    fn read_only(&self) -> bool {
        self.property("read-only")
    }

    fn send_keys(&self, keyvals: &[gdk::Key]) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            let mut res = Vec::with_capacity(keyvals.len());
            for k in keyvals {
                res.push(k.into_glib());
            }
            ffi::rdw_display_send_keys(self_.to_glib_none().0, res.as_mut_ptr(), keyvals.len());
        }
        #[cfg(not(feature = "bindings"))]
        {
            macro_rules! process_keys {
                ($self:ident, $keyvals:expr, $operation:ident) => {
                    let imp = self_.imp();
                    for &kv in $keyvals {
                        // todo: group & level might be necessary, some day
                        if let Some(kks) = $self.display().map_keyval(kv) {
                            for kk in kks {
                                imp.$operation(kv, kk.keycode());
                            }
                        } else {
                            log::warn!("Failed to send key {:?}", kv);
                        }
                    }
                };
            }

            process_keys!(self, keyvals.iter(), do_key_press);
            process_keys!(self, keyvals.iter().rev(), do_key_release);
        }
    }

    fn display_size(&self) -> Option<(usize, usize)> {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            let (mut w, mut h) = (
                std::mem::MaybeUninit::uninit(),
                std::mem::MaybeUninit::uninit(),
            );
            ffi::rdw_display_get_display_size(
                self_.to_glib_none().0,
                w.as_mut_ptr(),
                h.as_mut_ptr(),
            )
            .then(|| (w.assume_init(), h.assume_init()))
        }
        #[cfg(not(feature = "bindings"))]
        {
            let (w, h) = self_.imp().picture.paintable().size();
            Some((w as _, h as _))
        }
    }

    fn set_display_size(&self, size: Option<(usize, usize)>) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        log::trace!("set_display_size: {size:?}");
        #[cfg(feature = "bindings")]
        unsafe {
            let (w, h) = size.unwrap_or_default();
            ffi::rdw_display_set_display_size(self_.to_glib_none().0, w, h);
        }
        #[cfg(not(feature = "bindings"))]
        {
            let imp = self_.imp();

            if self.display_size() == size {
                return;
            }

            let (width, height) = size.unwrap_or((0, 0));
            if let Err(e) = imp.picture.paintable().set_size(width, height) {
                log::warn!("Failed to set size: {e}");
            }

            self.queue_resize();
        }
    }

    fn display_pixel_format(&self) -> PixelFormat {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            PixelFormat::from_glib(ffi::rdw_display_get_display_pixel_format(
                self_.to_glib_none().0,
            ))
        }
        #[cfg(not(feature = "bindings"))]
        {
            self_.imp().picture.paintable().pixel_format()
        }
    }

    fn set_display_pixel_format(&self, format: PixelFormat) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        log::trace!("set_pixel_format: {format:?}");
        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_set_display_pixel_format(self_.to_glib_none().0, format.into_glib());
        }
        #[cfg(not(feature = "bindings"))]
        {
            let imp = self_.imp();

            if let Err(e) = imp.picture.paintable().set_pixel_format(format) {
                log::warn!("Failed to set pixel format: {e}");
            }

            self.queue_resize();
        }
    }

    fn define_cursor(&self, cursor: Option<gdk::Cursor>) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_define_cursor(self_.to_glib_none().0, cursor.to_glib_none().0);
        }
        #[cfg(not(feature = "bindings"))]
        {
            let imp = self_.imp();
            imp.cursor.replace(cursor);
            imp.update_cursor();
        }
    }

    fn mouse_absolute(&self) -> bool {
        self.property("mouse-absolute")
    }

    fn set_mouse_absolute(&self, absolute: bool) {
        self.set_property("mouse-absolute", absolute)
    }

    fn set_cursor_position(&self, pos: Option<(i32, i32)>) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            let (x, y, enabled) = match pos {
                Some((x, y)) => (x, y, true),
                None => (0, 0, false),
            };
            ffi::rdw_display_set_cursor_position(self_.to_glib_none().0, enabled, x, y);
        }
        #[cfg(not(feature = "bindings"))]
        {
            self_.imp().cursor_position.set(pos);
            self.queue_draw();
        }
    }

    fn set_cursor_visible(&self, visible: bool) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_set_cursor_visible(self_.to_glib_none().0, visible);
        }
        #[cfg(not(feature = "bindings"))]
        {
            let imp = self_.imp();
            if imp.cursor_hidden.replace(!visible) != !visible {
                imp.update_cursor();
            }
        }
    }

    fn grab_shortcut(&self) -> gtk::ShortcutTrigger {
        self.property("grab-shortcut")
    }

    fn grabbed(&self) -> Grab {
        self.property("grabbed")
    }

    fn update_area(&self, x: i32, y: i32, w: i32, h: i32, stride: i32, data: Option<&[u8]>) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_update_area(
                self_.to_glib_none().0,
                x,
                y,
                w,
                h,
                stride,
                data.map_or(std::ptr::null(), |d| d.as_ptr()),
            );
        }
        #[cfg(not(feature = "bindings"))]
        {
            let imp = self_.imp();

            if let Err(e) = imp
                .picture
                .paintable()
                .update_area(x, y, w, h, stride, data)
            {
                log::warn!("Failed to update area: {e}");
            }
        }
    }

    #[cfg(windows)]
    fn set_d3d11_texture2d_scanout(&self, s: Option<RdwD3d11Texture2dScanout>) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            let s = s.as_ref().map_or(std::ptr::null(), |s| s as *const _);
            ffi::rdw_display_set_d3d11_texture2d_scanout(self_.to_glib_none().0, s);
        }
        #[cfg(not(feature = "bindings"))]
        {
            let imp = self_.imp();

            if let Err(e) = imp.picture.paintable().import_d3d11_texture2d_scanout(s) {
                log::warn!("Failed to import D3D scanout: {}", e);
            }
        }
    }

    #[cfg(windows)]
    fn set_d3d11_texture2d_can_acquire(&self, can_acquire: bool) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_set_d3d11_texture2d_can_acquire(self_.to_glib_none().0, can_acquire);
        }
        #[cfg(not(feature = "bindings"))]
        {
            let imp = imp::Display::from_obj(self_);

            imp.picture
                .paintable()
                .set_d3d11_texture2d_can_acquire(can_acquire);
        }
    }

    #[cfg(unix)]
    fn set_dmabuf_scanout(&self, s: RdwDmabufScanout) {
        // Safety: safe because IsA<Display>
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_set_dmabuf_scanout(self_.to_glib_none().0, &s);
        }
        #[cfg(all(unix, not(feature = "bindings")))]
        {
            let imp = self_.imp();

            if let Err(e) = imp.picture.paintable().import_dmabuf(&s) {
                log::warn!("Failed to import DMABUF: {e}");
            }
        }
    }

    // fn render(&self) {
    //                 #[cfg(windows)]
    //                 let guard = imp.d3d11_texture2d_acquire0().unwrap();
    //                 // imp.texture_blit(flip);
    //                 #[cfg(windows)]
    //                 drop(guard)

    fn set_alternative_text(&self, alt_text: &str) {
        self.update_property(&[gtk::accessible::Property::Description(alt_text)]);
    }

    fn remote_resize(&self) -> bool {
        self.property("remote-resize")
    }

    fn set_remote_resize(&self, remote_resize: bool) {
        self.set_property("remote-resize", remote_resize);
    }

    fn try_grab(&self) -> Grab {
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_try_grab(self_.to_glib_none().0)
        }

        #[cfg(not(feature = "bindings"))]
        {
            self_.imp().try_grab()
        }
    }

    fn ungrab(&self) {
        let self_: &Display = unsafe { self.unsafe_cast_ref::<Display>() };

        #[cfg(feature = "bindings")]
        unsafe {
            ffi::rdw_display_ungrab(self_.to_glib_none().0);
        }

        #[cfg(not(feature = "bindings"))]
        {
            self_.imp().ungrab()
        }
    }

    fn connect_key_event<F: Fn(&Self, u32, u32, KeyEvent) + 'static>(
        &self,
        f: F,
    ) -> SignalHandlerId {
        unsafe extern "C" fn connect_trampoline<P, F: Fn(&P, u32, u32, KeyEvent) + 'static>(
            this: *mut RdwDisplay,
            keyval: u32,
            keycode: u32,
            event: KeyEvent,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f = &*(f as *const F);
            f(
                Display::from_glib_borrow(this).unsafe_cast_ref::<P>(),
                keyval,
                keycode,
                event,
            )
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"key-event".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    connect_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_motion<F: Fn(&Self, f64, f64) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn connect_trampoline<P, F: Fn(&P, f64, f64) + 'static>(
            this: *mut RdwDisplay,
            x: f64,
            y: f64,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f = &*(f as *const F);
            f(Display::from_glib_borrow(this).unsafe_cast_ref::<P>(), x, y)
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"motion".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    connect_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_motion_relative<F: Fn(&Self, f64, f64) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn connect_trampoline<P, F: Fn(&P, f64, f64) + 'static>(
            this: *mut RdwDisplay,
            dx: f64,
            dy: f64,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f = &*(f as *const F);
            f(
                Display::from_glib_borrow(this).unsafe_cast_ref::<P>(),
                dx,
                dy,
            )
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"motion-relative".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    connect_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_mouse_press<F: Fn(&Self, u32) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn connect_trampoline<P, F: Fn(&P, u32) + 'static>(
            this: *mut RdwDisplay,
            button: u32,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f = &*(f as *const F);
            f(
                Display::from_glib_borrow(this).unsafe_cast_ref::<P>(),
                button,
            )
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"mouse-press".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    connect_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_mouse_release<F: Fn(&Self, u32) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn connect_trampoline<P, F: Fn(&P, u32) + 'static>(
            this: *mut RdwDisplay,
            button: u32,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f = &*(f as *const F);
            f(
                Display::from_glib_borrow(this).unsafe_cast_ref::<P>(),
                button,
            )
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"mouse-release".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    connect_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_scroll_discrete<F: Fn(&Self, Scroll) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn connect_trampoline<P, F: Fn(&P, Scroll) + 'static>(
            this: *mut RdwDisplay,
            scroll: Scroll,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f = &*(f as *const F);
            f(
                Display::from_glib_borrow(this).unsafe_cast_ref::<P>(),
                scroll,
            )
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"scroll-discrete".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    connect_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_property_read_only_notify<F: Fn(&Self) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn notify_trampoline<P, F: Fn(&P) + 'static>(
            this: *mut RdwDisplay,
            _param_spec: glib::ffi::gpointer,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f: &F = &*(f as *const F);
            f(Display::from_glib_borrow(this).unsafe_cast_ref())
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut _,
                c"notify::read-only".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    notify_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_property_grabbed_notify<F: Fn(&Self) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn notify_trampoline<P, F: Fn(&P) + 'static>(
            this: *mut RdwDisplay,
            _param_spec: glib::ffi::gpointer,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f: &F = &*(f as *const F);
            f(Display::from_glib_borrow(this).unsafe_cast_ref())
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut _,
                c"notify::grabbed".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    notify_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_remote_resize_notify<F: Fn(&Self) + 'static>(&self, f: F) -> SignalHandlerId {
        unsafe extern "C" fn notify_trampoline<P, F: Fn(&P) + 'static>(
            this: *mut RdwDisplay,
            _param_spec: glib::ffi::gpointer,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f: &F = &*(f as *const F);
            f(Display::from_glib_borrow(this).unsafe_cast_ref())
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut _,
                c"notify::remote-resize".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    notify_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }

    fn connect_resize_request<F: Fn(&Self, u32, u32, u32, u32) + 'static>(
        &self,
        f: F,
    ) -> SignalHandlerId {
        unsafe extern "C" fn connect_trampoline<P, F: Fn(&P, u32, u32, u32, u32) + 'static>(
            this: *mut RdwDisplay,
            width: u32,
            height: u32,
            width_mm: u32,
            height_mm: u32,
            f: glib::ffi::gpointer,
        ) where
            P: IsA<Display>,
        {
            let f = &*(f as *const F);
            f(
                Display::from_glib_borrow(this).unsafe_cast_ref::<P>(),
                width,
                height,
                width_mm,
                height_mm,
            )
        }
        unsafe {
            let f: Box<F> = Box::new(f);
            glib::signal::connect_raw(
                self.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"resize-request".as_ptr() as *const _,
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                    connect_trampoline::<Self, F> as *const (),
                )),
                Box::into_raw(f),
            )
        }
    }
}

pub trait DisplayImpl: DisplayImplExt + WidgetImpl {}

pub trait DisplayImplExt: ObjectSubclass {}

impl<T: DisplayImpl> DisplayImplExt for T {}

unsafe impl<T: DisplayImpl> IsSubclassable<T> for Display {}

#[cfg(not(feature = "bindings"))]
glib::wrapper! {
    pub struct Display(ObjectSubclass<imp::Display>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// cbindgen:ignore
#[cfg(feature = "bindings")]
mod ffi;

#[cfg(feature = "bindings")]
glib::wrapper! {
    pub struct Display(Object<RdwDisplay, RdwDisplayClass>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;

    match fn {
        type_ => || ffi::rdw_display_get_type(),
    }
}
