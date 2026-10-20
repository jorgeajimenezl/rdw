use super::*;
use gtk::graphene;
use std::cell::{Cell, RefCell};

use std::cell::OnceCell;

#[cfg(windows)]
mod win32;

/// The current DMABUF scanout. Each update is copied from its kept import, or, when that
/// fails, wraps the same buffer in a new texture.
#[cfg(unix)]
#[derive(Debug)]
struct Dmabuf {
    /// Which buffer it is, for its kept import.
    key: Option<BufferKey>,
    fds: Vec<std::os::fd::OwnedFd>,
    width: u32,
    height: u32,
    offset: [u32; 4],
    stride: [u32; 4],
    fourcc: u32,
    modifier: u64,
}

/// A DMABUF's identity: the same buffer behind any fd (its inode), with the same layout.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BufferKey {
    planes: [(u64, u64, u32, u32); 4],
    width: u32,
    height: u32,
    fourcc: u32,
    modifier: u64,
}

#[cfg(unix)]
impl BufferKey {
    fn of(s: &crate::RdwDmabufScanout) -> std::io::Result<Self> {
        let mut planes = [(0, 0, 0, 0); 4];
        for (plane, p) in planes.iter_mut().enumerate().take(s.num_planes as usize) {
            let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(s.fd[plane], st.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let st = unsafe { st.assume_init() };
            // dev_t and ino_t aren't u64 everywhere.
            #[allow(clippy::unnecessary_cast)]
            let id = (st.st_dev as u64, st.st_ino as u64);
            *p = (id.0, id.1, s.offset[plane], s.stride[plane]);
        }
        Ok(Self {
            planes,
            width: s.width,
            height: s.height,
            fourcc: s.fourcc,
            modifier: s.modifier,
        })
    }
}

/// A DMABUF imported into a GL texture, which its updates are copied from.
#[cfg(unix)]
#[derive(Debug)]
struct Import {
    key: BufferKey,
    display: crate::egl::Display,
    image: crate::egl::Image,
    texture: gl::types::GLuint,
}

/// How many DMABUF imports are kept: a source may switch between a few buffers every frame.
#[cfg(unix)]
const KEPT_IMPORTS: usize = 4;

/// GLES stores BGRA through EXT_texture_format_BGRA8888, in BGRA storage only.
#[cfg(unix)]
const BGRA_EXT: u32 = 0x80E1;

#[derive(Debug, Default)]
pub struct Paintable {
    buffer: RefCell<Vec<u8>>,
    width: Cell<i32>,
    height: Cell<i32>,
    pixel_format: Cell<PixelFormat>,
    texture: RefCell<Option<gdk::Texture>>,
    y0_top: Cell<Option<bool>>,
    /// The size it's drawn at, in device pixels, for the damage margin.
    shown: Cell<(i32, i32)>,
    #[cfg(unix)]
    dmabuf: RefCell<Option<Dmabuf>>,
    /// Size and format of the GL texture's storage, once memory frames are uploaded through GL.
    #[cfg(unix)]
    gl_storage: Cell<Option<(i32, i32, PixelFormat)>>,
    /// GL was unusable: memory frames go through `GdkMemoryTexture` instead.
    #[cfg(unix)]
    gl_failed: Cell<bool>,
    /// Fence syncs of textures GTK has released, deleted on the next upload. The builder leaves
    /// them to us, and the release function may run off the main thread.
    #[cfg(unix)]
    released_syncs: std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
    /// The kept DMABUF imports, most recently used first.
    #[cfg(unix)]
    imports: RefCell<Vec<Import>>,
    /// A DMABUF import or copy failed: each update is a new `GdkDmabufTexture` instead.
    #[cfg(unix)]
    import_failed: Cell<bool>,

    ctxt: OnceCell<gdk::GLContext>,
    texture_id: Cell<Option<gl::types::GLuint>>,
    #[cfg(windows)]
    pub(crate) win32: win32::Helper,
}

/// cbindgen:ignore
#[glib::object_subclass]
impl ObjectSubclass for Paintable {
    const NAME: &'static str = "RdwPaintable";
    type Type = super::Paintable;
    type ParentType = glib::Object;
    type Interfaces = (gdk::Paintable,);

    fn class_init(_klass: &mut Self::Class) {}
}

impl ObjectImpl for Paintable {
    fn constructed(&self) {}

    fn dispose(&self) {
        #[cfg(unix)]
        self.drop_imports();
        if let Some(tex_id) = self.texture_id.take() {
            #[cfg(unix)]
            if let Some(ctxt) = self.ctxt.get() {
                ctxt.make_current();
                self.delete_released_syncs();
            }
            unsafe {
                gl::DeleteTextures(1, &tex_id);
            }
        }
    }
}

impl PaintableImpl for Paintable {
    fn intrinsic_width(&self) -> i32 {
        self.width.get()
    }

    fn intrinsic_height(&self) -> i32 {
        self.height.get()
    }

    fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
        if let Some(texture) = self.texture.borrow().as_ref() {
            let flip = self.y0_top.get().unwrap_or_default();
            if flip {
                snapshot.save();
                snapshot.translate(&graphene::Point::new(0.0, height as _));
                snapshot.scale(1.0, -1.0);
                snapshot.append_texture(
                    texture,
                    &graphene::Rect::new(0.0, 0.0, width as _, height as _),
                );
                snapshot.restore();
            } else {
                snapshot.append_texture(
                    texture,
                    &graphene::Rect::new(0.0, 0.0, width as _, height as _),
                );
            }
        }
    }
}

impl Paintable {
    pub(crate) fn pixel_format(&self) -> PixelFormat {
        self.pixel_format.get()
    }

    pub(crate) fn size(&self) -> (i32, i32) {
        (self.width.get(), self.height.get())
    }

    /// The size the picture is drawn at, in device pixels (its widget sets it).
    pub(crate) fn set_shown_size(&self, w: i32, h: i32) {
        self.shown.set((w, h));
    }

    /// GTK's update region for a change of `r`: grown by [`damage_margin`] when the picture
    /// is scaled, inside the texture.
    fn damage_region(&self, r: cairo::RectangleInt) -> cairo::Region {
        let (w, h) = self.size();
        let m = damage_margin((w, h), self.shown.get());
        let grown = cairo::RectangleInt::new(
            r.x().saturating_sub(m),
            r.y().saturating_sub(m),
            r.width().saturating_add(2 * m),
            r.height().saturating_add(2 * m),
        );
        cairo::Region::create_rectangle(&clamp_rect(&grown, w, h).unwrap_or(r))
    }

    fn memory_format(format: PixelFormat) -> gdk::MemoryFormat {
        match format {
            PixelFormat::Bgra => gdk::MemoryFormat::B8g8r8a8Premultiplied,
            PixelFormat::Rgba => gdk::MemoryFormat::R8g8b8a8Premultiplied,
            PixelFormat::Bgrx => gdk::MemoryFormat::B8g8r8x8,
            other => {
                log::warn!("Unrecognized pixel format {other:?}, falling back to B8G8R8A8");
                gdk::MemoryFormat::B8g8r8a8Premultiplied
            }
        }
    }

    pub(crate) fn set_pixel_format(&self, format: PixelFormat) -> Result<(), glib::error::Error> {
        if self.pixel_format() == format {
            return Ok(());
        }
        self.pixel_format.set(format);

        #[cfg(windows)]
        {
            let (w, h) = self.size();
            if w > 0 && h > 0 {
                self.recreate_gl_texture((w, h), format)?;
            }
        }

        Ok(())
    }

    pub(crate) fn set_size(&self, w: usize, h: usize) -> Result<(), glib::error::Error> {
        if self.size() == (w as _, h as _) {
            return Ok(());
        }
        let (w, h) = (w as i32, h as i32);
        self.width.set(w);
        self.height.set(h);
        self.buffer
            .borrow_mut()
            .resize((w as usize) * (h as usize) * 4, 0);
        self.texture.replace(None);

        #[cfg(windows)]
        self.recreate_gl_texture((w, h), self.pixel_format.get())?;

        self.obj().invalidate_size();
        Ok(())
    }

    pub(crate) fn update_area(
        &self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        stride: i32,
        data: Option<&[u8]>,
    ) -> Result<(), glib::error::Error> {
        let (max_w, max_h) = self.size();
        let x = x.clamp(0, max_w);
        let y = y.clamp(0, max_h);
        let w = w.clamp(0, max_w - x);
        let h = h.clamp(0, max_h - y);

        if let Some(data) = data {
            #[cfg(windows)]
            unsafe {
                self.win32.import_d3d11_texture2d_scanout(self, None)?
            };
            // Back from a DMABUF scanout: the old texture is not this buffer's previous content.
            #[cfg(unix)]
            if self.dmabuf.take().is_some() {
                self.y0_top.set(None);
                self.texture.replace(None);
                self.gl_storage.set(None);
                self.drop_imports();
            }

            let buf_stride = max_w as usize * 4;
            // The GL texture is RGBA, and GTK's Vulkan renderer takes it through a dmabuf of a
            // format with alpha, ignoring the texture's "x": an XRGB source's X byte (0 from
            // QEMU's firmware screen) would make the picture transparent.
            let opaque = !self.pixel_format.get().has_alpha();
            let mut buffer = self.buffer.borrow_mut();
            for row in 0..h as usize {
                let src_start = row * stride as usize;
                let src_end = src_start + w as usize * 4;
                let dst_start = (y as usize + row) * buf_stride + x as usize * 4;
                let dst_end = dst_start + w as usize * 4;
                if src_end <= data.len() && dst_end <= buffer.len() {
                    let dst = &mut buffer[dst_start..dst_end];
                    dst.copy_from_slice(&data[src_start..src_end]);
                    if opaque {
                        for px in dst.chunks_exact_mut(4) {
                            px[3] = 0xff;
                        }
                    }
                }
            }
            drop(buffer);

            // GTK uploads a whole memory texture on every update; through GL only the damage goes.
            #[cfg(unix)]
            if !self.gl_failed.get() {
                match self.upload_gl_area(x, y, w, h) {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        log::warn!("GL upload unavailable, using memory textures: {e}");
                        self.gl_failed.set(true);
                        self.texture.replace(None);
                    }
                }
            }

            let buffer = self.buffer.borrow();
            let bytes = glib::Bytes::from(&*buffer);
            let region = self.damage_region(cairo::RectangleInt::new(x, y, w, h));
            let texture = gdk::MemoryTextureBuilder::new()
                .set_bytes(Some(&bytes))
                .set_format(Self::memory_format(self.pixel_format.get()))
                .set_width(max_w)
                .set_height(max_h)
                .set_stride(buf_stride)
                .set_update_texture(self.texture.borrow().as_ref())
                .set_update_region(Some(&region))
                .build();
            self.texture.replace(Some(texture));
            self.obj().invalidate_contents();
        } else {
            #[cfg(windows)]
            if self.win32.has_texture() {
                self.win32.update_texture(self, x, y, w, h)?;
                let region = self.damage_region(cairo::RectangleInt::new(x, y, w, h));
                self.update_gl_texture(None, Some(&region))?;
            }
            #[cfg(unix)]
            if self.dmabuf.borrow().is_some() {
                // The texture is drawn flipped when y0_top; damage is in texture rows.
                let y = if self.y0_top.get().unwrap_or_default() {
                    max_h - y - h
                } else {
                    y
                };
                let rect = cairo::RectangleInt::new(x, y, w, h);
                if !self.import_failed.get() {
                    match self.copy_from_import(Some(rect)) {
                        Ok(()) => return Ok(()),
                        Err(e) => self.fail_imports(e),
                    }
                }
                let region = self.damage_region(rect);
                unsafe { self.build_dmabuf_texture(&region)? };
            } else {
                log::warn!("update_area called with no data and no DMABUF scanout");
            }
        }

        Ok(())
    }

    #[cfg(unix)]
    pub(crate) unsafe fn import_dmabuf(
        &self,
        s: &crate::RdwDmabufScanout,
    ) -> Result<(), glib::error::Error> {
        use std::os::fd::BorrowedFd;

        // Keep our own fds: QEMU sends later updates of this buffer without them.
        let fds = s.fd[..s.num_planes as usize]
            .iter()
            .map(|&fd| BorrowedFd::borrow_raw(fd).try_clone_to_owned())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                glib::Error::new(crate::Error::GL, &format!("Failed to dup dmabuf fd: {e}"))
            })?;
        let key = BufferKey::of(s)
            .map_err(|e| log::warn!("Failed to identify the dmabuf: {e}"))
            .ok();
        self.dmabuf.replace(Some(Dmabuf {
            key,
            fds,
            width: s.width,
            height: s.height,
            offset: s.offset,
            stride: s.stride,
            fourcc: s.fourcc,
            modifier: s.modifier,
        }));

        // A source may hand the same buffers every frame: a relayout each time blanks frames.
        if self.size() != (s.width as i32, s.height as i32) {
            self.width.set(s.width as i32);
            self.height.set(s.height as i32);
            self.obj().invalidate_size();
        }
        self.y0_top.set(Some(s.y0_top));

        // Through the buffer's kept import, the texture keeps the previous frame, which the
        // source's next updates are against; a new import copies the whole buffer.
        if !self.import_failed.get() {
            match self.copy_from_import(None) {
                Ok(()) => return Ok(()),
                Err(e) => self.fail_imports(e),
            }
        }

        // A new buffer: nothing carries over from the previous texture.
        self.texture.replace(None);
        let region = cairo::Region::create_rectangle(&cairo::RectangleInt::new(
            0,
            0,
            s.width as _,
            s.height as _,
        ));
        self.build_dmabuf_texture(&region)?;
        Ok(())
    }

    /// Wraps the current DMABUF in a new texture that differs from the previous one in `region`.
    #[cfg(unix)]
    unsafe fn build_dmabuf_texture(
        &self,
        region: &cairo::Region,
    ) -> Result<(), glib::error::Error> {
        use std::os::fd::IntoRawFd;

        let display = gdk::Display::default()
            .ok_or(glib::Error::new(crate::Error::GL, "No default display"))?;
        let dmabuf = self.dmabuf.borrow();
        let Some(d) = dmabuf.as_ref() else {
            return Ok(());
        };

        // The texture owns its own copies, closed when GTK releases it.
        let dup_fds: Vec<_> = d
            .fds
            .iter()
            .map(|fd| fd.try_clone())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                glib::Error::new(crate::Error::GL, &format!("Failed to dup dmabuf fd: {e}"))
            })?
            .into_iter()
            .map(IntoRawFd::into_raw_fd)
            .collect();

        let mut builder = gdk::DmabufTextureBuilder::new()
            .set_display(&display)
            .set_fourcc(d.fourcc)
            .set_modifier(d.modifier)
            .set_width(d.width)
            .set_height(d.height)
            .set_n_planes(d.fds.len() as u32);

        for (plane, &fd) in dup_fds.iter().enumerate() {
            builder = builder.set_fd(plane as u32, fd);
            builder = builder.set_offset(plane as u32, d.offset[plane]);
            builder = builder.set_stride(plane as u32, d.stride[plane]);
        }

        let texture = builder
            .set_update_texture(self.texture.borrow().as_ref())
            .set_update_region(Some(region))
            .build_with_release_func(move || {
                for fd in dup_fds {
                    libc::close(fd);
                }
            })
            .map_err(|e| {
                glib::Error::new(
                    crate::Error::GL,
                    &format!("Failed to build dmabuf texture: {e}"),
                )
            })?;

        self.texture.replace(Some(texture));
        self.obj().invalidate_contents();
        Ok(())
    }

    pub(crate) fn gl_context(&self) -> Result<&gdk::GLContext, glib::error::Error> {
        let ctxt = if let Some(ctxt) = self.ctxt.get() {
            ctxt
        } else {
            let ctxt = gdk::Display::default().unwrap().create_gl_context()?;
            #[cfg(unix)]
            ctxt.realize()?;
            self.ctxt.set(ctxt).unwrap();
            self.ctxt.get().unwrap()
        };
        Ok(ctxt)
    }

    pub(crate) fn texture_id(&self) -> Result<gl::types::GLuint, glib::error::Error> {
        match self.texture_id.get() {
            None => {
                let mut tex_id = 0;
                let ctxt = self.gl_context()?;
                ctxt.make_current();
                unsafe {
                    gl::GenTextures(1, &mut tex_id);
                    gl::BindTexture(gl::TEXTURE_2D, tex_id);
                    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as _);
                    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as _);
                    assert_eq!(gl::GetError(), gl::NO_ERROR);
                    self.texture_id.set(Some(tex_id));
                }
                Ok(tex_id)
            }
            Some(id) => Ok(id),
        }
    }

    #[cfg(windows)]
    fn recreate_gl_texture(
        &self,
        size: (i32, i32),
        format: PixelFormat,
    ) -> Result<(), glib::error::Error> {
        let (w, h) = size;
        let ctxt = self.gl_context()?;
        ctxt.make_current();

        unsafe {
            assert_eq!(gl::GetError(), gl::NO_ERROR);
            gl::BindTexture(gl::TEXTURE_2D, self.texture_id()?);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA as _,
                w as _,
                h as _,
                0,
                format.as_opengl(),
                gl::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            if gl::GetError() != gl::NO_ERROR {
                gl::TexImage2D(
                    gl::TEXTURE_2D,
                    0,
                    gl::RGB as _,
                    w as _,
                    h as _,
                    0,
                    gl::RGB,
                    gl::UNSIGNED_BYTE,
                    std::ptr::null(),
                );
            }
            assert_eq!(gl::GetError(), gl::NO_ERROR);
        }

        self.update_gl_texture(Some((w, h)), None)?;
        Ok(())
    }

    /// Uploads the damaged rectangle of the frame buffer to the GL texture (the whole buffer when
    /// the storage changes), and wraps the texture for GTK.
    #[cfg(unix)]
    fn upload_gl_area(&self, x: i32, y: i32, w: i32, h: i32) -> Result<(), glib::error::Error> {
        let ctxt = self.gl_context()?;
        let gles = ctxt.api() == gdk::GLAPI::GLES;
        ctxt.make_current();

        let (max_w, max_h) = self.size();
        let format = self.pixel_format.get();
        let storage = (max_w, max_h, format);
        let full = self.gl_storage.get() != Some(storage);
        let buffer = self.buffer.borrow();
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, self.texture_id()?);
            if full {
                Self::alloc_storage(gles, storage, buffer.as_ptr() as _);
            } else {
                gl::PixelStorei(gl::UNPACK_ROW_LENGTH, max_w);
                gl::PixelStorei(gl::UNPACK_SKIP_PIXELS, x);
                gl::PixelStorei(gl::UNPACK_SKIP_ROWS, y);
                gl::TexSubImage2D(
                    gl::TEXTURE_2D,
                    0,
                    x,
                    y,
                    w,
                    h,
                    format.as_opengl(),
                    gl::UNSIGNED_BYTE,
                    buffer.as_ptr() as _,
                );
                gl::PixelStorei(gl::UNPACK_ROW_LENGTH, 0);
                gl::PixelStorei(gl::UNPACK_SKIP_PIXELS, 0);
                gl::PixelStorei(gl::UNPACK_SKIP_ROWS, 0);
            }
            let err = gl::GetError();
            if err != gl::NO_ERROR {
                self.gl_storage.set(None);
                return Err(glib::Error::new(
                    crate::Error::GL,
                    &format!("texture upload failed: GL error {err:#x}"),
                ));
            }
        }
        drop(buffer);
        self.gl_storage.set(Some(storage));

        let region = if full {
            cairo::RectangleInt::new(0, 0, max_w, max_h)
        } else {
            cairo::RectangleInt::new(x, y, w, h)
        };
        self.update_gl_texture(None, Some(&self.damage_region(region)))
    }

    /// Imports the current DMABUF into a GL texture of its own. The EGL image holds the buffer.
    #[cfg(unix)]
    fn import(&self, key: BufferKey, d: &Dmabuf) -> Result<Import, glib::error::Error> {
        use crate::egl;
        use std::os::fd::AsRawFd;

        let err = |msg: String| glib::Error::new(crate::Error::GL, &msg);
        let ctxt = self.gl_context()?;
        ctxt.make_current();
        let instance = egl::egl();
        let display = instance
            .get_current_display()
            .ok_or_else(|| err("no current EGL display".into()))?;
        let target_texture = egl::image_target_texture_2d_oes()
            .ok_or_else(|| err("no glEGLImageTargetTexture2DOES".into()))?;

        const PLANES: [[egl::Int; 5]; 4] = [
            [
                egl::DMA_BUF_PLANE0_FD_EXT,
                egl::DMA_BUF_PLANE0_OFFSET_EXT,
                egl::DMA_BUF_PLANE0_PITCH_EXT,
                egl::DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                egl::DMA_BUF_PLANE0_MODIFIER_HI_EXT,
            ],
            [
                egl::DMA_BUF_PLANE1_FD_EXT,
                egl::DMA_BUF_PLANE1_OFFSET_EXT,
                egl::DMA_BUF_PLANE1_PITCH_EXT,
                egl::DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                egl::DMA_BUF_PLANE1_MODIFIER_HI_EXT,
            ],
            [
                egl::DMA_BUF_PLANE2_FD_EXT,
                egl::DMA_BUF_PLANE2_OFFSET_EXT,
                egl::DMA_BUF_PLANE2_PITCH_EXT,
                egl::DMA_BUF_PLANE2_MODIFIER_LO_EXT,
                egl::DMA_BUF_PLANE2_MODIFIER_HI_EXT,
            ],
            [
                egl::DMA_BUF_PLANE3_FD_EXT,
                egl::DMA_BUF_PLANE3_OFFSET_EXT,
                egl::DMA_BUF_PLANE3_PITCH_EXT,
                egl::DMA_BUF_PLANE3_MODIFIER_LO_EXT,
                egl::DMA_BUF_PLANE3_MODIFIER_HI_EXT,
            ],
        ];
        const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
        let n_planes = d.fds.len();
        if n_planes == 0 || n_planes > 4 {
            return Err(err(format!("bad plane count {n_planes}")));
        }
        let mut attribs: Vec<egl::Attrib> = vec![
            egl::WIDTH as _,
            d.width as _,
            egl::HEIGHT as _,
            d.height as _,
            egl::LINUX_DRM_FOURCC_EXT as _,
            d.fourcc as _,
        ];
        for (plane, names) in PLANES.iter().enumerate().take(n_planes) {
            attribs.extend_from_slice(&[
                names[0] as _,
                d.fds[plane].as_raw_fd() as _,
                names[1] as _,
                d.offset[plane] as _,
                names[2] as _,
                d.stride[plane] as _,
            ]);
            if d.modifier != DRM_FORMAT_MOD_INVALID {
                attribs.extend_from_slice(&[
                    names[3] as _,
                    (d.modifier & 0xffff_ffff) as _,
                    names[4] as _,
                    (d.modifier >> 32) as _,
                ]);
            }
        }
        attribs.push(egl::NONE as _);

        let image = instance
            .create_image(
                display,
                egl::no_context(),
                egl::LINUX_DMA_BUF_EXT,
                egl::no_client_buffer(),
                &attribs,
            )
            .map_err(|e| err(format!("eglCreateImage failed: {e}")))?;

        let mut texture = 0;
        unsafe {
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(gl::TEXTURE_2D, texture);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as _);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as _);
            target_texture(gl::TEXTURE_2D, image.as_ptr());
        }
        let import = Import {
            key,
            display,
            image,
            texture,
        };
        let glerr = unsafe { gl::GetError() };
        if glerr != gl::NO_ERROR {
            self.delete_import(import);
            return Err(err(format!(
                "EGL image to texture failed: GL error {glerr:#x}"
            )));
        }
        Ok(import)
    }

    /// Copies `rect` (in texture rows) of the current DMABUF scanout from its kept import into
    /// the GL texture, importing the buffer first when it's new, and hands GTK the texture with
    /// that damage. A new import, or a change of the texture's size or format, copies the whole
    /// buffer; `None` copies nothing otherwise.
    #[cfg(unix)]
    fn copy_from_import(
        &self,
        rect: Option<cairo::RectangleInt>,
    ) -> Result<(), glib::error::Error> {
        let err = |msg: String| glib::Error::new(crate::Error::GL, &msg);
        let dmabuf = self.dmabuf.borrow();
        let d = dmabuf
            .as_ref()
            .ok_or_else(|| err("no DMABUF scanout".into()))?;
        let key = d.key.ok_or_else(|| err("unidentified DMABUF".into()))?;
        let ctxt = self.gl_context()?;
        let gles = ctxt.api() == gdk::GLAPI::GLES;
        ctxt.make_current();

        let mut imports = self.imports.borrow_mut();
        let new = match imports.iter().position(|i| i.key == key) {
            Some(at) => {
                imports[..=at].rotate_right(1);
                false
            }
            None => {
                imports.insert(0, self.import(key, d)?);
                let keep = KEPT_IMPORTS.min(imports.len());
                for old in imports.split_off(keep) {
                    self.delete_import(old);
                }
                true
            }
        };
        let source = imports[0].texture;
        drop(imports);

        let (max_w, max_h) = self.size();
        let format = self.pixel_format.get();
        let storage = (max_w, max_h, format);
        let full = new || self.gl_storage.get() != Some(storage);
        let rect = if full {
            cairo::RectangleInt::new(0, 0, max_w, max_h)
        } else {
            match rect.and_then(|r| clamp_rect(&r, max_w, max_h)) {
                Some(r) => r,
                None => return Ok(()),
            }
        };

        let texture_id = self.texture_id()?;
        unsafe {
            if full {
                gl::BindTexture(gl::TEXTURE_2D, texture_id);
                Self::alloc_storage(gles, storage, std::ptr::null());
            }
            let mut fbos = [0; 2];
            gl::GenFramebuffers(2, fbos.as_mut_ptr());
            gl::BindFramebuffer(gl::READ_FRAMEBUFFER, fbos[0]);
            gl::FramebufferTexture2D(
                gl::READ_FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                source,
                0,
            );
            gl::BindFramebuffer(gl::DRAW_FRAMEBUFFER, fbos[1]);
            gl::FramebufferTexture2D(
                gl::DRAW_FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                texture_id,
                0,
            );
            let read = gl::CheckFramebufferStatus(gl::READ_FRAMEBUFFER);
            let draw = gl::CheckFramebufferStatus(gl::DRAW_FRAMEBUFFER);
            let complete = read == gl::FRAMEBUFFER_COMPLETE && draw == gl::FRAMEBUFFER_COMPLETE;
            if complete {
                let (x0, y0) = (rect.x(), rect.y());
                let (x1, y1) = (x0 + rect.width(), y0 + rect.height());
                gl::BlitFramebuffer(
                    x0,
                    y0,
                    x1,
                    y1,
                    x0,
                    y0,
                    x1,
                    y1,
                    gl::COLOR_BUFFER_BIT,
                    gl::NEAREST,
                );
            }
            gl::BindFramebuffer(gl::READ_FRAMEBUFFER, 0);
            gl::BindFramebuffer(gl::DRAW_FRAMEBUFFER, 0);
            gl::DeleteFramebuffers(2, fbos.as_ptr());
            let glerr = gl::GetError();
            if !complete || glerr != gl::NO_ERROR {
                self.gl_storage.set(None);
                return Err(err(format!(
                    "DMABUF copy failed: framebuffers {read:#x}/{draw:#x}, GL error {glerr:#x}"
                )));
            }
        }
        drop(dmabuf);
        self.gl_storage.set(Some(storage));
        self.update_gl_texture(None, Some(&self.damage_region(rect)))
    }

    /// Gives up on kept imports for good: each DMABUF update becomes a new texture.
    #[cfg(unix)]
    fn fail_imports(&self, e: glib::Error) {
        log::warn!("DMABUF import unusable, wrapping each update instead: {e}");
        self.import_failed.set(true);
        self.drop_imports();
    }

    #[cfg(unix)]
    pub(crate) fn import_failed(&self) -> bool {
        self.import_failed.get()
    }

    #[cfg(unix)]
    fn drop_imports(&self) {
        for import in self.imports.take() {
            self.delete_import(import);
        }
    }

    #[cfg(unix)]
    fn delete_import(&self, import: Import) {
        if let Some(ctxt) = self.ctxt.get() {
            ctxt.make_current();
            unsafe { gl::DeleteTextures(1, &import.texture) };
        }
        if let Err(e) = crate::egl::egl().destroy_image(import.display, import.image) {
            log::warn!("eglDestroyImage failed: {e}");
        }
    }

    /// (Re)allocates the bound texture's storage, filled from `data` (or left undefined).
    #[cfg(unix)]
    unsafe fn alloc_storage(
        gles: bool,
        (w, h, format): (i32, i32, PixelFormat),
        data: *const std::ffi::c_void,
    ) {
        let internal = match (gles, format.as_opengl()) {
            (true, gl::BGRA) => BGRA_EXT,
            _ => gl::RGBA8,
        };
        gl::TexImage2D(
            gl::TEXTURE_2D,
            0,
            internal as _,
            w,
            h,
            0,
            format.as_opengl(),
            gl::UNSIGNED_BYTE,
            data,
        );
    }

    /// Deletes the syncs of released textures. Our GL context must be current.
    #[cfg(unix)]
    fn delete_released_syncs(&self) {
        for sync in self.released_syncs.lock().unwrap().drain(..) {
            unsafe { gl::DeleteSync(sync as gl::types::GLsync) };
        }
    }

    fn update_gl_texture(
        &self,
        size: Option<(i32, i32)>,
        region: Option<&cairo::Region>,
    ) -> Result<(), glib::error::Error> {
        let ctxt = self.gl_context()?;
        ctxt.make_current();
        #[cfg(unix)]
        self.delete_released_syncs();

        let (w, h) = size.unwrap_or_else(|| self.size());
        let texture = unsafe {
            let sync = gl::FenceSync(gl::SYNC_GPU_COMMANDS_COMPLETE, 0);
            let builder = gdk::GLTexture::builder()
                .set_context(Some(ctxt))
                .set_id(self.texture_id()?)
                .set_width(w)
                .set_height(h)
                .set_format(if self.pixel_format.get().has_alpha() {
                    gdk::MemoryFormat::R8g8b8a8
                } else {
                    gdk::MemoryFormat::R8g8b8x8
                })
                .set_update_region(region)
                .set_update_texture(self.texture.take().as_ref());
            let builder = if sync.is_null() {
                builder
            } else {
                builder.set_sync(Some(sync as _))
            };
            #[cfg(unix)]
            {
                let released = self.released_syncs.clone();
                let sync = sync as usize;
                builder.build_with_release_func(move || {
                    if sync != 0 {
                        released.lock().unwrap().push(sync);
                    }
                })
            }
            #[cfg(not(unix))]
            builder.build()
        };
        self.texture.replace(Some(texture));
        if region.is_some() {
            self.obj().invalidate_contents();
        }

        Ok(())
    }
}

/// How far damage grows on a `source`-sized texture drawn over `shown` device pixels: the
/// scaling filter reads past the changed texels, and GTK repaints only the update region, so
/// without a margin the old pixels around a change stay on screen. None at 1:1 (or before the
/// first allocation); else at least a pixel, and a texel per unit of downscale.
fn damage_margin(source: (i32, i32), shown: (i32, i32)) -> i32 {
    if source == shown || shown.0 <= 0 || shown.1 <= 0 || source.0 <= 0 || source.1 <= 0 {
        return 0;
    }
    let ratio = |s: i32, d: i32| (s as u32).div_ceil(d as u32) as i32;
    ratio(source.0, shown.0)
        .max(ratio(source.1, shown.1))
        .max(1)
}

/// `r` clipped to a `w`×`h` frame, or `None` when nothing is left.
fn clamp_rect(r: &cairo::RectangleInt, w: i32, h: i32) -> Option<cairo::RectangleInt> {
    let x0 = r.x().clamp(0, w);
    let y0 = r.y().clamp(0, h);
    let x1 = r.x().saturating_add(r.width()).clamp(0, w);
    let y1 = r.y().saturating_add(r.height()).clamp(0, h);
    (x1 > x0 && y1 > y0).then(|| cairo::RectangleInt::new(x0, y0, x1 - x0, y1 - y0))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> cairo::RectangleInt {
        cairo::RectangleInt::new(x, y, w, h)
    }

    #[test]
    fn clamp_rect_clips_to_the_frame() {
        assert_eq!(
            clamp_rect(&rect(10, 20, 30, 40), 100, 100),
            Some(rect(10, 20, 30, 40))
        );
        assert_eq!(
            clamp_rect(&rect(-5, -5, 10, 10), 100, 100),
            Some(rect(0, 0, 5, 5))
        );
        assert_eq!(
            clamp_rect(&rect(90, 95, 20, 20), 100, 100),
            Some(rect(90, 95, 10, 5))
        );
        assert_eq!(clamp_rect(&rect(100, 0, 10, 10), 100, 100), None);
        assert_eq!(clamp_rect(&rect(0, 0, 0, 10), 100, 100), None);
        assert_eq!(clamp_rect(&rect(0, 0, -10, 10), 100, 100), None);
        assert_eq!(clamp_rect(&rect(i32::MAX, 0, i32::MAX, 1), 100, 100), None);
    }

    #[test]
    fn damage_margins() {
        assert_eq!(damage_margin((1920, 1080), (1920, 1080)), 0);
        assert_eq!(damage_margin((3840, 2160), (1920, 1080)), 2, "2× down");
        assert_eq!(damage_margin((1920, 1080), (2880, 1620)), 1, "1.5× up");
        assert_eq!(
            damage_margin((1920, 1080), (1000, 1080)),
            2,
            "a texel per unit of downscale"
        );
        assert_eq!(damage_margin((1920, 1080), (0, 0)), 0, "not allocated yet");
        assert_eq!(damage_margin((0, 0), (800, 600)), 0, "no picture");
    }
}
