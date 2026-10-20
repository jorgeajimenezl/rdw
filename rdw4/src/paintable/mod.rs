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

    pub fn set_pixel_format(&self, format: PixelFormat) -> Result<(), glib::error::Error> {
        self.imp().set_pixel_format(format)
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.imp().pixel_format()
    }

    pub fn set_size(&self, w: usize, h: usize) -> Result<(), glib::error::Error> {
        self.imp().set_size(w, h)
    }

    pub fn size(&self) -> (i32, i32) {
        self.imp().size()
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
