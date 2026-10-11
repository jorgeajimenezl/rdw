//! Whether the guest shows its (hardware) cursor, from QEMU's `MouseSet` events.
//!
//! virtio-gpu reports `on = resource_id != 0`, so a driver that hides the cursor by detaching
//! the sprite (`UPDATE_CURSOR` with resource 0) sends `MouseSet { on: 0 }`.
//!
//! Some drivers instead park the sprite at the origin to hide it (viogpudo before virtio-win's
//! kvm-guest-drivers-windows#1615: Windows' software-cursor fallback and "hide pointer while
//! typing" both arrive as a `MOVE_CURSOR` to (0,0)). So a (0,0) after a move hides the cursor at
//! once, unless the host pointer really is at the top-left corner. `UPDATE_CURSOR` (a
//! `CursorDefine`) also carries (0,0), but a visible cursor gets its real position right after:
//! a (0,0) just after a `CursorDefine` only hides after a short grace period.

use std::time::Duration;

/// How long the (0,0) of a shape update may stand before it counts as hidden.
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
/// `after_define` says whether the previous event was a `CursorDefine`. `host` is the last
/// absolute position sent to the guest and `hot` the current cursor's hot spot: the guest
/// reports the sprite's top-left corner, i.e. `host - hot`.
pub(crate) fn on_mouse_set(
    x: i32,
    y: i32,
    on: bool,
    after_define: bool,
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
        _ if after_define => Visibility::HideSoon,
        _ => Visibility::Hide,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Visibility::*;

    const MOVE: bool = false;
    const DEFINE: bool = true;

    #[test]
    fn detached_sprite_hides() {
        assert_eq!(
            on_mouse_set(0, 0, false, DEFINE, Some((500, 300)), (0, 0)),
            Hide
        );
        assert_eq!(on_mouse_set(12, 40, false, MOVE, None, (0, 0)), Hide);
    }

    #[test]
    fn real_position_shows() {
        assert_eq!(
            on_mouse_set(894, 14, true, MOVE, Some((894, 14)), (0, 0)),
            Show
        );
        assert_eq!(on_mouse_set(1, 0, true, DEFINE, None, (0, 0)), Show);
    }

    #[test]
    fn origin_after_a_move_hides_at_once() {
        // The software-cursor fallback (I-beam) and hide-while-typing on viogpudo 0.1.302.
        assert_eq!(
            on_mouse_set(0, 0, true, MOVE, Some((600, 300)), (0, 0)),
            Hide
        );
        assert_eq!(on_mouse_set(0, 0, true, MOVE, None, (0, 0)), Hide);
    }

    #[test]
    fn origin_of_a_shape_update_hides_soon() {
        // UPDATE_CURSOR's own (0,0): the real position follows if the cursor is visible.
        assert_eq!(
            on_mouse_set(0, 0, true, DEFINE, Some((600, 300)), (0, 0)),
            HideSoon
        );
        // Attached while the guest cursor is parked: the replay is a define, then (0,0).
        assert_eq!(on_mouse_set(0, 0, true, DEFINE, None, (0, 0)), HideSoon);
    }

    #[test]
    fn origin_at_the_corner_shows() {
        assert_eq!(on_mouse_set(0, 0, true, MOVE, Some((0, 0)), (0, 0)), Show);
        assert_eq!(on_mouse_set(0, 0, true, MOVE, Some((1, 1)), (0, 0)), Show);
        // A hand cursor (hot spot 6,1) with the host pointer at (6,1) is at the origin too.
        assert_eq!(on_mouse_set(0, 0, true, MOVE, Some((6, 1)), (6, 1)), Show);
        assert_eq!(on_mouse_set(0, 0, true, MOVE, Some((40, 1)), (6, 1)), Hide);
    }

    /// Entering a text field, as traced on viogpudo 0.1.302: one move with the arrow, then the
    /// I-beam fails and the sprite is parked. No grace period may keep the arrow up.
    #[test]
    fn entering_a_text_field() {
        let host = Some((445, 80));
        let seq = [
            (444, 79, MOVE, Show),
            (0, 0, MOVE, Hide),
            (0, 0, MOVE, Hide),
            (0, 0, DEFINE, HideSoon),
            (444, 79, MOVE, Show),
            (445, 79, MOVE, Show),
            (0, 0, MOVE, Hide),
        ];
        for (x, y, after_define, want) in seq {
            assert_eq!(
                on_mouse_set(x, y, true, after_define, host, (0, 0)),
                want,
                "{x},{y}"
            );
        }
    }
}
