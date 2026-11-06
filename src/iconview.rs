//! GtkIconView, measured less often in big folders.
//!
//! GtkIconView answers every size request by measuring all its items: the
//! scrolled window around it asks for its width, its height and (to tell
//! whether a scrollbar is needed) its height at a few widths before it is
//! even laid out, which lays out every item again. In a big folder that's
//! most of the time it takes to show. Those answers only decide the
//! window's smallest size and whether the scrollbar shows, so with many
//! items they're worked out from the item size instead, when that's sure
//! to give the same result. Laying out and drawing are left as they are.

use gtk::prelude::*;
use gtk::subclass::prelude::*;

glib::wrapper! {
    pub struct IconView(ObjectSubclass<imp::IconView>)
        @extends gtk::IconView, gtk::Container, gtk::Widget,
        @implements gtk::Buildable, gtk::CellLayout, gtk::Scrollable;
}

impl IconView {
    pub fn with_model(model: &impl IsA<gtk::TreeModel>) -> gtk::IconView {
        glib::Object::builder::<IconView>().property("model", model.as_ref()).build().upcast()
    }
}

/// Up to this many items, measuring them all costs little: GtkIconView's
/// own answers are kept.
const MEASURED_UP_TO: i32 = 1000;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct IconView;

    #[glib::object_subclass]
    impl ObjectSubclass for IconView {
        const NAME: &'static str = "ArkhaIconView";
        type Type = super::IconView;
        type ParentType = gtk::IconView;
    }

    impl ObjectImpl for IconView {}
    impl ContainerImpl for IconView {}
    impl IconViewImpl for IconView {}

    impl IconView {
        /// The number of items and the narrowest an item can be (its
        /// width and padding), when there are too many to measure.
        fn big(&self) -> Option<(i32, i32)> {
            let iv = self.obj();
            let n = iv.model()?.iter_n_children(None);
            let w = iv.item_width();
            (n > MEASURED_UP_TO && w > 0).then(|| (n, w + 2 * iv.item_padding()))
        }

        /// The least height all items can take at `width`: as many in a
        /// row as fit at their narrowest, each row as low as it can be.
        fn least_height(&self, n: i32, item: i32, width: i32) -> i32 {
            let iv = self.obj();
            let margin = IconViewExt::margin(&*iv);
            let spacing = iv.column_spacing();
            let columns = ((width - 2 * margin + spacing) / (item + spacing)).max(1);
            let rows = (n + columns - 1) / columns;
            let row = 2 * iv.item_padding() + 1;
            rows.saturating_mul(row + iv.row_spacing()) - iv.row_spacing() + 2 * margin
        }
    }

    impl WidgetImpl for IconView {
        fn preferred_width(&self) -> (i32, i32) {
            let Some((n, item)) = self.big() else {
                return self.parent_preferred_width();
            };
            // As GtkIconView: one item at least, all in a row at best.
            let iv = self.obj();
            let margin = IconViewExt::margin(&*iv);
            let all = item.saturating_mul(n).saturating_add(iv.column_spacing().saturating_mul(n - 1));
            (item + 2 * margin, all.saturating_add(2 * margin))
        }

        fn preferred_height(&self) -> (i32, i32) {
            // Only the scrolled window asks, and it scrolls whatever it is.
            let Some((n, item)) = self.big() else {
                return self.parent_preferred_height();
            };
            let h = self.least_height(n, item, item);
            (h.min(item), h)
        }

        fn preferred_height_for_width(&self, width: i32) -> (i32, i32) {
            // The scrolled window asks to tell whether it needs its
            // scrollbar: it does if even the least height is more than
            // the whole window's.
            if let Some((n, item)) = self.big() {
                let least = self.least_height(n, item, width);
                let window = self.obj().toplevel().filter(|t| t.is_toplevel()).map(|t| t.allocated_height());
                if window.is_some_and(|h| least > h) {
                    return (least, least);
                }
            }
            self.parent_preferred_height_for_width(width)
        }
    }
}
