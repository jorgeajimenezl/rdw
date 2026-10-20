#![allow(dead_code)]

#[cfg(unix)]
use std::os::unix::prelude::RawFd;
#[cfg(windows)]
use std::os::windows::raw::HANDLE;

#[cfg(not(feature = "bindings"))]
pub use khronos_egl::*;

#[cfg(not(feature = "bindings"))]
mod imp {
    use super::*;
    use gdk::prelude::GLContextExt;
    use gtk::gdk;
    use std::{ffi::c_void, sync::OnceLock};

    type EglInstance = khronos_egl::DynamicInstance<khronos_egl::EGL1_5>;

    pub(crate) fn egl() -> &'static EglInstance {
        static INSTANCE: OnceLock<EglInstance> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            unsafe { khronos_egl::DynamicInstance::<khronos_egl::EGL1_5>::load_required() }
                .expect("failed to load EGL")
        })
    }

    pub(crate) const LINUX_DMA_BUF_EXT: Enum = 0x3270;
    pub(crate) const LINUX_DRM_FOURCC_EXT: Int = 0x3271;
    pub(crate) const DMA_BUF_PLANE0_FD_EXT: Int = 0x3272;
    pub(crate) const DMA_BUF_PLANE0_OFFSET_EXT: Int = 0x3273;
    pub(crate) const DMA_BUF_PLANE0_PITCH_EXT: Int = 0x3274;
    pub(crate) const DMA_BUF_PLANE0_MODIFIER_LO_EXT: Int = 0x3443;
    pub(crate) const DMA_BUF_PLANE0_MODIFIER_HI_EXT: Int = 0x3444;
    pub(crate) const DMA_BUF_PLANE1_FD_EXT: Int = 0x3275;
    pub(crate) const DMA_BUF_PLANE1_OFFSET_EXT: Int = 0x3276;
    pub(crate) const DMA_BUF_PLANE1_PITCH_EXT: Int = 0x3277;
    pub(crate) const DMA_BUF_PLANE1_MODIFIER_LO_EXT: Int = 0x3445;
    pub(crate) const DMA_BUF_PLANE1_MODIFIER_HI_EXT: Int = 0x3446;
    pub(crate) const DMA_BUF_PLANE2_FD_EXT: Int = 0x3278;
    pub(crate) const DMA_BUF_PLANE2_OFFSET_EXT: Int = 0x3279;
    pub(crate) const DMA_BUF_PLANE2_PITCH_EXT: Int = 0x327A;
    pub(crate) const DMA_BUF_PLANE2_MODIFIER_LO_EXT: Int = 0x3447;
    pub(crate) const DMA_BUF_PLANE2_MODIFIER_HI_EXT: Int = 0x3448;
    pub(crate) const DMA_BUF_PLANE3_FD_EXT: Int = 0x3440;
    pub(crate) const DMA_BUF_PLANE3_OFFSET_EXT: Int = 0x3441;
    pub(crate) const DMA_BUF_PLANE3_PITCH_EXT: Int = 0x3442;
    pub(crate) const DMA_BUF_PLANE3_MODIFIER_LO_EXT: Int = 0x3449;
    pub(crate) const DMA_BUF_PLANE3_MODIFIER_HI_EXT: Int = 0x344A;

    pub(crate) const DEVICE_EXT: Int = 0x322C;
    pub(crate) const D3D11_DEVICE_ANGLE: Int = 0x33A1;
    pub(crate) const D3D11_TEXTURE_ANGLE: Enum = 0x3484;
    pub(crate) const BGRA_EXT: Int = 0x80E1;

    pub type EGLDevice = *mut c_void;

    // GLAPI void APIENTRY glEGLImageTargetTexture2DOES (GLenum target, GLeglImageOES image);
    pub(crate) type ImageTargetTexture2DOesFn =
        extern "C" fn(gl::types::GLenum, gl::types::GLeglImageOES);

    pub(crate) fn image_target_texture_2d_oes() -> Option<ImageTargetTexture2DOesFn> {
        unsafe {
            egl()
                .get_proc_address("glEGLImageTargetTexture2DOES")
                .map(|f| std::mem::transmute::<_, ImageTargetTexture2DOesFn>(f))
        }
    }

    // EGLBoolean (GLAPIENTRY *PFNEGLQUERYDISPLAYATTRIBEXTPROC)(EGLDisplay dpy, EGLint attribute, EGLAttrib * value);
    pub(crate) type QueryDisplayAttribFn = extern "C" fn(EGLDisplay, Int, *mut Attrib) -> Boolean;

    pub(crate) fn query_display_attrib() -> Option<QueryDisplayAttribFn> {
        unsafe {
            egl()
                .get_proc_address("eglQueryDisplayAttribEXT")
                .map(|f| std::mem::transmute::<_, QueryDisplayAttribFn>(f))
        }
    }

    // EGLBoolean (GLAPIENTRY *PFNEGLQUERYDEVICEATTRIBEXTPROC)(EGLDeviceEXT device, EGLint attribute, EGLAttrib * value);
    pub(crate) type QueryDeviceAttribFn = extern "C" fn(EGLDevice, Int, *mut Attrib) -> Boolean;

    pub(crate) fn query_device_attrib() -> Option<QueryDeviceAttribFn> {
        unsafe {
            egl()
                .get_proc_address("eglQueryDeviceAttribEXT")
                .map(|f| std::mem::transmute::<_, QueryDeviceAttribFn>(f))
        }
    }

    pub(crate) fn no_context() -> Context {
        unsafe { Context::from_ptr(NO_CONTEXT) }
    }

    pub(crate) fn no_client_buffer() -> ClientBuffer {
        unsafe { ClientBuffer::from_ptr(std::ptr::null_mut()) }
    }

    pub(crate) fn display(ctxt: &gdk::GLContext) -> Option<khronos_egl::Display> {
        #[cfg(any(feature = "wayland", windows, feature = "x11"))]
        use gtk::glib::object::Cast;

        let _dpy = ctxt.display()?;

        #[cfg(feature = "wayland")]
        if let Ok(dpy) = _dpy.clone().downcast::<gdk_wl::WaylandDisplay>() {
            return dpy.egl_display();
        }

        #[cfg(feature = "x11")]
        if let Ok(dpy) = _dpy.clone().downcast::<gdk_x11::X11Display>() {
            return dpy.egl_display();
        };

        #[cfg(windows)]
        if let Ok(dpy) = _dpy.clone().downcast::<gdk_win32::Win32Display>() {
            return dpy.egl_display();
        };

        None
    }
}

#[cfg(not(feature = "bindings"))]
pub use imp::*;

#[cfg(windows)]
#[derive(Debug)]
#[repr(C)]
pub struct RdwD3d11Texture2dScanout {
    pub handle: HANDLE,
    pub tex_width: u32,
    pub tex_height: u32,
    pub y0_top: bool,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[cfg(windows)]
impl Drop for RdwD3d11Texture2dScanout {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};

        unsafe {
            if let Err(e) = CloseHandle(HANDLE(self.handle as _)) {
                log::warn!("Failed to close handle: {:?}", e);
            }
        }
    }
}

/// RdwDmabufScanout:
/// @fd: DMABUF fd, ownership is taken.
///
/// A DMABUF file descriptor along with the associated details.
#[cfg(unix)]
#[derive(Debug)]
#[repr(C)]
pub struct RdwDmabufScanout {
    pub width: u32,
    pub height: u32,
    pub offset: [u32; 4],
    pub stride: [u32; 4],
    pub fourcc: u32,
    pub modifier: u64,
    pub fd: [RawFd; 4],
    pub y0_top: bool,
    pub num_planes: u32,
}

#[cfg(unix)]
impl Drop for RdwDmabufScanout {
    fn drop(&mut self) {
        for &fd in &self.fd[..self.num_planes as usize] {
            if fd >= 0 {
                unsafe {
                    libc::close(fd);
                }
            }
        }
    }
}
