use glib::bitflags::bitflags;

use gtk::glib::{
    self,
    translate::{from_glib, FromGlib, IntoGlib, ToGlibPtr, ToGlibPtrMut},
    types::StaticType,
    value::*,
    Type,
};

use crate::ffi;
use gtk::gdk;

/// cbindgen:ignore
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Clone, Copy)]
#[non_exhaustive]
#[repr(C)]
pub enum Scroll {
    Up,
    Down,
    Left,
    Right,
    __Unknown(i32),
}

impl IntoGlib for Scroll {
    type GlibType = ffi::RdwScroll;

    fn into_glib(self) -> ffi::RdwScroll {
        match self {
            Scroll::Up => ffi::RDW_SCROLL_UP,
            Scroll::Down => ffi::RDW_SCROLL_DOWN,
            Scroll::Left => ffi::RDW_SCROLL_LEFT,
            Scroll::Right => ffi::RDW_SCROLL_RIGHT,
            Scroll::__Unknown(v) => v,
        }
    }
}

impl FromGlib<ffi::RdwScroll> for Scroll {
    unsafe fn from_glib(value: ffi::RdwScroll) -> Self {
        match value {
            ffi::RDW_SCROLL_UP => Self::Up,
            ffi::RDW_SCROLL_DOWN => Self::Down,
            ffi::RDW_SCROLL_LEFT => Self::Left,
            ffi::RDW_SCROLL_RIGHT => Self::Right,
            value => Self::__Unknown(value),
        }
    }
}

impl StaticType for Scroll {
    fn static_type() -> Type {
        unsafe { from_glib(ffi::rdw_scroll_get_type()) }
    }
}

impl ValueType for Scroll {
    type Type = Self;
}

unsafe impl<'a> FromValue<'a> for Scroll {
    type Checker = GenericValueTypeChecker<Self>;

    unsafe fn from_value(value: &'a Value) -> Self {
        from_glib(glib::gobject_ffi::g_value_get_enum(
            ToGlibPtr::to_glib_none(value).0,
        ))
    }
}

impl ToValue for Scroll {
    fn to_value(&self) -> Value {
        let mut value = Value::for_value_type::<Self>();
        unsafe {
            glib::gobject_ffi::g_value_set_enum(
                ToGlibPtrMut::to_glib_none_mut(&mut value).0,
                IntoGlib::into_glib(*self),
            )
        }
        value
    }

    fn value_type(&self) -> Type {
        <Self as StaticType>::static_type()
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct Grab: u32 {
        const MOUSE = ffi::RDW_GRAB_MOUSE;
        const KEYBOARD = ffi::RDW_GRAB_KEYBOARD;
    }
}

impl IntoGlib for Grab {
    type GlibType = ffi::RdwGrab;

    fn into_glib(self) -> ffi::RdwGrab {
        self.bits()
    }
}

impl FromGlib<ffi::RdwGrab> for Grab {
    unsafe fn from_glib(value: ffi::RdwGrab) -> Self {
        Grab::from_bits_truncate(value)
    }
}

impl StaticType for Grab {
    fn static_type() -> Type {
        unsafe { from_glib(ffi::rdw_grab_get_type()) }
    }
}

impl ValueType for Grab {
    type Type = Self;
}

unsafe impl<'a> FromValue<'a> for Grab {
    type Checker = GenericValueTypeChecker<Self>;

    unsafe fn from_value(value: &'a Value) -> Self {
        from_glib(glib::gobject_ffi::g_value_get_flags(
            ToGlibPtr::to_glib_none(value).0,
        ))
    }
}

impl ToValue for Grab {
    fn to_value(&self) -> Value {
        let mut value = Value::for_value_type::<Self>();
        unsafe {
            glib::gobject_ffi::g_value_set_flags(
                ToGlibPtrMut::to_glib_none_mut(&mut value).0,
                IntoGlib::into_glib(*self),
            )
        }
        value
    }

    fn value_type(&self) -> Type {
        <Self as StaticType>::static_type()
    }
}

impl std::default::Default for Grab {
    fn default() -> Self {
        Self::empty()
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct KeyEvent: u32 {
        const PRESS = ffi::RDW_KEY_EVENT_PRESS;
        const RELEASE = ffi::RDW_KEY_EVENT_RELEASE;
    }
}

impl IntoGlib for KeyEvent {
    type GlibType = ffi::RdwKeyEvent;

    fn into_glib(self) -> ffi::RdwKeyEvent {
        self.bits()
    }
}

impl FromGlib<ffi::RdwKeyEvent> for KeyEvent {
    unsafe fn from_glib(value: ffi::RdwKeyEvent) -> Self {
        KeyEvent::from_bits_truncate(value)
    }
}

impl StaticType for KeyEvent {
    fn static_type() -> Type {
        unsafe { from_glib(ffi::rdw_key_event_get_type()) }
    }
}

impl ValueType for KeyEvent {
    type Type = Self;
}

unsafe impl<'a> FromValue<'a> for KeyEvent {
    type Checker = GenericValueTypeChecker<Self>;

    unsafe fn from_value(value: &'a Value) -> Self {
        from_glib(glib::gobject_ffi::g_value_get_flags(
            ToGlibPtr::to_glib_none(value).0,
        ))
    }
}

impl ToValue for KeyEvent {
    fn to_value(&self) -> Value {
        let mut value = Value::for_value_type::<Self>();
        unsafe {
            glib::gobject_ffi::g_value_set_flags(
                ToGlibPtrMut::to_glib_none_mut(&mut value).0,
                IntoGlib::into_glib(*self),
            )
        }
        value
    }

    fn value_type(&self) -> Type {
        <Self as StaticType>::static_type()
    }
}

/// GLES stores BGRA through EXT_texture_format_BGRA8888, in BGRA storage only.
const BGRA_EXT: u32 = 0x80E1;

/// Pixel formats supported by `Paintable`.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Clone, Copy, Default)]
#[non_exhaustive]
#[repr(C)]
pub enum PixelFormat {
    #[default]
    Bgra,
    Rgba,
    Bgrx,
    /// Four 16-bit floats, R first (DRM `ABGR16161616F`).
    Rgba16f,
    __Unknown(i32),
}

impl PixelFormat {
    pub fn as_opengl(self) -> u32 {
        match self {
            PixelFormat::Rgba | PixelFormat::Rgba16f => gl::RGBA,
            PixelFormat::Bgra | PixelFormat::Bgrx | _ => gl::BGRA,
        }
    }

    /// The GL type of a component.
    pub fn gl_type(self) -> u32 {
        match self {
            PixelFormat::Rgba16f => gl::HALF_FLOAT,
            _ => gl::UNSIGNED_BYTE,
        }
    }

    /// Bytes per pixel.
    pub fn bpp(self) -> usize {
        match self {
            PixelFormat::Rgba16f => 8,
            _ => 4,
        }
    }

    pub fn has_alpha(self) -> bool {
        !matches!(self, PixelFormat::Bgrx)
    }

    /// The format of a memory texture of these pixels.
    pub(crate) fn memory_format(self) -> gdk::MemoryFormat {
        match self {
            PixelFormat::Bgra => gdk::MemoryFormat::B8g8r8a8Premultiplied,
            PixelFormat::Rgba => gdk::MemoryFormat::R8g8b8a8Premultiplied,
            PixelFormat::Bgrx => gdk::MemoryFormat::B8g8r8x8,
            PixelFormat::Rgba16f => gdk::MemoryFormat::R16g16b16a16FloatPremultiplied,
            other => {
                log::warn!("Unrecognized pixel format {other:?}, falling back to B8G8R8A8");
                gdk::MemoryFormat::B8g8r8a8Premultiplied
            }
        }
    }

    /// The internal format of a GL texture's storage for these pixels.
    pub(crate) fn gl_internal_format(self, gles: bool) -> u32 {
        match (gles, self) {
            (_, PixelFormat::Rgba16f) => gl::RGBA16F,
            (true, f) if f.as_opengl() == gl::BGRA => BGRA_EXT,
            _ => gl::RGBA8,
        }
    }

    /// What GTK is told that storage holds.
    pub(crate) fn gl_memory_format(self, gles: bool) -> gdk::MemoryFormat {
        use gdk::MemoryFormat as M;
        match (self.gl_internal_format(gles), self.has_alpha()) {
            (gl::RGBA16F, _) => M::R16g16b16a16Float,
            (BGRA_EXT, true) => M::B8g8r8a8,
            (BGRA_EXT, false) => M::B8g8r8x8,
            (_, true) => M::R8g8b8a8,
            (_, false) => M::R8g8b8x8,
        }
    }
}

impl IntoGlib for PixelFormat {
    type GlibType = ffi::RdwPixelFormat;

    fn into_glib(self) -> ffi::RdwPixelFormat {
        match self {
            PixelFormat::Bgra => ffi::RDW_PIXEL_FORMAT_BGRA,
            PixelFormat::Rgba => ffi::RDW_PIXEL_FORMAT_RGBA,
            PixelFormat::Bgrx => ffi::RDW_PIXEL_FORMAT_BGRX,
            PixelFormat::Rgba16f => ffi::RDW_PIXEL_FORMAT_RGBA16F,
            PixelFormat::__Unknown(v) => v,
        }
    }
}

impl FromGlib<ffi::RdwPixelFormat> for PixelFormat {
    unsafe fn from_glib(value: ffi::RdwPixelFormat) -> Self {
        match value {
            ffi::RDW_PIXEL_FORMAT_BGRA => Self::Bgra,
            ffi::RDW_PIXEL_FORMAT_RGBA => Self::Rgba,
            ffi::RDW_PIXEL_FORMAT_BGRX => Self::Bgrx,
            ffi::RDW_PIXEL_FORMAT_RGBA16F => Self::Rgba16f,
            value => Self::__Unknown(value),
        }
    }
}

impl StaticType for PixelFormat {
    fn static_type() -> Type {
        unsafe { from_glib(ffi::rdw_pixel_format_get_type()) }
    }
}

impl ValueType for PixelFormat {
    type Type = Self;
}

unsafe impl<'a> FromValue<'a> for PixelFormat {
    type Checker = GenericValueTypeChecker<Self>;

    unsafe fn from_value(value: &'a Value) -> Self {
        from_glib(glib::gobject_ffi::g_value_get_enum(
            ToGlibPtr::to_glib_none(value).0,
        ))
    }
}

impl ToValue for PixelFormat {
    fn to_value(&self) -> Value {
        let mut value = Value::for_value_type::<Self>();
        unsafe {
            glib::gobject_ffi::g_value_set_enum(
                ToGlibPtrMut::to_glib_none_mut(&mut value).0,
                IntoGlib::into_glib(*self),
            )
        }
        value
    }

    fn value_type(&self) -> Type {
        <Self as StaticType>::static_type()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_formats() {
        assert_eq!(PixelFormat::Bgrx.bpp(), 4);
        assert_eq!(PixelFormat::Rgba16f.bpp(), 8);
        assert_eq!(PixelFormat::Rgba16f.gl_type(), gl::HALF_FLOAT);
        assert_eq!(PixelFormat::Bgra.gl_type(), gl::UNSIGNED_BYTE);
        for gles in [false, true] {
            assert_eq!(PixelFormat::Rgba16f.gl_internal_format(gles), gl::RGBA16F);
            assert_eq!(
                PixelFormat::Rgba16f.gl_memory_format(gles),
                gdk::MemoryFormat::R16g16b16a16Float
            );
            assert_eq!(PixelFormat::Rgba.gl_internal_format(gles), gl::RGBA8);
        }
        assert_eq!(PixelFormat::Bgrx.gl_internal_format(true), BGRA_EXT);
        assert_eq!(
            PixelFormat::Bgrx.gl_memory_format(true),
            gdk::MemoryFormat::B8g8r8x8
        );
        assert_eq!(
            PixelFormat::Bgra.gl_memory_format(false),
            gdk::MemoryFormat::R8g8b8a8
        );
    }
}
