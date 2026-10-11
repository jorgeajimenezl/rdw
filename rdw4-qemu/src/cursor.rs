//! Whether the guest shows its (hardware) cursor, from QEMU's `MouseSet` events.
//!
//! virtio-gpu reports `on = resource_id != 0`, so a driver that hides the cursor by detaching
//! the sprite (`UPDATE_CURSOR` with resource 0) sends `MouseSet { on: 0 }`.
//!
//! Some drivers instead park the sprite at the origin to hide it (viogpudo before virtio-win's
//! kvm-guest-drivers-windows#1615: Windows' software-cursor fallback and "hide pointer while
//! typing" both arrive as a `MOVE_CURSOR` to (0,0)). Every `UPDATE_CURSOR` also carries (0,0), but
//! a visible cursor gets its real position right after. So (0,0) hides the cursor after a short
//! grace period unless a real position follows, or unless the host pointer really is at the
//! top-left corner.

use std::time::Duration;

/// How long a (0,0) position may stand before it counts as hidden.
pub(crate) const PARK_GRACE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Visibility {
    Show,
    Hide,
    /// Hide after [`PARK_GRACE`] unless another event decides first.
    HideSoon,
}

/// The guest cursor's visibility after `MouseSet { x, y, on }`.
///
/// `host` is the last absolute position sent to the guest and `hot` the current cursor's hot
/// spot: the guest reports the sprite's top-left corner, i.e. `host - hot`.
pub(crate) fn on_mouse_set(
    x: i32,
    y: i32,
    on: bool,
    host: Option<(i32, i32)>,
    hot: (i32, i32),
) -> Visibility {
    if !on {
        return Visibility::Hide;
    }
    if (x, y) != (0, 0) {
        return Visibility::Show;
    }
    // (0,0) is a real position when the host pointer is at the corner.
    match host {
        Some((hx, hy)) if hx - hot.0 <= 1 && hy - hot.1 <= 1 => Visibility::Show,
        _ => Visibility::HideSoon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_sprite_hides() {
        assert_eq!(
            on_mouse_set(0, 0, false, Some((500, 300)), (0, 0)),
            Visibility::Hide
        );
        assert_eq!(on_mouse_set(12, 40, false, None, (0, 0)), Visibility::Hide);
    }

    #[test]
    fn real_position_shows() {
        assert_eq!(
            on_mouse_set(894, 14, true, Some((894, 14)), (0, 0)),
            Visibility::Show
        );
        assert_eq!(on_mouse_set(1, 0, true, None, (0, 0)), Visibility::Show);
    }

    #[test]
    fn origin_away_from_the_corner_hides_soon() {
        // The software-cursor fallback (I-beam) and hide-while-typing on viogpudo 0.1.302.
        assert_eq!(
            on_mouse_set(0, 0, true, Some((600, 300)), (0, 0)),
            Visibility::HideSoon
        );
        // Attached while the guest cursor is parked: no host position yet.
        assert_eq!(on_mouse_set(0, 0, true, None, (0, 0)), Visibility::HideSoon);
    }

    #[test]
    fn origin_at_the_corner_shows() {
        assert_eq!(
            on_mouse_set(0, 0, true, Some((0, 0)), (0, 0)),
            Visibility::Show
        );
        assert_eq!(
            on_mouse_set(0, 0, true, Some((1, 1)), (0, 0)),
            Visibility::Show
        );
        // A hand cursor (hot spot 6,1) with the host pointer at (6,1) is at the origin too.
        assert_eq!(
            on_mouse_set(0, 0, true, Some((6, 1)), (6, 1)),
            Visibility::Show
        );
        assert_eq!(
            on_mouse_set(0, 0, true, Some((40, 1)), (6, 1)),
            Visibility::HideSoon
        );
    }
}
