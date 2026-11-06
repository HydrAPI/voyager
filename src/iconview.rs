//! GtkIconView, measured less often in big folders.
//!
//! GtkIconView answers every size request by measuring all its items: the
//! scrolled window around it asks for its width, its height and (to tell
//! whether a scrollbar is needed) its height at a few widths before it is
//! even laid out, which lays out every item again. In a big folder that's
//! most of the time it takes to show. Those answers only decide the
//! window's smallest size and whether the scrollbar shows, so with many
//! items they're worked out from the item size instead, when that's sure
//! to give the same result. Laying out is left as it is.
//!
//! Drawing, too, went through every item for every frame, to see which
//! ones are in sight (GtkIconView keeps its items in a list): scrolling a
//! big folder cost more the more items it had. Here only the rows in sight
//! are drawn, found from GtkIconView's own positions, each item painted as
//! gtk_icon_view_paint_item does (GTK 3.24). Whatever this doesn't cover
//! (a drag over the view, a rubber band, right-to-left text, or positions
//! not as expected) is left to GtkIconView. ARKHA_GTK_DRAW=1 always does,
//! to compare.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use glib::translate::*;
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
    pub struct IconView {
        /// The selected items' indices; None when out of date.
        selected: RefCell<Option<HashSet<i32>>>,
        /// The item under the pointer, as GtkIconView's last_prelight.
        prelight: Cell<Option<i32>>,
        /// As GtkIconView's draw_focus: after the keyboard moved the cursor,
        /// until a click.
        draw_focus: Cell<bool>,
        /// Mouse button 1 is down: a rubber band may be drawn.
        pressed: Cell<bool>,
        /// Cell area contexts of rows drawn: (row, first item, items, item
        /// width, row height) → context. Emptied when anything changes.
        contexts: RefCell<HashMap<RowKey, gtk::CellAreaContext>>,
        model_handlers: RefCell<Vec<(gtk::TreeModel, glib::SignalHandlerId)>>,
    }

    /// (row, first item, items, item width, row height).
    type RowKey = (i32, i32, i32, i32, i32);

    /// Where the items are, as gtk_icon_view_layout put them (all in the
    /// bin window's coordinates).
    struct Grid {
        n_items: i32,
        columns: i32,
        item_width: i32,
        padding: i32,
        margin: i32,
        column_spacing: i32,
        offset: (i32, i32),
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconView {
        const NAME: &'static str = "ArkhaIconView";
        type Type = super::IconView;
        type ParentType = gtk::IconView;
    }

    impl ObjectImpl for IconView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.connect_selection_changed(|iv| iv.imp().selected.replace(None).map_or((), drop));
            // Anything set on the view (sizes, spacing, the model) may move
            // the items.
            obj.connect_notify_local(None, |iv, pspec| {
                let imp = iv.imp();
                imp.forget();
                if pspec.name() == "model" {
                    imp.watch_model();
                }
            });
            self.watch_model();
        }

        fn dispose(&self) {
            for (m, id) in self.model_handlers.take() {
                m.disconnect(id);
            }
        }
    }
    impl ContainerImpl for IconView {}
    impl IconViewImpl for IconView {
        fn move_cursor(&self, step: gtk::MovementStep, count: i32) -> bool {
            self.draw_focus.set(true);
            self.parent_move_cursor(step, count)
        }
    }

    impl IconView {
        /// Forgets what depends on the items and where they are.
        fn forget(&self) {
            self.selected.replace(None);
            self.contexts.borrow_mut().clear();
        }

        fn watch_model(&self) {
            for (m, id) in self.model_handlers.take() {
                m.disconnect(id);
            }
            let Some(model) = self.obj().model() else { return };
            let weak = self.obj().downgrade();
            let changed = move || {
                if let Some(iv) = weak.upgrade() {
                    iv.imp().forget();
                    iv.imp().prelight.set(None);
                }
            };
            let mut ids = vec![];
            let c = changed.clone();
            ids.push(model.connect_row_inserted(move |_, _, _| c()));
            let c = changed.clone();
            ids.push(model.connect_row_deleted(move |_, _| c()));
            let c = changed.clone();
            ids.push(model.connect_row_changed(move |_, _, _| c()));
            // (rows-reordered has no binding for a list's top level.)
            let c = changed;
            ids.push(model.connect_local("rows-reordered", false, move |_| {
                c();
                None
            }));
            *self.model_handlers.borrow_mut() = ids.into_iter().map(|id| (model.clone(), id)).collect();
        }

        /// GtkIconView's bin window (where the items are), which it has no
        /// getter for: the child window of its own that it registered.
        fn bin_window(&self) -> Option<gdk::Window> {
            let iv = self.obj();
            let me = iv.as_ptr() as glib::ffi::gpointer;
            iv.window()?.children().into_iter().find(|w| unsafe {
                let mut data = std::ptr::null_mut();
                gdk::ffi::gdk_window_get_user_data(w.to_glib_none().0, &mut data);
                data == me
            })
        }

        /// GtkIconView's rectangle for item `index` (cell_area, without the
        /// padding), in the bin window.
        fn item_rect(&self, grid: &Grid, index: i32) -> Option<gdk::Rectangle> {
            let r = self.obj().cell_rect(&gtk::TreePath::from_indicesv(&[index]), None::<&gtk::CellRenderer>)?;
            let p = grid.padding;
            Some(gdk::Rectangle::new(r.x() + p - grid.offset.0, r.y() + p - grid.offset.1, r.width() - 2 * p, r.height() - 2 * p))
        }

        /// The layout, checked against GtkIconView's positions of the
        /// first items (the columns as gtk_icon_view_layout works them
        /// out); None if it isn't as expected.
        fn grid(&self) -> Option<Grid> {
            let iv = self.obj();
            if iv.direction() == gtk::TextDirection::Rtl || iv.columns() > 0 {
                return None;
            }
            let n_items = iv.model()?.iter_n_children(None);
            let bin = self.bin_window()?;
            let mut grid = Grid {
                n_items,
                columns: 1,
                item_width: 0,
                padding: iv.item_padding(),
                margin: IconViewExt::margin(&*iv),
                column_spacing: iv.column_spacing(),
                offset: bin.position(),
            };
            if n_items == 0 {
                return Some(grid);
            }
            let first = self.item_rect(&grid, 0)?;
            grid.item_width = first.width();
            if first.x() != grid.margin + grid.padding || first.width() <= 0 {
                return None;
            }
            let pitch = grid.item_width + 2 * grid.padding + grid.column_spacing;
            let size = iv.allocated_width() - 2 * grid.margin + grid.column_spacing;
            grid.columns = if size <= pitch { 1 } else { size / pitch };
            // The last item of the first row is beside the first; the next
            // one starts the second row.
            let last = self.item_rect(&grid, (grid.columns - 1).min(n_items - 1))?;
            if last.y() != first.y() || last.x() != self.column_x(&grid, (grid.columns - 1).min(n_items - 1)) {
                return None;
            }
            if n_items > grid.columns {
                let next = self.item_rect(&grid, grid.columns)?;
                if next.x() != first.x() || next.y() <= first.y() {
                    return None;
                }
            }
            Some(grid)
        }

        fn column_x(&self, grid: &Grid, col: i32) -> i32 {
            grid.margin + (col * 2 + 1) * grid.padding + col * (grid.column_spacing + grid.item_width)
        }

        /// Row `row`'s top and height (its first item's), in the bin window.
        fn row_span(&self, grid: &Grid, row: i32) -> Option<(i32, i32)> {
            let r = self.item_rect(grid, row * grid.columns)?;
            Some((r.y(), r.height()))
        }

        fn selected_set(&self) -> HashSet<i32> {
            if let Some(s) = self.selected.borrow().as_ref() {
                return s.clone();
            }
            let s: HashSet<i32> = self.obj().selected_items().iter().filter_map(|p| p.indices().first().copied()).collect();
            self.selected.replace(Some(s.clone()));
            s
        }

        /// The cell area context of a row, as gtk_icon_view_layout makes
        /// one: its items measured at the item width, allocated the row's
        /// height.
        fn row_context(&self, model: &gtk::TreeModel, area: &gtk::CellArea, grid: &Grid, row: i32, height: i32) -> Option<gtk::CellAreaContext> {
            let first = row * grid.columns;
            let count = grid.columns.min(grid.n_items - first);
            let key = (row, first, count, grid.item_width, height);
            if let Some(c) = self.contexts.borrow().get(&key) {
                return Some(c.clone());
            }
            let iv = self.obj();
            let ctx = area.create_context()?;
            for i in first..first + count {
                if let Some(it) = model.iter_nth_child(None, i) {
                    area.apply_attributes(model, &it, false, false);
                    area.preferred_height_for_width(&ctx, &*iv, grid.item_width);
                }
            }
            ctx.allocate(grid.item_width, height);
            let mut contexts = self.contexts.borrow_mut();
            if contexts.len() > 200 {
                contexts.clear();
            }
            contexts.insert(key, ctx.clone());
            Some(ctx)
        }

        /// gtk_icon_view_draw for the bin window, only going through the
        /// rows in sight. False if it can't be done here.
        fn draw_items(&self, cr: &gtk::cairo::Context) -> bool {
            let iv = self.obj();
            if self.pressed.get() || !iv.children().is_empty() || std::env::var_os("ARKHA_GTK_DRAW").is_some() {
                return false;
            }
            // A drop target is marked by GtkIconView.
            let dropping = unsafe {
                let mut path = std::ptr::null_mut();
                let mut pos = std::mem::MaybeUninit::uninit();
                gtk::ffi::gtk_icon_view_get_drag_dest_item(iv.upcast_ref::<gtk::IconView>().to_glib_none().0, &mut path, pos.as_mut_ptr());
                if path.is_null() {
                    false
                } else {
                    gtk::ffi::gtk_tree_path_free(path);
                    true
                }
            };
            if dropping {
                return false;
            }
            let (Some(bin), Some(model)) = (self.bin_window(), iv.model()) else { return false };
            let Some(grid) = self.grid() else { return false };
            let ctx = iv.style_context();
            gtk::render_background(&ctx, cr, 0.0, 0.0, iv.allocated_width() as f64, iv.allocated_height() as f64);
            if grid.n_items == 0 {
                return true;
            }
            let area = CellLayoutExt::area(&*iv);
            let Some(area) = area else { return false };
            cr.save().ok();
            gtk::cairo_transform_to_window(cr, &*iv, &bin);
            cr.set_line_width(1.0);
            let (_, top, _, bottom) = cr.clip_extents().unwrap_or((0.0, 0.0, 0.0, 0.0));
            let p = grid.padding;
            let rows = (grid.n_items + grid.columns - 1) / grid.columns;
            // The first row reaching into sight.
            let (mut lo, mut hi) = (0, rows - 1);
            let mut ok = true;
            while lo < hi {
                let mid = (lo + hi) / 2;
                match self.row_span(&grid, mid) {
                    Some((y, h)) if ((y + h + p) as f64) < top => lo = mid + 1,
                    Some(_) => hi = mid,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                cr.restore().ok();
                return false;
            }
            let state = iv.state_flags();
            let cursor = iv.cursor().and_then(|(path, _)| path.indices().first().copied());
            let selected = self.selected_set();
            let prelight = self.prelight.get();
            let draw_focus = self.draw_focus.get();
            let mut row = lo;
            while row < rows {
                let Some((y, h)) = self.row_span(&grid, row) else { break };
                if ((y - p) as f64) > bottom {
                    break;
                }
                let first = row * grid.columns;
                let count = grid.columns.min(grid.n_items - first);
                let Some(row_ctx) = self.row_context(&model, &area, &grid, row, h) else { break };
                for i in first..first + count {
                    let x = self.column_x(&grid, i - first);
                    self.paint_item(cr, &model, &area, &row_ctx, i, gdk::Rectangle::new(x, y, grid.item_width, h), p, state, cursor, &selected, prelight, draw_focus);
                }
                row += 1;
            }
            cr.restore().ok();
            true
        }

        /// gtk_icon_view_paint_item, clipped to the item as
        /// gtk_icon_view_draw does.
        #[allow(clippy::too_many_arguments)]
        fn paint_item(
            &self,
            cr: &gtk::cairo::Context,
            model: &gtk::TreeModel,
            area: &gtk::CellArea,
            row_ctx: &gtk::CellAreaContext,
            index: i32,
            cell: gdk::Rectangle,
            p: i32,
            widget_state: gtk::StateFlags,
            cursor: Option<i32>,
            selected: &HashSet<i32>,
            prelight: Option<i32>,
            draw_focus: bool,
        ) {
            if cell.width() <= 0 || cell.height() <= 0 {
                return;
            }
            let (px, py, pw, ph) = ((cell.x() - p) as f64, (cell.y() - p) as f64, (cell.width() + 2 * p) as f64, (cell.height() + 2 * p) as f64);
            cr.save().ok();
            cr.rectangle(px, py, pw, ph);
            cr.clip();
            let in_sight = cr.clip_extents().is_ok_and(|(x1, y1, x2, y2)| x2 > x1 && y2 > y1);
            if in_sight {
                if let Some(it) = model.iter_nth_child(None, index) {
                    let iv = self.obj();
                    area.apply_attributes(model, &it, false, false);
                    let ctx = iv.style_context();
                    ctx.save();
                    ctx.add_class("cell");
                    let mut state = widget_state - (gtk::StateFlags::SELECTED | gtk::StateFlags::PRELIGHT);
                    let mut flags = gtk::CellRendererState::empty();
                    if widget_state.contains(gtk::StateFlags::FOCUSED) && cursor == Some(index) {
                        flags |= gtk::CellRendererState::FOCUSED;
                    }
                    if selected.contains(&index) {
                        state |= gtk::StateFlags::SELECTED;
                        flags |= gtk::CellRendererState::SELECTED;
                    }
                    if prelight == Some(index) {
                        state |= gtk::StateFlags::PRELIGHT;
                        flags |= gtk::CellRendererState::PRELIT;
                    }
                    ctx.set_state(state);
                    gtk::render_background(&ctx, cr, px, py, pw, ph);
                    gtk::render_frame(&ctx, cr, px, py, pw, ph);
                    area.render(row_ctx, &*iv, cr, &cell, &cell, flags, draw_focus);
                    ctx.restore();
                }
            }
            cr.restore().ok();
        }

        /// The item at (x, y) in the bin window, as
        /// _gtk_icon_view_get_item_at_coords finds it for the pointer.
        fn item_at(&self, x: f64, y: f64) -> Option<i32> {
            let iv = self.obj();
            let grid = self.grid()?;
            if grid.n_items == 0 {
                return None;
            }
            let (cs, rs) = ((iv.column_spacing() / 2) as f64, (iv.row_spacing() / 2) as f64);
            let rows = (grid.n_items + grid.columns - 1) / grid.columns;
            let (mut lo, mut hi) = (0, rows - 1);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let (ry, rh) = self.row_span(&grid, mid)?;
                if ((ry + rh) as f64 + rs) < y {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let (ry, rh) = self.row_span(&grid, lo)?;
            if y < ry as f64 - rs || y > (ry + rh) as f64 + rs {
                return None;
            }
            let first = lo * grid.columns;
            (first..(first + grid.columns).min(grid.n_items)).find(|&i| {
                let x0 = self.column_x(&grid, i - first) as f64;
                x >= x0 - cs && x <= x0 + grid.item_width as f64 + cs
            })
        }

        /// Follows the pointer as gtk_icon_view_motion does, redrawing the
        /// items it leaves and enters.
        fn set_prelight(&self, item: Option<i32>) {
            let old = self.prelight.replace(item);
            if old != item {
                let iv = self.obj();
                for i in [old, item].into_iter().flatten() {
                    if let Some(r) = iv.cell_rect(&gtk::TreePath::from_indicesv(&[i]), None::<&gtk::CellRenderer>) {
                        iv.queue_draw_area(r.x(), r.y(), r.width(), r.height());
                    }
                }
            }
        }

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
        fn draw(&self, cr: &gtk::cairo::Context) -> glib::Propagation {
            match self.bin_window() {
                Some(bin) if gtk::cairo_should_draw_window(cr, &bin) && self.draw_items(cr) => glib::Propagation::Proceed,
                _ => self.parent_draw(cr),
            }
        }

        fn size_allocate(&self, allocation: &gtk::Allocation) {
            self.parent_size_allocate(allocation);
            self.forget();
        }

        fn motion_notify_event(&self, event: &gdk::EventMotion) -> glib::Propagation {
            let r = self.parent_motion_notify_event(event);
            if !self.pressed.get() {
                let (x, y) = event.position();
                self.set_prelight(self.item_at(x, y));
            }
            r
        }

        fn leave_notify_event(&self, event: &gdk::EventCrossing) -> glib::Propagation {
            self.set_prelight(None);
            self.parent_leave_notify_event(event)
        }

        fn button_press_event(&self, event: &gdk::EventButton) -> glib::Propagation {
            self.draw_focus.set(false);
            if event.button() == 1 {
                self.pressed.set(true);
            }
            self.parent_button_press_event(event)
        }

        fn button_release_event(&self, event: &gdk::EventButton) -> glib::Propagation {
            let r = self.parent_button_release_event(event);
            if event.button() == 1 && self.pressed.replace(false) {
                // What a rubber band covered is drawn again without it.
                self.obj().queue_draw();
            }
            r
        }

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
