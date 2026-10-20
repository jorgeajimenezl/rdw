//! A source that feeds frames through rdw's public API instead of a remote protocol: a writer
//! draws into three slots of a memfd, and on each frame-clock tick the newest slot goes to the
//! display with its damage against the previous frame, either as the slot's DMABUF (one
//! udmabuf per slot, `set_dmabuf_scanout`, then `update_area` without data, which rdw copies on
//! the GPU from its kept import) or as pixels (`update_area` with data, the GL damage upload).
//!
//! ```text
//! cargo run --release -p rdw4 --example external_source -- [--dmabuf] [--full] [--check]
//!     [--size WxH] [--seconds N]
//! ```
//!
//! - `--dmabuf`: hand rdw each slot's DMABUF (default: the damaged pixels).
//! - `--full`: repaint the whole frame each tick (default: a moving 100 px square).
//! - `--check`: every 30 frames, render the display offscreen and compare the square's centre
//!   with what was written. The colour then changes every frame, which flashes on screen.
//! - `--seconds N`: quit after N seconds, printing the totals.
//!
//! Each second prints frames/s and the process's CPU (user + system, and system alone).

use std::{
    cell::{Cell, RefCell},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    rc::Rc,
    time::Instant,
};

use rdw4::{
    gtk::{self, cairo, gdk, glib, prelude::*},
    DisplayExt, PixelFormat, RdwDmabufScanout,
};

const SLOTS: usize = 3;
const SQUARE: i32 = 100;
const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
const DRM_FORMAT_MOD_LINEAR: u64 = 0;
const BACKGROUND: u32 = 0xff20_2020;
// Distinct square colours, one per frame in turn under --check, so a stale frame shows the
// wrong one.
const COLOURS: [u32; 7] = [
    0xffff_0000,
    0xff00_ff00,
    0xff00_00ff,
    0xffff_ff00,
    0xff00_ffff,
    0xffff_00ff,
    0xffff_ffff,
];

#[derive(Clone, Copy, Debug)]
struct Options {
    dmabuf: bool,
    full: bool,
    check: bool,
    width: i32,
    height: i32,
    seconds: Option<u64>,
}

fn parse_options() -> Options {
    let mut o = Options {
        dmabuf: false,
        full: false,
        check: false,
        width: 1920,
        height: 1080,
        seconds: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dmabuf" => o.dmabuf = true,
            "--upload" => o.dmabuf = false,
            "--full" => o.full = true,
            "--check" => o.check = true,
            "--size" => {
                let v = args.next().expect("--size WxH");
                let (w, h) = v.split_once('x').expect("--size WxH");
                o.width = w.parse().expect("width");
                o.height = h.parse().expect("height");
            }
            "--seconds" => o.seconds = Some(args.next().expect("--seconds N").parse().unwrap()),
            other => panic!("unknown argument {other}"),
        }
    }
    o
}

/// Three XRGB8888 slots in one sealed memfd: stride aligned to 256, offsets to pages.
struct Slots {
    memfd: OwnedFd,
    map: *mut u8,
    len: usize,
    stride: usize,
    slot_size: usize,
}

impl Slots {
    fn new(width: i32, height: i32) -> Self {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let stride = (width as usize * 4).next_multiple_of(256);
        let slot_size = (stride * height as usize).next_multiple_of(page);
        let len = slot_size * SLOTS;
        unsafe {
            let fd = libc::memfd_create(
                c"rdw-external-source".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            );
            assert!(fd >= 0, "memfd_create: {}", std::io::Error::last_os_error());
            let memfd = OwnedFd::from_raw_fd(fd);
            assert_eq!(libc::ftruncate(fd, len as _), 0);
            // udmabuf wants the memfd sealed against shrinking.
            assert_eq!(
                libc::fcntl(
                    fd,
                    libc::F_ADD_SEALS,
                    libc::F_SEAL_SHRINK | libc::F_SEAL_GROW
                ),
                0
            );
            let map = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            assert_ne!(map, libc::MAP_FAILED);
            Self {
                memfd,
                map: map as *mut u8,
                len,
                stride,
                slot_size,
            }
        }
    }

    fn slot(&self, i: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.map.add(i * self.slot_size), self.slot_size) }
    }

    fn slot_mut(&mut self, i: usize) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.map.add(i * self.slot_size), self.slot_size) }
    }

    fn fill(&mut self, i: usize, r: &cairo::RectangleInt, colour: u32) {
        let stride = self.stride;
        let slot = self.slot_mut(i);
        let px = colour.to_le_bytes();
        for y in r.y()..r.y() + r.height() {
            let row = &mut slot[y as usize * stride..];
            for x in r.x()..r.x() + r.width() {
                row[x as usize * 4..x as usize * 4 + 4].copy_from_slice(&px);
            }
        }
    }
}

impl Drop for Slots {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.map as _, self.len) };
    }
}

/// A udmabuf over `size` bytes of the memfd from `offset`.
fn udmabuf(memfd: RawFd, offset: usize, size: usize) -> std::io::Result<OwnedFd> {
    #[repr(C)]
    struct UdmabufCreate {
        memfd: u32,
        flags: u32,
        offset: u64,
        size: u64,
    }
    const UDMABUF_FLAGS_CLOEXEC: u32 = 1;
    // _IOW('u', 0x42, struct udmabuf_create)
    const UDMABUF_CREATE: libc::c_ulong = 0x4018_7542;

    let dev = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/udmabuf")?;
    let create = UdmabufCreate {
        memfd: memfd as u32,
        flags: UDMABUF_FLAGS_CLOEXEC,
        offset: offset as u64,
        size: size as u64,
    };
    let fd = unsafe { libc::ioctl(dev.as_raw_fd(), UDMABUF_CREATE, &create) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// One udmabuf per slot, kept for the session.
fn slot_dmabufs(slots: &Slots) -> Vec<OwnedFd> {
    (0..SLOTS)
        .map(|i| {
            udmabuf(
                slots.memfd.as_raw_fd(),
                i * slots.slot_size,
                slots.slot_size,
            )
            .expect("udmabuf")
        })
        .collect()
}

/// A scanout of a slot's udmabuf, with an fd of its own for rdw to take.
fn slot_scanout(fd: &OwnedFd, slots: &Slots, o: &Options) -> RdwDmabufScanout {
    use std::os::fd::IntoRawFd;
    RdwDmabufScanout {
        width: o.width as u32,
        height: o.height as u32,
        offset: [0; 4],
        stride: [slots.stride as u32, 0, 0, 0],
        fourcc: DRM_FORMAT_XRGB8888,
        modifier: DRM_FORMAT_MOD_LINEAR,
        fd: [fd.try_clone().unwrap().into_raw_fd(), -1, -1, -1],
        y0_top: false,
        num_planes: 1,
    }
}

/// The writer's and the reader's state across ticks.
struct Source {
    o: Options,
    slots: Slots,
    dmabufs: Vec<OwnedFd>,
    frame: u64,
    /// The square in each slot, to erase before drawing the next one there.
    square_in: [Option<cairo::RectangleInt>; SLOTS],
    /// The square the display shows.
    shown: Option<cairo::RectangleInt>,
}

impl Source {
    fn square(&self, frame: u64) -> cairo::RectangleInt {
        let span = (self.o.width - SQUARE).max(1) as u64;
        let x = (frame * 7 % (2 * span)) as i32;
        let x = if x as u64 > span {
            2 * span as i32 - x
        } else {
            x
        };
        cairo::RectangleInt::new(x, (self.o.height - SQUARE) / 2, SQUARE, SQUARE)
    }

    /// The square's colour (the whole frame's with `--full`): steady, or with `--check` a new
    /// one each frame, so a stale frame shows the wrong one. That flashes on screen.
    fn colour(&self, frame: u64) -> u32 {
        if self.o.check {
            COLOURS[frame as usize % COLOURS.len()]
        } else {
            COLOURS[0]
        }
    }

    /// Writes the next frame into its slot; returns the slot and the damage against the frame
    /// the display shows.
    fn write(&mut self) -> (usize, Vec<cairo::RectangleInt>) {
        let frame = self.frame;
        let i = frame as usize % SLOTS;
        let whole = cairo::RectangleInt::new(0, 0, self.o.width, self.o.height);
        if self.o.full {
            self.slots.fill(i, &whole, self.colour(frame));
            self.square_in[i] = None;
            return (i, vec![whole]);
        }
        match self.square_in[i] {
            Some(old) => self.slots.fill(i, &old, BACKGROUND),
            None => self.slots.fill(i, &whole, BACKGROUND),
        }
        let square = self.square(frame);
        self.slots.fill(i, &square, self.colour(frame));
        self.square_in[i] = Some(square);
        let damage = match self.shown.replace(square) {
            Some(prev) => vec![prev, square],
            None => vec![whole],
        };
        (i, damage)
    }
}

/// The process's user + system and system CPU time, in clock ticks.
fn cpu_ticks() -> (u64, u64) {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
    let f: Vec<u64> = rest
        .split_whitespace()
        .skip(11)
        .take(2)
        .filter_map(|v| v.parse().ok())
        .collect();
    match f.as_slice() {
        [u, s] => (u + s, *s),
        _ => (0, 0),
    }
}

/// Renders the display's picture offscreen and reads the pixel at frame position (x, y).
fn read_back(display: &rdw4::Display, o: &Options, x: i32, y: i32) -> Option<u32> {
    let picture = display.first_child()?;
    let (w, h) = (picture.width(), picture.height());
    if w <= 0 || h <= 0 {
        return None;
    }
    let snapshot = gtk::Snapshot::new();
    display.snapshot_child(&picture, &snapshot);
    let node = snapshot.to_node()?;
    let texture = display.native()?.renderer()?.render_texture(node, None);
    let mut down = gdk::TextureDownloader::new(&texture);
    down.set_format(gdk::MemoryFormat::B8g8r8a8);
    let (bytes, stride) = down.download_bytes();
    let px = (x as i64 * texture.width() as i64 / o.width as i64) as usize;
    let py = (y as i64 * texture.height() as i64 / o.height as i64) as usize;
    let at = py * stride + px * 4;
    let b = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) | 0xff00_0000)
}

fn main() -> glib::ExitCode {
    let o = parse_options();
    let app = gtk::Application::builder()
        .application_id("org.gnome.rdw.ExternalSource")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| activate(app, o));
    // GTK must not parse our options.
    app.run_with_args(&["external_source"])
}

fn activate(app: &gtk::Application, o: Options) {
    let display: rdw4::Display = glib::Object::new();
    display.set_display_pixel_format(PixelFormat::Bgrx);
    display.set_display_size(Some((o.width as usize, o.height as usize)));
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("rdw external source")
        .default_width(1280)
        .default_height(720)
        .child(&display)
        .build();
    window.present();

    let slots = Slots::new(o.width, o.height);
    let dmabufs = if o.dmabuf {
        slot_dmabufs(&slots)
    } else {
        Vec::new()
    };
    let source = Rc::new(RefCell::new(Source {
        o,
        slots,
        dmabufs,
        frame: 0,
        square_in: [None; SLOTS],
        shown: None,
    }));

    let start = Instant::now();
    let second = Cell::new((Instant::now(), 0u64, cpu_ticks()));
    let checks = Cell::new((0u32, 0u32));
    let totals = Cell::new((0u64, cpu_ticks()));
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let import_failed = Cell::new(false);
    let app = app.clone();

    display.add_tick_callback(move |display, _clock| {
        let mut s = source.borrow_mut();
        let (i, damage) = s.write();
        if s.o.dmabuf {
            // The same buffer again imports nothing new.
            display.set_dmabuf_scanout(slot_scanout(&s.dmabufs[i], &s.slots, &s.o));
            for r in &damage {
                display.update_area(r.x(), r.y(), r.width(), r.height(), 0, None);
            }
            if display.dmabuf_import_failed() && !import_failed.replace(true) {
                eprintln!("rdw's DMABUF import failed: a texture per update");
            }
        } else {
            let stride = s.slots.stride;
            let slot = s.slots.slot(i);
            for r in &damage {
                let at = r.y() as usize * stride + r.x() as usize * 4;
                display.update_area(
                    r.x(),
                    r.y(),
                    r.width(),
                    r.height(),
                    stride as i32,
                    Some(&slot[at..]),
                );
            }
        }

        if s.o.check && s.frame % 30 == 29 {
            let (x, y) = if s.o.full {
                (s.o.width / 2, s.o.height / 2)
            } else {
                let sq = s.square(s.frame);
                (sq.x() + SQUARE / 2, sq.y() + SQUARE / 2)
            };
            let expected = s.colour(s.frame);
            let (n, stale) = checks.get();
            match read_back(display, &s.o, x, y) {
                Some(got) if got == expected => checks.set((n + 1, stale)),
                got => {
                    eprintln!("frame {}: expected {expected:#010x}, got {got:x?}", s.frame);
                    checks.set((n + 1, stale + 1));
                }
            }
        }
        s.frame += 1;

        let (since, frames, cpu0) = second.get();
        let elapsed = since.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            let cpu = cpu_ticks();
            let frames = s.frame - frames;
            println!(
                "{:.0} fps, {:.0} % CPU ({:.0} % system)",
                frames as f64 / elapsed,
                (cpu.0 - cpu0.0) as f64 / hz / elapsed * 100.0,
                (cpu.1 - cpu0.1) as f64 / hz / elapsed * 100.0,
            );
            second.set((Instant::now(), s.frame, cpu));
        }

        if let Some(secs) = s.o.seconds {
            let run = start.elapsed().as_secs_f64();
            if run >= secs as f64 {
                let (f0, c0) = totals.get();
                let cpu = cpu_ticks();
                let (n, stale) = checks.get();
                println!(
                    "total: {:.1} fps, {:.1} % CPU ({:.1} % system), checks {stale} stale of {n}",
                    (s.frame - f0) as f64 / run,
                    (cpu.0 - c0.0) as f64 / hz / run * 100.0,
                    (cpu.1 - c0.1) as f64 / hz / run * 100.0,
                );
                app.quit();
                return glib::ControlFlow::Break;
            }
        }
        glib::ControlFlow::Continue
    });
}
