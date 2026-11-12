//! The parts of a tab that stand on their own, made for Pane::new.

use super::*;

/// The model of a folder's items (the COL_* columns), sorted with folders
/// first while `folders_first` says so.
pub(super) fn new_store(folders_first: &Rc<Cell<bool>>) -> gtk::ListStore {
    let store = gtk::ListStore::new(&[
        gio::Icon::static_type(),
        String::static_type(),
        String::static_type(),
        u64::static_type(),
        String::static_type(),
        i64::static_type(),
        String::static_type(),
        String::static_type(),
        bool::static_type(),
        bool::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
        String::static_type(),
        i64::static_type(),
        i64::static_type(),
        String::static_type(),
        String::static_type(),
        u32::static_type(),
        u64::static_type(),
        i64::static_type(),
    ]);

    // Sorting (folders first, regardless of direction). A big folder
    // takes tens of thousands of comparisons: the names, dearest to
    // read, only on a tie.
    for id in 0..crate::details::sort_count() {
        let ff = folders_first.clone();
        store.set_sort_func(gtk::SortColumn::Index(id), move |m, a, b| {
            if ff.get() {
                let da: bool = m.gv(a, COL_IS_DIR as i32);
                let db: bool = m.gv(b, COL_IS_DIR as i32);
                if da != db {
                    let desc = m
                        .dynamic_cast_ref::<gtk::TreeSortable>()
                        .and_then(|s| s.sort_column_id())
                        .is_some_and(|(_, t)| t == gtk::SortType::Descending);
                    let o = if da { Ordering::Less } else { Ordering::Greater };
                    return if desc { o.reverse() } else { o };
                }
            }
            let o = if id == 0 { Ordering::Equal } else { crate::details::cmp(crate::details::sort_name(id), m, a, b) };
            o.then_with(|| cmp_text(m, a, b, COL_KEY))
        });
    }
    store
}

/// The icon view, for the filter's row-deleted fix, once it's made.
pub(super) type IconViewRef = Rc<RefCell<Option<glib::WeakRef<gtk::IconView>>>>;

/// The filter over `store` (hidden items left out unless `show_hidden`),
/// and where the icon view is to be noted for its fix (see below).
pub(super) fn new_filter_for(store: &gtk::ListStore, show_hidden: &Rc<Cell<bool>>) -> (gtk::TreeModelFilter, IconViewRef) {
    let filter = new_filter(store);
    {
        let sh = show_hidden.clone();
        filter.set_visible_func(move |m, it| sh.get() || !m.gv::<bool>(it, COL_HIDDEN as i32));
    }

    // GtkIconView's row-deleted handler (GTK 3.24) gives up a reference
    // on the row that moves into the removed one's place (it looks the
    // path up after the removal), and it only holds references on rows
    // added while it's attached, not on those there when it gets the
    // model. Uncorrected, the filter model's counts go below zero (the
    // "elt->ref_count > 0" assertion in gtk_tree_model_filter_real_unref_node,
    // on removing a row or later on freeing a row reference). This
    // handler, connected before the icon view's (so it runs first), gives
    // that row the reference the icon view then takes away.
    let icon_view_ref: IconViewRef = Rc::default();
    {
        let iv = icon_view_ref.clone();
        filter.connect_row_deleted(move |m, path| {
            let attached = iv.borrow().as_ref().and_then(|w| w.upgrade()).is_some_and(|v| v.model().is_some());
            if !attached || path.depth() != 1 {
                return;
            }
            if let Some(mut next) = m.iter(path) {
                use glib::translate::{ToGlibPtr, ToGlibPtrMut};
                // SAFETY: a valid iter of this model, as GtkIconView passes it.
                unsafe { gtk::ffi::gtk_tree_model_ref_node(m.upcast_ref::<gtk::TreeModel>().to_glib_none().0, next.to_glib_none_mut().0) };
            }
        });
    }
    (filter, icon_view_ref)
}

/// The list view's columns: the name, and one for every detail.
pub(super) struct ListColumns {
    pub name_col: gtk::TreeViewColumn,
    pub list_pix: gtk::CellRendererPixbuf,
    pub list_text: gtk::CellRendererText,
    /// Whether the list view is the columns view's (folders get an arrow).
    pub columns_mode: Rc<Cell<bool>>,
    pub detail_cols: Vec<(&'static str, gtk::TreeViewColumn)>,
    pub fit_cols: Vec<(gtk::TreeViewColumn, &'static str, bool)>,
    pub sort_cols: Vec<(u32, gtk::TreeViewColumn)>,
}

/// The name column, put in `tree_view` (icons of `list_size`), and the
/// detail columns, put in when first shown (see apply_column_choice).
pub(super) fn list_columns(tree_view: &gtk::TreeView, list_size: &Rc<Cell<i32>>) -> ListColumns {
    let name_col = gtk::TreeViewColumn::new();
    name_col.set_title(&tr("Name"));
    name_col.set_expand(true);
    name_col.set_resizable(true);
    name_col.set_min_width(200);
    let list_pix = gtk::CellRendererPixbuf::new();
    let list_text = gtk::CellRendererText::new();
    list_text.set_property("ellipsize", pango::EllipsizeMode::End);
    CellLayoutExt::pack_start(&name_col, &list_pix, false);
    CellLayoutExt::pack_start(&name_col, &list_text, true);
    CellLayoutExt::add_attribute(&name_col, &list_text, "text", COL_NAME as i32);
    {
        let sz = list_size.clone();
        CellLayoutExt::set_cell_data_func(
            &name_col,
            &list_pix,
            Some(Box::new(move |_, cell, m, it| {
                // Its size is fixed: the picture is needed only when drawn.
                cell.set_property("pixbuf", if icons::drawing() { row_pixbuf(m, it, sz.get()) } else { None });
            })),
        );
    }
    // In the columns view, folders get an arrow, as in Finder.
    let columns_mode = Rc::new(Cell::new(false));
    {
        let arrow = gtk::CellRendererPixbuf::new();
        arrow.set_property("icon-name", "go-next-symbolic");
        CellLayoutExt::pack_end(&name_col, &arrow, false);
        let on = columns_mode.clone();
        CellLayoutExt::set_cell_data_func(
            &name_col,
            &arrow,
            Some(Box::new(move |_, cell, m, it| {
                cell.set_visible(on.get() && m.gv::<bool>(it, COL_IS_DIR as i32));
            })),
        );
    }
    tree_view.append_column(&name_col);
    // A column for every detail, hidden until chosen. Those whose text
    // the model holds show it; the others make it for the rows drawn.
    let mut detail_cols = vec![];
    for d in crate::details::DETAILS {
        let c = gtk::TreeViewColumn::new();
        c.set_title(&tr(d.title));
        c.set_resizable(true);
        // Made at least as wide as its title when first shown.
        c.set_min_width(d.min_width);
        c.set_reorderable(true);
        let r = gtk::CellRendererText::new();
        r.set_xalign(d.xalign);
        r.set_property("ellipsize", pango::EllipsizeMode::End);
        CellLayoutExt::pack_start(&c, &r, true);
        let held = match d.id {
            "size" => Some(COL_SIZE_TEXT),
            "type" => Some(COL_TYPE),
            "mtime" => Some(COL_MTIME_TEXT),
            "orig" => Some(COL_ORIG),
            _ => None,
        };
        match held {
            Some(col) => CellLayoutExt::add_attribute(&c, &r, "text", col as i32),
            None => {
                let id = d.id;
                CellLayoutExt::set_cell_data_func(
                    &c,
                    &r,
                    Some(Box::new(move |_, cell, m, it| cell.set_property("text", crate::details::text(id, m, it)))),
                );
            }
        }
        c.set_visible(false);
        // The list's rows are all as high (fixed height mode), which
        // takes columns of a set width.
        c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        c.set_fixed_width(d.min_width);
        // Put in the list view when first shown (apply_column_choice):
        // with the usual columns, it has just those.
        detail_cols.push((d.id, c));
    }
    let mut fit_cols = vec![(name_col.clone(), "name", true)];
    fit_cols.extend(detail_cols.iter().map(|(id, c)| (c.clone(), *id, false)));
    let mut sort_cols = vec![(0, name_col.clone())];
    sort_cols.extend(detail_cols.iter().map(|(id, c)| (crate::details::sort_id(id), c.clone())));
    for (_, c) in &sort_cols {
        c.set_clickable(true);
    }
    ListColumns { name_col, list_pix, list_text, columns_mode, detail_cols, fit_cols, sort_cols }
}

/// The bar over the Trash's items, with Restore and Empty Trash. The
/// window's actions do the work (and say when they can). No mnemonics:
/// Alt+E is the Edit menu.
pub(super) fn trash_bar() -> gtk::InfoBar {
    let trash_bar = gtk::InfoBar::new();
    trash_bar.set_message_type(gtk::MessageType::Other);
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::new(Some(&tr("Trash")));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.style_context().add_class("dim-label");
        let restore = gtk::Button::with_label(&tr("Restore Selected Items"));
        restore.set_action_name(Some("win.restore"));
        restore.set_tooltip_text(Some(&tr("Move the selected items back to where they were deleted from")));
        let empty = gtk::Button::with_label(&tr("Empty Trash"));
        empty.set_action_name(Some("win.empty-trash"));
        empty.set_tooltip_text(Some(&tr("Delete all items in the Trash")));
        row.pack_start(&title, true, true, 0);
        row.pack_start(&restore, false, false, 0);
        row.pack_start(&empty, false, false, 0);
        trash_bar.content_area().add(&row);
    }
    trash_bar.set_no_show_all(true);
    trash_bar.content_area().show_all();
    trash_bar
}
