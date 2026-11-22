use crate::PixelFormat;
use gtk::{cairo, gdk, glib, prelude::*, subclass::prelude::*};

mod imp;

glib::wrapper! {
    pub struct Paintable(ObjectSubclass<imp::Paintable>)
        @implements gdk::Paintable;
}

impl Default for Paintable {
    fn default() -> Self {
        Self::new()
    }
}

impl Paintable {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// The format of the frames' pixels. A DMABUF scanout's storage follows it too, so an FP16
    /// buffer needs `Rgba16f`. After a change of bytes per pixel, send a whole frame.
    pub fn set_pixel_format(&self, format: PixelFormat) -> Result<(), glib::error::Error> {
        self.imp().set_pixel_format(format)
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.imp().pixel_format()
    }

    /// The colour state of the frames, sRGB by default. A change makes the next frame whole.
    pub fn set_color_state(&self, state: &gdk::ColorState) {
        self.imp().set_color_state(state)
    }

    pub fn color_state(&self) -> gdk::ColorState {
        self.imp().color_state()
    }

    /// A factor on the frames' linear light, 1 by default: for an HDR source whose SDR white
    /// isn't GTK's (scRGB puts Windows' at its nits / 80), so that it matches the host's. Not
    /// free: GTK copies each new frame to apply it, which costs Vulkan a fresh image per frame.
    pub fn set_white_scale(&self, scale: f32) {
        self.imp().set_white_scale(scale)
    }

    pub fn white_scale(&self) -> f32 {
        self.imp().white_scale()
    }

    pub fn set_size(&self, w: usize, h: usize) -> Result<(), glib::error::Error> {
        self.imp().set_size(w, h)
    }

    pub fn size(&self) -> (i32, i32) {
        self.imp().size()
    }

    /// The size the picture is drawn at, in device pixels: damage grows on a scaled picture.
    pub(crate) fn set_shown_size(&self, w: i32, h: i32) {
        self.imp().set_shown_size(w, h)
    }

    pub fn update_area(
        &self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        stride: i32,
        data: Option<&[u8]>,
    ) -> Result<(), glib::error::Error> {
        self.imp().update_area(x, y, w, h, stride, data)
    }

    #[cfg(unix)]
    pub fn import_dmabuf(&self, s: &crate::RdwDmabufScanout) -> Result<(), glib::error::Error> {
        unsafe { self.imp().import_dmabuf(s) }
    }

    #[cfg(unix)]
    pub fn dmabuf_import_failed(&self) -> bool {
        self.imp().import_failed()
    }

    #[cfg(windows)]
    pub(crate) fn import_d3d11_texture2d_scanout(
        &self,
        s: Option<crate::RdwD3d11Texture2dScanout>,
    ) -> Result<(), glib::error::Error> {
        unsafe {
            self.imp()
                .win32
                .import_d3d11_texture2d_scanout(self.imp(), s)
        }
    }

    #[cfg(windows)]
    pub(crate) fn set_d3d11_texture2d_can_acquire(&self, can_acquire: bool) {
        self.imp()
            .win32
            .set_d3d11_texture2d_can_acquire(can_acquire)
    }
}
