use gtk::{
    gdk,
    glib::{self, translate::*},
};
use std::ptr;

use crate::display::*;

#[cfg(windows)]
use crate::egl::RdwD3d11Texture2dScanout;
#[cfg(unix)]
use crate::egl::RdwDmabufScanout;
use crate::ffi::RdwPixelFormat;

#[no_mangle]
pub extern "C" fn rdw_error_quark() -> glib::ffi::GQuark {
    <crate::Error as glib::error::ErrorDomain>::domain().into_glib()
}

#[no_mangle]
pub extern "C" fn rdw_display_get_type() -> glib::ffi::GType {
    <crate::Display as glib::types::StaticType>::static_type().into_glib()
}

/// rdw_init:
/// Initializes the library (for debugging)
#[no_mangle]
pub extern "C" fn rdw_init(_error: *mut *mut glib::ffi::GError) -> bool {
    env_logger::init();
    log::trace!("rdw_init");
    true
}

/// rdw_shot_widget:
/// @widget: A #GtkWidget
///
/// Returns: (nullable) (transfer full): a #GdkPixbuf
#[no_mangle]
pub extern "C" fn rdw_shot_widget(
    widget: *mut gtk::ffi::GtkWidget,
) -> *mut gtk::gdk_pixbuf::ffi::GdkPixbuf {
    let w: &gtk::Widget = unsafe { &from_glib_borrow(widget) };
    Display::shot_widget(w).to_glib_full()
}

/// rdw_display_try_grab:
/// @dpy: A #RdwDisplay
#[no_mangle]
pub extern "C" fn rdw_display_try_grab(dpy: *mut RdwDisplay) -> crate::ffi::RdwGrab {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    this.try_grab().into_glib()
}

/// rdw_display_ungrab:
/// @dpy: A #RdwDisplay
#[no_mangle]
pub extern "C" fn rdw_display_ungrab(dpy: *mut RdwDisplay) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    this.ungrab()
}

/// rdw_display_send_keys:
/// @dpy: A #RdwDisplay
/// @keys: (array length=nkeys): an array of GDK keyvals
/// @nkeys: the number of elements in @keys
#[no_mangle]
pub extern "C" fn rdw_display_send_keys(dpy: *mut RdwDisplay, keys: *mut u32, nkeys: usize) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    let mut res = Vec::with_capacity(nkeys);
    for i in 0..nkeys {
        res.push(unsafe { from_glib(ptr::read(keys.add(i))) });
    }
    this.send_keys(&res);
}

/// rdw_display_get_display_size:
/// @dpy: A #RdwDisplay
/// @width: (out): display width
/// @height: (out): display height
#[no_mangle]
pub extern "C" fn rdw_display_get_display_size(
    dpy: *mut RdwDisplay,
    width: *mut usize,
    height: *mut usize,
) -> bool {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    match this.display_size() {
        Some(res) => unsafe {
            *width = res.0;
            *height = res.1;
            true
        },
        _ => false,
    }
}

/// rdw_display_set_display_size:
/// @dpy: A #RdwDisplay
/// @width: display width
/// @height: display height
///
/// Set the display size. If width & height are 0, the display size is set to
/// unknown.
#[no_mangle]
pub extern "C" fn rdw_display_set_display_size(dpy: *mut RdwDisplay, width: usize, height: usize) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    let size = if width != 0 && height != 0 {
        Some((width, height))
    } else {
        None
    };
    this.set_display_size(size);
}

/// rdw_display_get_display_pixel_format:
/// @dpy: A #RdwDisplay
/// @format: (out): current display pixel format
#[no_mangle]
pub extern "C" fn rdw_display_get_display_pixel_format(dpy: *mut RdwDisplay) -> RdwPixelFormat {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    this.display_pixel_format().into_glib()
}

/// rdw_display_set_display_pixel_format:
/// @dpy: A #RdwDisplay
/// @format: pixel format
///
/// Set the display pixel format.
#[no_mangle]
pub extern "C" fn rdw_display_set_display_pixel_format(
    dpy: *mut RdwDisplay,
    format: RdwPixelFormat,
) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    this.set_display_pixel_format(unsafe { crate::enums::PixelFormat::from_glib(format) });
}

/// rdw_display_define_cursor:
/// @dpy: A #RdwDisplay
/// @cursor: (nullable): a #GdkCursor
///
/// Set the cursor shape.
#[no_mangle]
pub extern "C" fn rdw_display_define_cursor(
    dpy: *mut RdwDisplay,
    cursor: *const gdk::ffi::GdkCursor,
) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    let cursor = unsafe { from_glib_none(cursor) };
    this.define_cursor(cursor);
}

/// rdw_display_set_cursor_position:
/// @dpy: A #RdwDisplay
/// @enabled: whether the cursor is shown
///
/// Set the cursor position (in mouse relative mode).
#[no_mangle]
pub extern "C" fn rdw_display_set_cursor_position(
    dpy: *mut RdwDisplay,
    enabled: bool,
    x: i32,
    y: i32,
) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    let pos = enabled.then(|| (x, y));
    this.set_cursor_position(pos);
}

/// rdw_display_set_cursor_visible:
/// @dpy: A #RdwDisplay
/// @visible: whether the remote shows its cursor
///
/// Hide or show the remote cursor (in mouse absolute mode, a hidden cursor shows no pointer).
#[no_mangle]
pub extern "C" fn rdw_display_set_cursor_visible(dpy: *mut RdwDisplay, visible: bool) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    this.set_cursor_visible(visible);
}

/// rdw_display_update_area:
/// @dpy: A #RdwDisplay
/// @data: (array) (element-type guint8): data
#[no_mangle]
pub extern "C" fn rdw_display_update_area(
    dpy: *mut RdwDisplay,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    stride: i32,
    data: *const u8,
) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    let data = if data.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(data, (h * stride) as _) })
    };
    this.update_area(x, y, w, h, stride, data);
}

/// rdw_display_set_dmabuf_scanout:
/// @dpy: A #RdwDisplay
#[cfg(unix)]
#[no_mangle]
pub extern "C" fn rdw_display_set_dmabuf_scanout(
    dpy: *mut RdwDisplay,
    dmabuf: *const RdwDmabufScanout,
) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    let dmabuf = unsafe { &*dmabuf };
    let dmabuf = RdwDmabufScanout {
        width: dmabuf.width,
        height: dmabuf.height,
        offset: dmabuf.offset,
        stride: dmabuf.stride,
        num_planes: dmabuf.num_planes,
        fourcc: dmabuf.fourcc,
        modifier: dmabuf.modifier,
        fd: dmabuf.fd,
        y0_top: dmabuf.y0_top,
    };
    this.set_dmabuf_scanout(dmabuf);
}

/// rdw_display_set_d3d11_texture2d_scanout:
/// @dpy: A #RdwDisplay
#[cfg(windows)]
#[no_mangle]
pub extern "C" fn rdw_display_set_d3d11_texture2d_scanout(
    dpy: *mut RdwDisplay,
    s: *const RdwD3d11Texture2dScanout,
) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    let s = if s.is_null() {
        None
    } else {
        let s = unsafe { &*s };
        Some(RdwD3d11Texture2dScanout {
            handle: s.handle,
            tex_width: s.tex_width,
            tex_height: s.tex_height,
            y0_top: s.y0_top,
            x: s.x,
            y: s.y,
            w: s.w,
            h: s.h,
        })
    };

    this.set_d3d11_texture2d_scanout(s);
}

/// rdw_display_set_d3d11_texture2d_can_acquire:
/// @dpy: A #RdwDisplay
#[cfg(windows)]
#[no_mangle]
pub extern "C" fn rdw_display_set_d3d11_texture2d_can_acquire(
    dpy: *mut RdwDisplay,
    can_acquire: bool,
) {
    let this: &Display = unsafe { &from_glib_borrow(dpy) };
    this.set_d3d11_texture2d_can_acquire(can_acquire);
}

#[no_mangle]
pub extern "C" fn rdw_content_provider_get_type() -> glib::ffi::GType {
    <crate::ContentProvider as glib::types::StaticType>::static_type().into_glib()
}

#[no_mangle]
pub extern "C" fn rdw_usb_redir_get_type() -> glib::ffi::GType {
    <crate::UsbRedir as glib::types::StaticType>::static_type().into_glib()
}

#[no_mangle]
pub extern "C" fn rdw_usb_device_get_type() -> glib::ffi::GType {
    <crate::UsbDevice as glib::types::StaticType>::static_type().into_glib()
}
