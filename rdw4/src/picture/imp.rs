use super::*;
use glib::clone;

#[derive(Debug, Default)]
pub struct Picture {
    pub(crate) paintable: Paintable,
}

/// cbindgen:ignore
#[glib::object_subclass]
impl ObjectSubclass for Picture {
    const NAME: &'static str = "RdwPicture";
    type Type = super::Picture;
    type ParentType = gtk::Widget;
}

impl ObjectImpl for Picture {
    fn constructed(&self) {
        self.parent_constructed();
        self.obj().set_focusable(true);

        self.paintable.connect_invalidate_contents(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                this.obj().queue_draw();
            }
        ));
        self.paintable.connect_invalidate_size(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                this.obj().queue_resize();
            }
        ));
        // The device-pixel size follows the scale too.
        self.obj()
            .connect_scale_factor_notify(|obj| obj.queue_allocate());
    }
}

impl WidgetImpl for Picture {
    fn request_mode(&self) -> gtk::SizeRequestMode {
        gtk::SizeRequestMode::HeightForWidth
    }

    fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
        self.parent_size_allocate(width, height, baseline);
        let obj = self.obj();
        // The surface's scale is fractional; the widget's scale factor is rounded up.
        let scale = obj
            .native()
            .and_then(|n| n.surface())
            .map_or(obj.scale_factor() as f64, |s| s.scale());
        self.paintable.set_shown_size(
            (width as f64 * scale).round() as i32,
            (height as f64 * scale).round() as i32,
        );
    }

    fn snapshot(&self, snapshot: &gtk::Snapshot) {
        let (w, h) = (self.obj().width() as _, self.obj().height() as _);
        self.paintable.snapshot(snapshot, w, h);
    }

    fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
        if for_size == 0 {
            return (0, 0, -1, -1);
        }

        let (width, height) = (
            self.paintable.intrinsic_width().max(640),
            self.paintable.intrinsic_height().max(480),
        );

        if orientation == gtk::Orientation::Horizontal {
            let (nat_width, _nat_height) = self.paintable.compute_concrete_size(
                0f64,
                for_size.max(0) as _,
                width as _,
                height as _,
            );
            (0, nat_width.ceil() as _, -1, -1)
        } else {
            let (_nat_width, nat_height) = self.paintable.compute_concrete_size(
                for_size.max(0) as _,
                0f64,
                width as _,
                height as _,
            );
            (0, nat_height.ceil() as _, -1, -1)
        }
    }
}
