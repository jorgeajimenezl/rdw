//! The console's picture while the application shows another source
//! ([`crate::Display::set_external_source`]): recorded, to show again when it's back.

/// The console's latest scanout.
#[derive(Debug, Default)]
pub(crate) enum ConsoleFrame {
    #[default]
    None,
    /// A memory scanout (`Scanout`/`Update`), with its pixels: QEMU sends no update of a still
    /// console, so the pixels shown before the source turned external are needed too.
    Memory {
        width: u32,
        height: u32,
        stride: u32,
        data: Vec<u8>,
    },
    /// A shared-memory scanout, whose mapping is the display's `scanout_map`.
    Map { width: u32, height: u32 },
    /// A DMABUF scanout, with fds of our own.
    #[cfg(unix)]
    Dmabuf(rdw::RdwDmabufScanout),
}

impl ConsoleFrame {
    /// Applies a memory update to the recorded pixels of a memory scanout.
    pub(crate) fn apply_update(
        &mut self,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        src_stride: u32,
        src: &[u8],
    ) {
        if let ConsoleFrame::Memory {
            width,
            height,
            stride,
            data,
        } = self
        {
            copy_rect(
                data,
                *stride as usize,
                (*width, *height),
                (x, y, w, h),
                src,
                src_stride as usize,
            );
        }
    }
}

/// Copies a `w`×`h` rectangle of 32-bit pixels from `src` (its first row first) to (`x`, `y`)
/// of `dst`, clipped to `size` and to both buffers.
pub(crate) fn copy_rect(
    dst: &mut [u8],
    dst_stride: usize,
    size: (u32, u32),
    (x, y, w, h): (u32, u32, u32, u32),
    src: &[u8],
    src_stride: usize,
) {
    let (max_w, max_h) = size;
    if x >= max_w || y >= max_h {
        return;
    }
    let w = w.min(max_w - x) as usize;
    let h = h.min(max_h - y) as usize;
    let (x, y) = (x as usize, y as usize);
    for row in 0..h {
        let s = row * src_stride;
        let d = (y + row) * dst_stride + x * 4;
        let (Some(s), Some(d)) = (src.get(s..s + w * 4), dst.get_mut(d..d + w * 4)) else {
            return;
        };
        d.copy_from_slice(s);
    }
}

/// A copy of a DMABUF scanout with fds of its own.
#[cfg(unix)]
pub(crate) fn dup_dmabuf(s: &rdw::RdwDmabufScanout) -> std::io::Result<rdw::RdwDmabufScanout> {
    use std::os::fd::{BorrowedFd, IntoRawFd};

    let mut fd = [-1; 4];
    for (plane, &raw) in s.fd.iter().enumerate().take(s.num_planes as usize) {
        // On failure, the copy made so far closes what it got.
        let owned = unsafe { BorrowedFd::borrow_raw(raw) }.try_clone_to_owned();
        match owned {
            Ok(owned) => fd[plane] = owned.into_raw_fd(),
            Err(e) => {
                drop(rdw::RdwDmabufScanout {
                    fd,
                    num_planes: plane as u32,
                    ..*s
                });
                return Err(e);
            }
        }
    }
    Ok(rdw::RdwDmabufScanout { fd, ..*s })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32) -> Vec<u8> {
        vec![0; (w * h * 4) as usize]
    }

    fn px(buf: &[u8], stride: usize, x: usize, y: usize) -> u8 {
        buf[y * stride + x * 4]
    }

    #[test]
    fn copy_rect_places_and_clips() {
        let mut dst = frame(4, 4);
        let src = vec![7u8; 3 * 3 * 4];
        copy_rect(&mut dst, 16, (4, 4), (2, 2, 3, 3), &src, 12);
        assert_eq!(px(&dst, 16, 1, 1), 0);
        assert_eq!(px(&dst, 16, 2, 2), 7);
        assert_eq!(px(&dst, 16, 3, 3), 7);
        // Entirely outside: nothing, and no panic.
        copy_rect(&mut dst, 16, (4, 4), (4, 0, 1, 1), &src, 12);
        copy_rect(&mut dst, 16, (4, 4), (0, u32::MAX, 1, 1), &src, 12);
    }

    #[test]
    fn copy_rect_stops_at_a_short_source() {
        let mut dst = frame(4, 4);
        let src = vec![9u8; 4 * 4]; // one row of 4 pixels, but 2 rows asked
        copy_rect(&mut dst, 16, (4, 4), (0, 0, 4, 2), &src, 16);
        assert_eq!(px(&dst, 16, 3, 0), 9);
        assert_eq!(px(&dst, 16, 0, 1), 0);
    }

    #[test]
    fn updates_apply_to_recorded_pixels() {
        let src = vec![5u8; 4];
        let mut f = ConsoleFrame::None;
        f.apply_update(0, 0, 1, 1, 4, &src);
        assert!(matches!(f, ConsoleFrame::None));

        let mut f = ConsoleFrame::Memory {
            width: 2,
            height: 2,
            stride: 8,
            data: frame(2, 2),
        };
        f.apply_update(1, 1, 1, 1, 4, &src);
        let ConsoleFrame::Memory { data: d, .. } = f else {
            unreachable!()
        };
        assert_eq!(px(&d, 8, 1, 1), 5);
        assert_eq!(px(&d, 8, 0, 0), 0);
    }

    #[cfg(unix)]
    #[test]
    fn dup_dmabuf_owns_new_fds() {
        use std::os::fd::AsRawFd;

        let file = std::fs::File::open("/dev/null").unwrap();
        let original = rdw::RdwDmabufScanout {
            width: 1,
            height: 1,
            offset: [0; 4],
            stride: [4, 0, 0, 0],
            fourcc: 0,
            modifier: 0,
            // Keep `file` the owner: the original must not close it.
            fd: [unsafe { libc::dup(file.as_raw_fd()) }, -1, -1, -1],
            y0_top: false,
            num_planes: 1,
        };
        let copy = dup_dmabuf(&original).unwrap();
        assert_ne!(copy.fd[0], original.fd[0]);
        assert_eq!(copy.fd[1], -1);
        drop(copy);
        // The original's fd outlives the copy.
        assert_ne!(unsafe { libc::fcntl(original.fd[0], libc::F_GETFD) }, -1);
    }
}
