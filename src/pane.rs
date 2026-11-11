//! One tab: a folder shown as icons, a list or a compact grid.

use gdk_pixbuf::Pixbuf;
use gio::prelude::*;
use glib::translate::*;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::config;
use crate::folders;
use crate::{clipboard, icons, util};

pub const COL_ICON: u32 = 0;
pub const COL_NAME: u32 = 1;
pub const COL_URI: u32 = 2;
pub const COL_SIZE: u32 = 3;
pub const COL_SIZE_TEXT: u32 = 4;
pub const COL_MTIME: u32 = 5;
pub const COL_MTIME_TEXT: u32 = 6;
pub const COL_TYPE: u32 = 7;
pub const COL_IS_DIR: u32 = 8;
pub const COL_HIDDEN: u32 = 9;
pub const COL_THUMB: u32 = 10;
pub const COL_CTYPE: u32 = 11;
pub const COL_KEY: u32 = 12;
pub const COL_ORIG: u32 = 13;
// More about each item, for the list's columns, sorting and the icon
// captions (see details.rs): 0 where not known.
pub const COL_ATIME: u32 = 14;
pub const COL_BTIME: u32 = 15;
pub const COL_OWNER: u32 = 16;
pub const COL_GROUP: u32 = 17;
pub const COL_MODE: u32 = 18;
pub const COL_ALLOC: u32 = 19;
pub const COL_TRASHED: u32 = 20;

const ATTRS: &str = "standard::name,standard::display-name,standard::type,standard::icon,\
standard::size,standard::content-type,standard::is-hidden,standard::is-backup,\
standard::is-symlink,standard::symlink-target,time::modified,access::can-read,\
thumbnail::path,thumbnail::is-valid,trash::orig-path,trash::deletion-date,\
time::access,time::created,owner::user,owner::group,unix::mode,standard::allocated-size";

/// Typed getter for tree model cells, and the rows of a list.
pub trait ModelGet {
    fn gv<T: for<'b> glib::value::FromValue<'b> + 'static>(&self, it: &gtk::TreeIter, col: i32) -> T;

    /// The (top-level) rows in order, each its own iter, for going through
    /// them. Not with iter_next: it moves an iter through a shared
    /// reference, which the compiler may not expect (a loop, or an
    /// Option::filter, can go on with the iter as it was).
    fn rows(&self) -> Vec<gtk::TreeIter>;
}

impl<O: IsA<gtk::TreeModel>> ModelGet for O {
    fn gv<T: for<'b> glib::value::FromValue<'b> + 'static>(&self, it: &gtk::TreeIter, col: i32) -> T {
        self.as_ref()
            .value(it, col)
            .get::<T>()
            .expect("tree model column type")
    }

    fn rows(&self) -> Vec<gtk::TreeIter> {
        let mut out = vec![];
        self.as_ref().foreach(|_, path, it| {
            if path.depth() == 1 {
                out.push(*it);
            }
            false
        });
        out
    }
}

/// A place in a pane's history: a folder, or a search started there (as in
/// Caja, the results are a place of their own: Back returns to the folder).
#[derive(Clone)]
pub struct Place {
    pub folder: gio::File,
    pub search: Option<crate::search::Query>,
}

impl Place {
    /// Its name in the Back button's menu.
    pub fn title(&self) -> String {
        match &self.search {
            Some(q) => q.title(),
            None => util::display_name(&self.folder),
        }
    }
}

/// Compares two rows' text in column `col` where it is, without copying it.
/// What fills in a cell for a row (a cell data function).
type CellFill = Rc<dyn Fn(&gtk::CellRenderer, &gtk::TreeModel, &gtk::TreeIter)>;

fn cmp_text(m: &gtk::TreeModel, a: &gtk::TreeIter, b: &gtk::TreeIter, col: u32) -> Ordering {
    let (va, vb) = (m.value(a, col as i32), m.value(b, col as i32));
    value_str(&va).cmp(value_str(&vb))
}

/// A string column's text, without copying it or checking it again for
/// UTF-8 (as Value::get does): it's read for every item at every layout and
/// sort, and the models only ever hold Rust strings. Empty if unset.
pub(crate) fn value_str(v: &glib::Value) -> &str {
    debug_assert!(v.is::<String>());
    // SAFETY: a string Value (asserted above); its text lives as long as
    // `v`, and was valid UTF-8 when it was stored.
    unsafe {
        let p = glib::gobject_ffi::g_value_get_string(glib::translate::ToGlibPtr::to_glib_none(v).0);
        if p.is_null() { "" } else { std::str::from_utf8_unchecked(std::ffi::CStr::from_ptr(p).to_bytes()) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Icon,
    List,
    Compact,
    /// macOS Finder's column view (see columns.rs); the open folder's
    /// column is the list view, so most code treats it as List.
    Columns,
}

impl ViewMode {
    pub fn from_str(s: &str) -> ViewMode {
        match s {
            "list" => ViewMode::List,
            "compact" => ViewMode::Compact,
            "columns" => ViewMode::Columns,
            _ => ViewMode::Icon,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ViewMode::Icon => "icon",
            ViewMode::List => "list",
            ViewMode::Compact => "compact",
            ViewMode::Columns => "columns",
        }
    }
}

pub enum Event {
    Location,
    Selection,
    Activate,
    Context { event: Option<gdk::Event>, on_item: bool },
    OpenNewTab(gio::File),
    /// A middle-click on a file.
    OpenFile(RowInfo),
    Back,
    Forward,
    Drop { uris: Vec<String>, dest: gio::File, action: gdk::DragAction },
    Rename { uri: String, new_name: String },
    Loading(bool),
    /// The user picked a sort order in the list view's column headers.
    Sort { by: String, desc: bool },
    /// The list view's columns were chosen or reordered here (and the
    /// setting changed): the other panes show them alike.
    ColumnsChanged,
}

/// Where a pane's events go (the window).
type Handler = Rc<dyn Fn(Event)>;

/// A rubber band's start (bin window coordinates) and the items selected
/// before it (see Pane::trim_rubber_band).
type Band = ((i32, i32), Vec<gtk::TreePath>);

#[derive(Clone, Debug)]
pub struct RowInfo {
    pub uri: String,
    pub name: String,
    pub is_dir: bool,
    pub ctype: String,
    pub size: u64,
    pub orig: String,
}

/// Whether a list view column header is held down (maybe being dragged).
static HEADER_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// GTK 3 warns "Negative content width … (node button, owner GtkButton)"
/// whenever a column header is dragged to another place: it gives the
/// header a width of 1 pixel while it moves it (any GtkTreeView does it).
/// That warning is left out while a header is held; all else is told.
pub fn quiet_header_drags() {
    // GTK logs through GLib's structured logging, so at the writer, which
    // every message goes through (set before anything is logged).
    glib::log_set_writer_func(|level, fields| {
        if level == glib::LogLevel::Warning && HEADER_HELD.load(std::sync::atomic::Ordering::Relaxed) {
            let field = |key: &str| fields.iter().find(|f| f.key() == key).and_then(|f| f.value_str());
            let message = field("MESSAGE").unwrap_or_default();
            if field("GLIB_DOMAIN") == Some("Gtk") && message.starts_with("Negative content width") && message.contains("owner GtkButton") {
                return glib::LogWriterOutput::Handled;
            }
        }
        glib::log_writer_default(level, fields)
    });
}

/// Shades every second row of `tv` (also used by the bulk rename list).
pub fn draw_stripes(tv: &gtk::TreeView, cr: &gtk::cairo::Context) {
    let Some((first, last)) = tv.visible_range() else { return };
    let (Some(&first), Some(&last)) = (first.indices().first(), last.indices().first()) else { return };
    // Rows scrolled under the column headers mustn't be painted over them.
    let (_, top) = tv.convert_bin_window_to_widget_coords(0, 0);
    let (width, height) = (tv.allocated_width() as f64, tv.allocated_height() as f64);
    let c = tv.style_context().color(gtk::StateFlags::NORMAL);
    let sel = tv.selection();
    cr.save().ok();
    cr.rectangle(0.0, top as f64, width, height - top as f64);
    cr.clip();
    cr.set_source_rgba(c.red(), c.green(), c.blue(), 0.06);
    for i in (first..=last).filter(|i| i % 2 == 1) {
        let path = gtk::TreePath::from_indicesv(&[i]);
        if sel.path_is_selected(&path) {
            continue;
        }
        let area = tv.background_area(Some(&path), None::<&gtk::TreeViewColumn>);
        let (_, y) = tv.convert_bin_window_to_widget_coords(0, area.y());
        cr.rectangle(0.0, y as f64, width, area.height() as f64);
    }
    let _ = cr.fill();
    cr.restore().ok();
}

thread_local! {
    static TYPE_DESC: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Folder URI the current in-app drag started from.
    static DRAG_SRC_DIR: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Whether everything in the current in-app drag is a folder (None:
    /// not known).
    static DRAG_ALL_DIRS: Cell<Option<bool>> = const { Cell::new(None) };
    /// The file systems of the folders the current drag passed over, by
    /// URI: asked at every move of the pointer, and each is a round trip on
    /// a network share.
    static DRAG_FS: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
}

pub fn type_description(ctype: &str, is_dir: bool) -> String {
    if is_dir {
        return "Folder".into();
    }
    TYPE_DESC.with(|t| {
        if let Some(d) = t.borrow().get(ctype) {
            return d.clone();
        }
        let d = gio::content_type_get_description(ctype).to_string();
        t.borrow_mut().insert(ctype.to_string(), d.clone());
        d
    })
}

pub fn collate_key(name: &str) -> String {
    unsafe {
        let p = glib::ffi::g_utf8_collate_key_for_filename(name.to_glib_none().0, -1);
        glib::GString::from_glib_full(p).to_string()
    }
}

fn dim(p: &Pixbuf) -> Pixbuf {
    faded(p, 110.0 / 255.0)
}

/// The rename editor of the icon views (Pane::open_rename_editor).
struct RenameEditor {
    frame: gtk::Frame,
    view: gtk::TextView,
    uri: String,
    name: String,
}

/// The rename editor's look: the theme's text field colours in a thin frame.
fn rename_css() -> gtk::CssProvider {
    thread_local! {
        static CSS: gtk::CssProvider = {
            let css = gtk::CssProvider::new();
            let _ = css.load_from_data(b"frame.arkha-rename, textview, textview text { background-color: @theme_base_color; color: @theme_text_color; }");
            css
        };
    }
    CSS.with(|c| c.clone())
}

/// A solid background for the inline rename field (some themes leave it
/// transparent over the file view).
fn edit_field_css() -> gtk::CssProvider {
    thread_local! {
        static CSS: gtk::CssProvider = {
            let css = gtk::CssProvider::new();
            let _ = css.load_from_data(b"entry { background-color: @theme_base_color; color: @theme_text_color; }");
            css
        };
    }
    CSS.with(|c| c.clone())
}

pub fn targets() -> Vec<gtk::TargetEntry> {
    vec![gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0)]
}

/// What can be dropped on folders: files, and files dragged out of an
/// archive manager (see xds), in that order of preference.
pub fn drop_targets() -> Vec<gtk::TargetEntry> {
    targets().into_iter().chain(crate::xds::target_entry()).collect()
}

pub fn dnd_actions() -> gdk::DragAction {
    gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK | gdk::DragAction::ASK
}

fn enable_drag_source_iv(iv: &gtk::IconView) {
    let t = targets();
    let stashes: Vec<_> = t.iter().map(|e| e.to_glib_none()).collect();
    let raw: Vec<_> = stashes.iter().map(|s| unsafe { *s.0 }).collect();
    unsafe {
        gtk::ffi::gtk_icon_view_enable_model_drag_source(
            iv.to_glib_none().0,
            gdk::ModifierType::BUTTON1_MASK.into_glib(),
            raw.as_ptr(),
            raw.len() as i32,
            dnd_actions().into_glib(),
        );
    }
}

/// The folder a drag from this program started in (for choosing between
/// copying and moving), or None.
pub fn set_drag_source_dir(uri: Option<String>) {
    DRAG_SRC_DIR.with(|d| *d.borrow_mut() = uri);
    DRAG_FS.with(|c| c.borrow_mut().clear());
    DRAG_ALL_DIRS.with(|d| d.set(None));
}

/// Whether the current in-app drag holds only folders (None: not known,
/// e.g. a drag from another program).
pub fn drag_is_folders() -> Option<bool> {
    DRAG_ALL_DIRS.with(|d| d.get())
}

/// The keyboard modifiers held now, as a drop sees them.
pub fn pointer_modifiers(widget: &gtk::Widget) -> gdk::ModifierType {
    if let (Some(win), Some(seat)) = (widget.window(), widget.display().default_seat()) {
        if let Some(ptr) = seat.pointer() {
            return win.device_position(&ptr).3;
        }
    }
    gdk::ModifierType::empty()
}

pub fn enable_drag_source_tv(tv: &gtk::TreeView) {
    let t = targets();
    let stashes: Vec<_> = t.iter().map(|e| e.to_glib_none()).collect();
    let raw: Vec<_> = stashes.iter().map(|s| unsafe { *s.0 }).collect();
    unsafe {
        gtk::ffi::gtk_tree_view_enable_model_drag_source(
            tv.to_glib_none().0,
            gdk::ModifierType::BUTTON1_MASK.into_glib(),
            raw.as_ptr(),
            raw.len() as i32,
            dnd_actions().into_glib(),
        );
    }
}

/// Decide copy / move / link from the keyboard modifiers and source/dest filesystem.
pub fn choose_action(widget: &gtk::Widget, ctx: &gdk::DragContext, dest: &gio::File) -> gdk::DragAction {
    let mods = pointer_modifiers(widget);
    let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
    let alt = mods.contains(gdk::ModifierType::MOD1_MASK);
    let wanted = if ctrl && shift {
        gdk::DragAction::LINK
    } else if ctrl {
        gdk::DragAction::COPY
    } else if shift {
        gdk::DragAction::MOVE
    } else if alt {
        gdk::DragAction::ASK
    } else if dest.uri().starts_with("trash:") {
        gdk::DragAction::MOVE
    } else if let Some(src) = DRAG_SRC_DIR.with(|d| d.borrow().clone()) {
        let fs = |f: &gio::File| {
            let uri = f.uri().to_string();
            if let Some(id) = DRAG_FS.with(|c| c.borrow().get(&uri).cloned()) {
                return id;
            }
            let id = f
                .query_info("id::filesystem", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
                .ok()
                .and_then(|i| i.attribute_string("id::filesystem"))
                .map(|s| s.to_string());
            DRAG_FS.with(|c| c.borrow_mut().insert(uri, id.clone()));
            id
        };
        let a = fs(&gio::File::for_uri(&src));
        if a.is_some() && a == fs(dest) {
            gdk::DragAction::MOVE
        } else {
            gdk::DragAction::COPY
        }
    } else {
        ctx.suggested_action()
    };
    if ctx.actions().contains(wanted) {
        wanted
    } else {
        ctx.suggested_action()
    }
}

#[allow(dead_code)]
pub struct Pane {
    pub root: gtk::Box,
    pub tab_label: gtk::Label,
    pub tab_icon: gtk::Image,
    store: gtk::ListStore,
    filter: gtk::TreeModelFilter,
    stack: gtk::Stack,
    icon_view: gtk::IconView,
    icon_pix: gtk::CellRendererPixbuf,
    icon_text: gtk::CellRendererText,
    /// The same two cells as their own types (see iconlabel.rs).
    image: crate::iconlabel::IconImage,
    label: crate::iconlabel::IconLabel,
    /// Caption lines under names in the icon view: 0, 1 (size) or 2 (and
    /// the date), more the larger the icons, as in Caja.
    /// The details shown under names in the icon view at this zoom level.
    captions: Rc<RefCell<Vec<&'static str>>>,
    /// The longest side of thumbnails in the icon view.
    thumb_size: Rc<Cell<i32>>,
    /// The icon view's width the columns were last fitted to, and how many
    /// there are then.
    fitted_width: Cell<i32>,
    icon_columns: Rc<Cell<i32>>,
    /// The icon view's cursor was moved with the keyboard: Caja then shows
    /// the focus around its name (not after a click).
    keyboard_focus: Cell<bool>,
    /// Names are shown in the icon view (not at the smallest size).
    labels_shown: Rc<Cell<bool>>,
    /// The icon view's cells are hidden while GTK looks for the item
    /// clicked (see ignore_items_once).
    hit_off: Rc<Cell<bool>>,
    /// The icon view's item a drag would drop into.
    drop_path: Rc<RefCell<Option<gtk::TreePath>>>,
    /// Where Shift+click selections in the icon views start.
    range_anchor: Cell<Option<i32>>,
    /// The selection a press on one of several selected items kept for a
    /// drag, and that item: just it is selected if no drag follows. Then
    /// the same, for a double click to open them all.
    press_keep: RefCell<Option<(Vec<gtk::TreePath>, gtk::TreePath)>>,
    released_keep: RefCell<Option<(Vec<gtk::TreePath>, gtk::TreePath)>>,
    /// A rubber band in the icon view: where it started (bin window
    /// coordinates) and what was selected before (see trim_rubber_band).
    band: RefCell<Option<Band>>,
    /// The item being renamed in the icon view, whose name isn't drawn.
    edit_hidden: Rc<RefCell<Option<String>>>,
    tree_view: gtk::TreeView,
    list_pix: gtk::CellRendererPixbuf,
    list_text: gtk::CellRendererText,
    name_col: gtk::TreeViewColumn,
    /// A column for each detail (details::DETAILS), shown as chosen (see
    /// apply_column_choice).
    detail_cols: Vec<(&'static str, gtk::TreeViewColumn)>,
    /// Set while apply_column_choice moves columns, so that isn't taken
    /// for the user dragging one.
    placing_columns: Cell<bool>,
    sort_cols: Vec<(u32, gtk::TreeViewColumn)>,
    /// The list view's columns with what they show ("name" or a detail)
    /// and whether they show an icon too (for fitting them to their contents).
    fit_cols: Vec<(gtk::TreeViewColumn, &'static str, bool)>,
    info_bar: gtk::InfoBar,
    /// Empty Trash and Restore above the trash folder, as in Caja.
    trash_bar: gtk::InfoBar,
    info_label: gtk::Label,

    current: RefCell<gio::File>,
    back: RefCell<Vec<Place>>,
    fwd: RefCell<Vec<Place>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    gen: Cell<u64>,
    /// Rows of deleted files waiting for remove_uri's batch.
    pending_removals: RefCell<Vec<String>>,
    /// Files changed in the open folder, to read again (see refresh_file),
    /// and their URIs (to take each once).
    pending_refresh: RefCell<Vec<gio::File>>,
    pending_refresh_uris: RefCell<std::collections::HashSet<String>>,
    /// When the last batch of removals and of changes was done (see batch_delay).
    last_removals: Cell<Option<Instant>>,
    last_refresh: Cell<Option<Instant>>,
    index: RefCell<HashMap<String, gtk::TreeIter>>,
    mode: Cell<ViewMode>,
    icon_size: Rc<Cell<i32>>,
    list_size: Rc<Cell<i32>>,
    show_hidden: Rc<Cell<bool>>,
    folders_first: Rc<Cell<bool>>,
    /// The search whose results are shown (see start_search), started in
    /// the folder in `current`.
    search: RefCell<Option<crate::search::Query>>,
    /// Caja's query editor, above the results.
    query_editor: Rc<crate::search::QueryEditor>,
    /// The search running in the background: it stops when this changes.
    search_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pending_select: RefCell<Vec<String>>,
    pending_rename: RefCell<Option<String>>,
    editing: Rc<RefCell<Option<String>>>,
    free_space: RefCell<Option<u64>>,
    loading: Cell<bool>,
    typeahead: RefCell<String>,
    typeahead_gen: Cell<u64>,
    /// Wheel scrolling not yet a whole step (see typeahead_scroll).
    typeahead_wheel: Cell<f64>,
    /// Shows what's been typed to select an item (bottom right, like Caja).
    typeahead_entry: gtk::Entry,
    /// Over the file view: the type-to-select field and the rename editor.
    overlay: gtk::Overlay,
    /// Renaming in the icon and compact views (see open_rename_editor).
    rename_editor: RefCell<Option<Rc<RenameEditor>>>,
    /// Times the rename editor's place was asked for before its item was
    /// laid out (see the overlay's get-child-position handler).
    rename_tries: Cell<u32>,
    /// The info panel beside the view (info.rs), and whether this folder
    /// shows it.
    info: Rc<crate::info::InfoPanel>,
    info_paned: gtk::Paned,
    info_on: Cell<bool>,
    handler: RefCell<Option<Handler>>,
    // TreeView multi-selection drag support
    tv_block: Rc<Cell<bool>>,
    tv_pending: RefCell<Option<gtk::TreePath>>,
    thumb_listener: Cell<Option<usize>>,
    // The columns view (columns.rs).
    columns_mode: Rc<Cell<bool>>,
    sw_list: gtk::ScrolledWindow,
    strip: gtk::Box,
    strip_sw: gtk::ScrolledWindow,
    strip_events: gtk::EventBox,
    anc_box: gtk::Box,
    preview_box: gtk::Box,
    preview_slot: gtk::Box,
    preview_info: Rc<crate::info::InfoPanel>,
    /// The columns of the folders above the open one, from the first.
    ancestors: RefCell<Vec<Rc<crate::columns::Column>>>,
    /// The selected folder's column, right of the open one.
    preview_col: RefCell<Option<Rc<crate::columns::Column>>>,
    /// The folder the columns start at (Finder: where you went to from
    /// the side pane or the location bar).
    column_root: RefCell<Option<gio::File>>,
    /// The next load comes from moving through the columns (or Back /
    /// Forward), so they keep their start.
    keep_columns: Cell<bool>,
    /// Select the first item once the folder is loaded (→ into a folder).
    select_first: Cell<bool>,
    /// A context menu asked for in a side column, opened once its folder
    /// is loaded: (the click, on an item).
    pending_context: RefCell<Option<(gdk::Event, bool)>>,
    /// Keep the strip of columns scrolled to its end while it's laid out.
    columns_to_end: Rc<Cell<bool>>,
    /// Scroll the strip to its end once it's shown (see scroll_columns_to_end).
    columns_end_on_map: Cell<bool>,
    /// Shut down (see shutdown): its tab is closing, nothing more is done.
    closed: Cell<bool>,
    me: RefCell<std::rc::Weak<Pane>>,
    /// An icon view scroll waiting for its item to be laid out (see
    /// scroll_icon_view): the item's URI, center, row alignment, whether
    /// it gets the cursor too; tries.
    pending_scroll: RefCell<Option<(String, bool, f32, bool)>>,
    scroll_tries: Cell<u32>,
    /// A selection change waits to be passed on (see emit).
    selection_pending: Cell<bool>,
    /// The open folder's display name, by its URI: asking for it is a
    /// round trip to where the folder is.
    folder_name: RefCell<Option<(String, String)>>,
}

impl Pane {
    pub fn new(start: &gio::File) -> Rc<Pane> {
        let s = config::get();
        let show_hidden = Rc::new(Cell::new(s.show_hidden));
        let folders_first = Rc::new(Cell::new(s.folders_first));
        let icon_size = Rc::new(Cell::new(s.icon_size));
        let list_size = Rc::new(Cell::new(s.list_size));

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

        let filter = new_filter(&store);
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
        let icon_view_ref: Rc<RefCell<Option<glib::WeakRef<gtk::IconView>>>> = Rc::default();
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

        // ---- icon view
        let icon_view = crate::iconview::IconView::with_model(&filter);
        *icon_view_ref.borrow_mut() = Some(icon_view.downgrade());
        icon_view.set_selection_mode(gtk::SelectionMode::Multiple);
        IconViewExt::set_margin(&icon_view, 8);
        icon_view.set_row_spacing(4);
        icon_view.set_column_spacing(4);
        icon_view.set_item_padding(4);
        icon_view.set_spacing(2);
        // Caja's look (see iconlabel.rs): no highlight over the whole item,
        // the icon tinted and the name boxed instead.
        {
            let css = gtk::CssProvider::new();
            let _ = css.load_from_data(b"iconview.caja-like:selected, iconview.caja-like:selected:focus { background-color: transparent; background-image: none; } iconview.caja-like { outline-width: 0; }");
            icon_view.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        let image = crate::iconlabel::IconImage::new();
        let icon_pix: gtk::CellRendererPixbuf = image.clone().upcast();
        CellRendererExt::set_alignment(&icon_pix, 0.5, 1.0);
        let label = crate::iconlabel::IconLabel::new();
        let icon_text: gtk::CellRendererText = label.clone().upcast();
        let captions: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let thumb_size = Rc::new(Cell::new(64));
        let icon_columns = Rc::new(Cell::new(1));
        let labels_shown = Rc::new(Cell::new(true));
        let hit_off = Rc::new(Cell::new(false));
        let drop_path: Rc<RefCell<Option<gtk::TreePath>>> = Rc::default();
        icon_view.pack_start(&icon_pix, false);
        CellLayoutExt::pack_start(&icon_view, &icon_text, false);
        // The item being renamed doesn't draw its name: the edit field over
        // it would show it through (see inline rename in setup_signals).
        let edit_hidden: Rc<RefCell<Option<String>>> = Rc::default();
        let (text_fill, pix_fill): (CellFill, CellFill);
        {
            let hidden = edit_hidden.clone();
            let captions = captions.clone();
            let columns = icon_columns.clone();
            let iv = icon_view.clone();
            let (shown, hit_off, drop_path, label) = (labels_shown.clone(), hit_off.clone(), drop_path.clone(), label.clone());
            let fill: CellFill = Rc::new(move |cell, m, it| {
                    cell.set_visible(shown.get() && !hit_off.get());
                    label.set_drop_target(drop_path.borrow().as_ref().is_some_and(|d| m.path(it).as_ref() == Some(d)));
                    if hidden.borrow().as_deref().is_some_and(|u| m.gv::<String>(it, COL_URI as i32) == u) {
                        label.set_label_text("");
                        return;
                    }
                    if iv.item_orientation() == gtk::Orientation::Horizontal {
                        label.set_label_text(value_str(&m.value(it, COL_NAME as i32)));
                        return;
                    }
                    label.set_label_text(&icon_cell_text(m, it, &captions.borrow(), columns.get()));
            });
            text_fill = fill.clone();
            CellLayoutExt::set_cell_data_func(&icon_view, &icon_text, Some(Box::new(move |_, cell, m, it| fill(cell, m, it))));
        }
        {
            let sz = icon_size.clone();
            let lsz = list_size.clone();
            let tsz = thumb_size.clone();
            let iv = icon_view.clone();
            let (hit_off, drop_path, image) = (hit_off.clone(), drop_path.clone(), image.clone());
            let fill: CellFill = Rc::new(move |cell, m, it| {
                    cell.set_visible(!hit_off.get());
                    image.set_drop_target(drop_path.borrow().as_ref().is_some_and(|d| m.path(it).as_ref() == Some(d)));
                    // GtkIconView measures every item, not just the ones in
                    // sight: that needs only the room (a fixed size), the
                    // picture is made when drawn.
                    let drawn = icons::drawing();
                    if iv.item_orientation() == gtk::Orientation::Horizontal {
                        image.set_picture(if drawn { row_pixbuf(m, it, lsz.get().min(48)) } else { None });
                        return;
                    }
                    // The thumbnail looked up once, for the room and the picture.
                    let thumb = icon_view_thumb(m, it);
                    let (w, h) = icon_view_dims(thumb.as_ref(), sz.get(), tsz.get());
                    cell.set_fixed_size(w, h);
                    image.set_picture(if drawn { icon_view_pixbuf(m, it, thumb, sz.get(), tsz.get()) } else { None });
            });
            pix_fill = fill.clone();
            CellLayoutExt::set_cell_data_func(&icon_view, &icon_pix, Some(Box::new(move |_, cell, m, it| fill(cell, m, it))));
        }
        // Items drawn by the icon view itself get both cells filled in
        // directly, the picture first as GtkCellArea does (see iconview.rs).
        {
            let (p, t) = (icon_pix.clone().upcast::<gtk::CellRenderer>(), icon_text.clone().upcast::<gtk::CellRenderer>());
            crate::iconview::IconView::set_filler(&icon_view, Rc::new(move |m, it| {
                pix_fill(&p, m, it);
                text_fill(&t, m, it);
            }));
        }

        mark_drawing(icon_view.upcast_ref());

        // ---- list view
        let tree_view = gtk::TreeView::with_model(&filter);
        mark_drawing(tree_view.upcast_ref());
        tree_view.set_headers_clickable(true);
        tree_view.set_enable_search(false);
        tree_view.set_rubber_banding(true);
        tree_view.selection().set_mode(gtk::SelectionMode::Multiple);
        // GTK 3 themes can't style even/odd rows, so shade every second row
        // after the view is drawn, with a faint wash of the text color.
        tree_view.connect_local("draw", true, |vals| {
            let tv = vals[0].get::<gtk::TreeView>().ok()?;
            let cr = vals[1].get::<gtk::cairo::Context>().ok()?;
            if config::with(|s| s.striped_rows) {
                draw_stripes(&tv, &cr);
            }
            Some(false.to_value())
        });
        let name_col = gtk::TreeViewColumn::new();
        name_col.set_title("Name");
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
            c.set_title(d.title);
            c.set_resizable(true);
            // At least as wide as its title, with room for the sort arrow.
            let width = d.min_width.max(tree_view.create_pango_layout(Some(d.title)).pixel_size().0 + 32);
            c.set_min_width(width);
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
            c.set_fixed_width(width);
            tree_view.append_column(&c);
            detail_cols.push((d.id, c));
        }
        let mut fit_cols = vec![(name_col.clone(), "name", true)];
        fit_cols.extend(detail_cols.iter().map(|(id, c)| (c.clone(), *id, false)));
        let mut sort_cols = vec![(0, name_col.clone())];
        sort_cols.extend(detail_cols.iter().map(|(id, c)| (crate::details::sort_id(id), c.clone())));
        for (_, c) in &sort_cols {
            c.set_clickable(true);
        }

        let sw_icons = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        // Scrollbars beside the items, as in Caja, not over them.
        sw_icons.set_overlay_scrolling(false);
        sw_icons.add(&icon_view);
        let sw_list = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        sw_list.add(&tree_view);
        // The list view shares its place with the columns view (columns.rs):
        // there, columns of the folders above go to its left, the selected
        // folder's contents or file's preview to its right, and the whole
        // strip scrolls sideways.
        let anc_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let preview_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let preview_info = crate::info::InfoPanel::new();
        preview_info.widget.set_size_request(crate::columns::WIDTH + 40, -1);
        let preview_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        preview_box.pack_start(&preview_slot, false, false, 0);
        preview_box.pack_start(&preview_info.widget, false, false, 0);
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        strip.pack_start(&anc_box, false, false, 0);
        strip.pack_start(&sw_list, true, true, 0);
        strip.pack_start(&preview_box, false, false, 0);
        let strip_sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        strip_sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        // Clicks on the blank space beside the columns (see setup_signals).
        let strip_events = gtk::EventBox::new();
        strip_events.set_visible_window(false);
        strip_events.add(&strip);
        strip_sw.add(&strip_events);
        // Columns appear and change width while loading: until that settles
        // (see scroll_columns_to_end), the strip follows its right end.
        let columns_to_end = Rc::new(Cell::new(false));
        {
            let to_end = columns_to_end.clone();
            let sw = strip_sw.downgrade();
            strip_sw.hadjustment().connect_changed(move |a| {
                if to_end.get() && sw.upgrade().is_some_and(|sw| sw.is_realized()) {
                    a.set_value(a.upper() - a.page_size());
                }
            });
        }
        anc_box.set_no_show_all(true);
        preview_box.show_all();
        preview_info.widget.hide();
        preview_box.set_no_show_all(true);
        let stack = gtk::Stack::new();
        stack.add_named(&sw_icons, "icons");
        stack.add_named(&strip_sw, "list");
        stack.set_vexpand(true);

        let info_bar = gtk::InfoBar::new();
        info_bar.set_message_type(gtk::MessageType::Error);
        let info_label = gtk::Label::new(None);
        info_label.set_line_wrap(true);
        info_label.set_xalign(0.0);
        info_bar.content_area().add(&info_label);
        info_bar.set_no_show_all(true);
        info_label.show();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // What's typed to select an item shows over the view's bottom right
        // corner, as in Caja. It never takes the focus: keys go to the view.
        let typeahead_entry = gtk::Entry::new();
        typeahead_entry.set_can_focus(false);
        typeahead_entry.set_width_chars(16);
        typeahead_entry.set_halign(gtk::Align::End);
        typeahead_entry.set_valign(gtk::Align::End);
        typeahead_entry.set_margin_end(16);
        typeahead_entry.set_margin_bottom(12);
        typeahead_entry.set_no_show_all(true);
        let overlay = gtk::Overlay::new();
        overlay.add(&stack);
        overlay.add_overlay(&typeahead_entry);
        // The info panel (info.rs) to the right of the view.
        let info = crate::info::InfoPanel::new();
        let info_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        info_paned.pack1(&overlay, true, false);
        info_paned.pack2(&info.widget, false, false);
        // Caja's query editor, above the search results (see start_search).
        let query_editor = crate::search::QueryEditor::new();
        root.pack_start(&query_editor.widget, false, false, 0);
        root.pack_start(&info_bar, false, false, 0);
        // The window's actions do the work (and say when they can).
        // No mnemonics: Alt+E is the Edit menu.
        let trash_bar = gtk::InfoBar::new();
        trash_bar.set_message_type(gtk::MessageType::Other);
        {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let title = gtk::Label::new(Some("Trash"));
            title.set_xalign(0.0);
            title.set_hexpand(true);
            title.style_context().add_class("dim-label");
            let restore = gtk::Button::with_label("Restore Selected Items");
            restore.set_action_name(Some("win.restore"));
            restore.set_tooltip_text(Some("Move the selected items back to where they were deleted from"));
            let empty = gtk::Button::with_label("Empty Trash");
            empty.set_action_name(Some("win.empty-trash"));
            empty.set_tooltip_text(Some("Delete all items in the Trash"));
            row.pack_start(&title, true, true, 0);
            row.pack_start(&restore, false, false, 0);
            row.pack_start(&empty, false, false, 0);
            trash_bar.content_area().add(&row);
        }
        trash_bar.set_no_show_all(true);
        trash_bar.content_area().show_all();
        root.pack_start(&trash_bar, false, false, 0);
        root.pack_start(&info_paned, true, true, 0);
        root.show_all();
        info_bar.hide();

        let tab_label = gtk::Label::new(None);
        tab_label.set_ellipsize(pango::EllipsizeMode::End);
        // Same natural and minimum width for every tab, so the notebook splits
        // the extra space evenly instead of favouring long names.
        tab_label.set_width_chars(12);
        tab_label.set_max_width_chars(12);
        let tab_icon = gtk::Image::new();

        let pane = Rc::new(Pane {
            root,
            tab_label,
            tab_icon,
            store,
            filter,
            stack,
            icon_view,
            icon_pix,
            icon_text,
            image,
            label,
            captions,
            thumb_size,
            fitted_width: Cell::new(0),
            icon_columns,
            keyboard_focus: Cell::new(false),
            labels_shown,
            hit_off,
            drop_path,
            range_anchor: Cell::new(None),
            press_keep: RefCell::new(None),
            released_keep: RefCell::new(None),
            band: RefCell::new(None),
            edit_hidden,
            tree_view,
            list_pix,
            list_text,
            name_col,
            detail_cols,
            placing_columns: Cell::new(false),
            sort_cols,
            fit_cols,
            info_bar,
            trash_bar,
            info_label,
            current: RefCell::new(start.clone()),
            back: RefCell::new(vec![]),
            fwd: RefCell::new(vec![]),
            monitor: RefCell::new(None),
            gen: Cell::new(0),
            pending_removals: RefCell::new(vec![]),
            pending_refresh: RefCell::new(vec![]),
            pending_refresh_uris: RefCell::new(Default::default()),
            last_removals: Cell::new(None),
            last_refresh: Cell::new(None),
            index: RefCell::new(HashMap::new()),
            mode: Cell::new(ViewMode::from_str(&s.default_view)),
            icon_size,
            list_size,
            show_hidden,
            folders_first,
            search: RefCell::new(None),
            query_editor,
            search_gen: Default::default(),
            pending_select: RefCell::new(vec![]),
            pending_rename: RefCell::new(None),
            pending_scroll: RefCell::new(None),
            scroll_tries: Cell::new(0),
            editing: Rc::new(RefCell::new(None)),
            free_space: RefCell::new(None),
            loading: Cell::new(false),
            typeahead: RefCell::new(String::new()),
            typeahead_gen: Cell::new(0),
            typeahead_wheel: Cell::new(0.0),
            typeahead_entry,
            overlay: overlay.clone(),
            rename_editor: RefCell::new(None),
            rename_tries: Cell::new(0),
            info,
            info_paned,
            info_on: Cell::new(false),
            handler: RefCell::new(None),
            tv_block: Rc::new(Cell::new(false)),
            tv_pending: RefCell::new(None),
            thumb_listener: Cell::new(None),
            columns_mode,
            sw_list,
            strip,
            strip_sw,
            strip_events,
            anc_box,
            preview_box,
            preview_slot,
            preview_info,
            ancestors: RefCell::new(vec![]),
            preview_col: RefCell::new(None),
            column_root: RefCell::new(None),
            keep_columns: Cell::new(false),
            select_first: Cell::new(false),
            pending_context: RefCell::new(None),
            columns_to_end,
            columns_end_on_map: Cell::new(false),
            closed: Cell::new(false),
            me: RefCell::new(std::rc::Weak::new()),
            selection_pending: Cell::new(false),
            folder_name: RefCell::new(None),
        });
        *pane.me.borrow_mut() = Rc::downgrade(&pane);
        pane.setup_signals();
        pane.apply_sort(&s.sort_by, s.sort_desc);
        pane.set_mode(pane.mode.get());
        pane.apply_click_policy();
        {
            // Applied every half second: each change makes the icon view
            // measure all its items again.
            let w = Rc::downgrade(&pane);
            let pending: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
            let id = icons::on_thumbnail_ready(move |uri, path| {
                let first = pending.borrow().is_empty();
                pending.borrow_mut().push((uri.to_string(), path.to_string()));
                if first {
                    let pending = pending.clone();
                    let w = w.clone();
                    glib::timeout_add_local_once(Duration::from_millis(500), move || {
                        let batch = pending.take();
                        if let Some(p) = w.upgrade() {
                            let set = || {
                                for (uri, path) in &batch {
                                    p.set_thumb(uri, path);
                                }
                            };
                            // In the icon view a thumbnail's shape sets the
                            // item's size: it has to be laid out again.
                            if p.icon_view.item_orientation() == gtk::Orientation::Vertical && p.mode.get() == ViewMode::Icon {
                                set();
                            } else {
                                p.quietly(set);
                            }
                        }
                    });
                }
            });
            pane.thumb_listener.set(Some(id));
        }
        pane
    }

    pub fn set_handler(&self, f: impl Fn(Event) + 'static) {
        *self.handler.borrow_mut() = Some(Rc::new(f));
    }

    fn emit(&self, e: Event) {
        if self.closed.get() {
            return;
        }
        if matches!(e, Event::Selection) {
            // Selections change item by item (selecting all, a rubber band
            // over thousands), and what follows looks at all that's
            // selected: once, when they're done.
            if !self.selection_pending.replace(true) {
                let w = self.me.borrow().clone();
                glib::idle_add_local_once(move || {
                    if let Some(p) = w.upgrade() {
                        if p.selection_pending.replace(false) && !p.closed.get() {
                            p.selection_changed();
                        }
                    }
                });
            }
            return;
        }
        // Anything else sees the selection as it is now (a context menu's
        // actions depend on it).
        if self.selection_pending.replace(false) {
            self.selection_changed();
        }
        let h = self.handler.borrow().clone();
        if let Some(h) = h {
            h(e);
        }
    }

    fn selection_changed(&self) {
        self.update_info();
        if self.columns_mode.get() {
            self.update_column_preview();
        }
        let h = self.handler.borrow().clone();
        if let Some(h) = h {
            h(Event::Selection);
        }
    }

    fn setup_signals(self: &Rc<Self>) {
        {
            let w = Rc::downgrade(self);
            self.strip_sw.connect_map(move |_| {
                if let Some(p) = w.upgrade() {
                    if p.columns_end_on_map.replace(false) {
                        p.scroll_columns_to_end();
                    }
                }
            });
        }
        let w = Rc::downgrade(self);
        // The query editor's rows are kept with the search; Reload (or Enter
        // in a row) searches again.
        {
            let w1 = w.clone();
            self.query_editor.connect_changed(move |criteria| {
                if let Some(p) = w1.upgrade() {
                    if let Some(q) = p.search.borrow_mut().as_mut() {
                        q.criteria = criteria;
                    }
                }
            });
            let w = w.clone();
            self.query_editor.connect_reload(move || {
                if let Some(p) = w.upgrade() {
                    p.reload();
                }
            });
        }
        // The rename editor sits over the item's name, also as the view
        // scrolls.
        {
            let w = w.clone();
            self.overlay.connect_get_child_position(move |_, child| {
                let p = w.upgrade()?;
                let ed = p.rename_editor.borrow().clone()?;
                if child != ed.frame.upcast_ref::<gtk::Widget>() {
                    return None;
                }
                if let Some(r) = p.rename_rect(&ed) {
                    p.rename_tries.set(0);
                    return Some(r);
                }
                // Its item isn't laid out yet (just added): out of sight
                // until it is (at its own size: smaller, GTK complains),
                // asked again shortly (for up to 2 s).
                if p.rename_tries.get() < 40 {
                    p.rename_tries.set(p.rename_tries.get() + 1);
                    let ov = p.overlay.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || ov.queue_resize());
                }
                let (w, h) = (ed.frame.preferred_width().1.max(1), ed.frame.preferred_height().1.max(1));
                Some(gdk::Rectangle::new(-w - 100, -h - 100, w, h))
            });
            if let Some(adj) = self.icon_view.vadjustment() {
                let ov = self.overlay.clone();
                adj.connect_value_changed(move |_| ov.queue_resize());
            }
        }
        {
            let w = w.clone();
            self.tree_view.connect_key_press_event(move |_, ev| match w.upgrade() {
                Some(p) => p.columns_key(ev),
                None => glib::Propagation::Proceed,
            });
        }
        // Double-clicking the divider at a column header's right edge fits
        // the column to its contents, as on Windows. (GTK's own double-click
        // there only goes back to the column's natural width, which is its
        // minimum while long text is cut off with "…".)
        {
            let w = w.clone();
            self.tree_view.connect_button_press_event(move |_, ev| {
                if ev.button() == 1 && ev.event_type() == gdk::EventType::DoubleButtonPress {
                    if let Some(p) = w.upgrade() {
                        if let Some(col) = p.column_divider_at(ev.root()) {
                            p.fit_column(&col);
                        }
                    }
                }
                glib::Propagation::Proceed
            });
        }
        // The columns view: a right-click on the blank space beside the
        // columns opens the menu of the open folder (to create something in
        // it, say), a left-click there clears the selection.
        {
            let w = w.clone();
            self.strip_events.connect_button_press_event(move |_, ev| {
                let Some(p) = w.upgrade() else { return glib::Propagation::Proceed };
                if !p.columns_mode.get() || ev.event_type() != gdk::EventType::ButtonPress {
                    return glib::Propagation::Proceed;
                }
                match ev.button() {
                    1 => p.select_filter_paths(&[]),
                    3 => {
                        p.select_filter_paths(&[]);
                        p.emit(Event::Context { event: Some(std::ops::Deref::deref(ev).clone()), on_item: false });
                    }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            });
        }
        // An empty open folder in the columns view says so, like the side
        // columns.
        {
            let w = w.clone();
            self.tree_view.connect_local("draw", true, move |vals| {
                let p = w.upgrade()?;
                if p.columns_mode.get() && !p.loading.get() && p.filter.iter_first().is_none() {
                    let v = vals[0].get::<gtk::TreeView>().ok()?;
                    let cr = vals[1].get::<gtk::cairo::Context>().ok()?;
                    crate::columns::draw_empty_hint(&v, &cr);
                }
                Some(false.to_value())
            });
        }
        // Dragging the info panel's divider: the width (shared by all
        // panes) is remembered, and the picture reloaded at the new size,
        // once the dragging stops.
        {
            let w = w.clone();
            let gen = Rc::new(Cell::new(0u64));
            self.info_paned.connect_position_notify(move |pn| {
                let Some(p) = w.upgrade() else { return };
                let total = pn.allocated_width();
                if !p.info.widget.is_visible() || total <= 1 {
                    return;
                }
                let width = total - pn.position();
                let g = gen.get() + 1;
                gen.set(g);
                let (w, gen) = (w.clone(), gen.clone());
                glib::timeout_add_local_once(Duration::from_millis(400), move || {
                    let Some(p) = w.upgrade() else { return };
                    if gen.get() != g || width == config::with(|s| s.info_panel_width) {
                        return;
                    }
                    config::update(|s| s.info_panel_width = width);
                    p.refresh_info();
                });
            });
        }
        // ---------------- icon view
        let iv = &self.icon_view;
        {
            let w = w.clone();
            iv.connect_local("draw", true, move |vals| {
                let cr = vals[1].get::<gtk::cairo::Context>().ok()?;
                if let Some(p) = w.upgrade() {
                    p.draw_whole_labels(&cr);
                }
                Some(false.to_value())
            });
        }
        {
            // Columns fitted to the width (after the allocation: changing
            // the item width in it would start another).
            let w = w.clone();
            iv.connect_size_allocate(move |_, _| {
                let w = w.clone();
                glib::idle_add_local_once(move || {
                    if let Some(p) = w.upgrade() {
                        p.fit_icon_columns();
                    }
                });
            });
        }
        {
            let w = w.clone();
            iv.connect_selection_changed(move |iv| {
                // Selected names are drawn over the items below (see
                // draw_whole_labels): GTK only redraws the item itself.
                iv.queue_draw();
                if let Some(p) = w.upgrade() {
                    p.trim_rubber_band();
                    p.emit(Event::Selection);
                }
            });
        }
        {
            let w = w.clone();
            iv.connect_item_activated(move |_, _| {
                if let Some(p) = w.upgrade() {
                    p.emit(Event::Activate);
                }
            });
        }
        {
            let w = w.clone();
            iv.connect_button_press_event(move |iv, ev| {
                let Some(p) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                p.keyboard_focus.set(false);
                // Clicks come in the bin window's coordinates.
                let (bx, by) = ev.position();
                let (ox, oy) = iv.convert_widget_to_bin_window_coords(0, 0);
                let (x, y) = (bx as i32 - ox, by as i32 - oy);
                let path = p.icon_hit(x, y);
                match ev.button() {
                    1 => p.icon_press(ev, iv.path_at_pos(bx as i32, by as i32).is_some(), path),
                    3 => {
                        iv.grab_focus();
                        match &path {
                            Some(tp) if !iv.path_is_selected(tp) => {
                                iv.unselect_all();
                                iv.select_path(tp);
                                iv.set_cursor(tp, None::<&gtk::CellRenderer>, false);
                            }
                            None => iv.unselect_all(),
                            _ => {}
                        }
                        p.emit(Event::Context {
                            event: Some(std::ops::Deref::deref(ev).clone()),
                            on_item: path.is_some(),
                        });
                        glib::Propagation::Stop
                    }
                    2 => {
                        let row = path.as_ref().and_then(|tp| p.row_at_filter_path(tp));
                        let press = ev.event_type() == gdk::EventType::ButtonPress;
                        match row {
                            Some(r) if r.is_dir => {
                                if press {
                                    p.emit(Event::OpenNewTab(gio::File::for_uri(&r.uri)));
                                }
                            }
                            Some(r) => {
                                if press {
                                    p.emit(Event::OpenFile(r));
                                }
                            }
                            None => {
                                if press {
                                    crate::scroll::autoscroll(iv.upcast_ref(), ev);
                                }
                            }
                        }
                        glib::Propagation::Stop
                    }
                    8 => {
                        p.emit(Event::Back);
                        glib::Propagation::Stop
                    }
                    9 => {
                        p.emit(Event::Forward);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        {
            let w = w.clone();
            iv.connect_button_release_event(move |iv, ev| {
                // A press on one of several selected items without a drag:
                // now just it is selected (see icon_press).
                if let Some(p) = w.upgrade() {
                    if ev.button() == 1 {
                        p.band.borrow_mut().take();
                        let taken = p.press_keep.borrow_mut().take();
                        if let Some((keep, clicked)) = taken {
                            iv.unselect_all();
                            iv.select_path(&clicked);
                            *p.released_keep.borrow_mut() = Some((keep, clicked));
                        }
                    }
                }
                glib::Propagation::Proceed
            });
        }
        {
            let w = w.clone();
            iv.connect_popup_menu(move |_| {
                if let Some(p) = w.upgrade() {
                    let on_item = !p.selected_paths().is_empty();
                    p.emit(Event::Context { event: None, on_item });
                }
                true
            });
        }
        // ---------------- tree view
        let tv = &self.tree_view;
        {
            let w = w.clone();
            tv.selection().connect_changed(move |_| {
                if let Some(p) = w.upgrade() {
                    p.emit(Event::Selection);
                }
            });
        }
        {
            let block = self.tv_block.clone();
            tv.selection()
                .set_select_function(Some(Box::new(move |_, _, _, _| !block.get())));
        }
        {
            let w = w.clone();
            tv.connect_row_activated(move |_, _, _| {
                if let Some(p) = w.upgrade() {
                    p.emit(Event::Activate);
                }
            });
        }
        {
            let w = w.clone();
            tv.connect_button_press_event(move |tv, ev| {
                let Some(p) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if ev.window() != tv.bin_window() {
                    return glib::Propagation::Proceed;
                }
                let (x, y) = ev.position();
                let path = tv
                    .path_at_pos(x as i32, y as i32)
                    .and_then(|(p, _, _, _)| p);
                let sel = tv.selection();
                match ev.button() {
                    3 => {
                        tv.grab_focus();
                        match &path {
                            Some(tp) if !sel.path_is_selected(tp) => {
                                sel.unselect_all();
                                sel.select_path(tp);
                                tv.set_cursor(tp, None::<&gtk::TreeViewColumn>, false);
                            }
                            None => sel.unselect_all(),
                            _ => {}
                        }
                        p.emit(Event::Context {
                            event: Some(std::ops::Deref::deref(ev).clone()),
                            on_item: path.is_some(),
                        });
                        glib::Propagation::Stop
                    }
                    2 => {
                        let row = path.as_ref().and_then(|tp| p.row_at_filter_path(tp));
                        let press = ev.event_type() == gdk::EventType::ButtonPress;
                        // The rows fill the list's width, leaving hardly any
                        // empty space, so files and folders autoscroll too
                        // (when it's on and there's something to scroll);
                        // otherwise, or with Shift, a middle-click opens
                        // them, a folder in a new tab.
                        if press {
                            let shift = ev.state().contains(gdk::ModifierType::SHIFT_MASK);
                            match row {
                                Some(r) if shift || !crate::scroll::autoscroll(tv.upcast_ref(), ev) => {
                                    if r.is_dir {
                                        p.emit(Event::OpenNewTab(gio::File::for_uri(&r.uri)));
                                    } else {
                                        p.emit(Event::OpenFile(r));
                                    }
                                }
                                Some(_) => {}
                                None => {
                                    crate::scroll::autoscroll(tv.upcast_ref(), ev);
                                }
                            }
                        }
                        glib::Propagation::Stop
                    }
                    8 => {
                        p.emit(Event::Back);
                        glib::Propagation::Stop
                    }
                    9 => {
                        p.emit(Event::Forward);
                        glib::Propagation::Stop
                    }
                    1 => {
                        let mods = ev.state()
                            & (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
                        match &path {
                            None => {
                                if mods.is_empty() {
                                    sel.unselect_all();
                                }
                            }
                            Some(tp) => {
                                if ev.event_type() == gdk::EventType::ButtonPress
                                    && mods.is_empty()
                                    && sel.path_is_selected(tp)
                                    && sel.count_selected_rows() > 1
                                {
                                    // Keep the multi-selection so it can be dragged.
                                    p.tv_block.set(true);
                                    *p.tv_pending.borrow_mut() = Some(tp.clone());
                                }
                            }
                        }
                        glib::Propagation::Proceed
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        {
            let w = w.clone();
            tv.connect_button_release_event(move |tv, _| {
                if let Some(p) = w.upgrade() {
                    if p.tv_block.get() {
                        p.tv_block.set(false);
                        let taken = p.tv_pending.borrow_mut().take();
                        if let Some(tp) = taken {
                            let sel = tv.selection();
                            sel.unselect_all();
                            sel.select_path(&tp);
                        }
                    }
                }
                glib::Propagation::Proceed
            });
        }
        {
            let w = w.clone();
            tv.connect_popup_menu(move |_| {
                if let Some(p) = w.upgrade() {
                    let on_item = !p.selected_paths().is_empty();
                    p.emit(Event::Context { event: None, on_item });
                }
                true
            });
        }
        // A right click on a column header: which columns to show.
        let held = |on: bool| HEADER_HELD.store(on, std::sync::atomic::Ordering::Relaxed);
        self.tree_view.connect_button_release_event(move |_, _| {
            held(false);
            glib::Propagation::Proceed
        });
        for (_, col) in &self.sort_cols {
            let Some(button) = col.button() else { continue };
            let w = w.clone();
            button.connect_button_release_event(move |_, _| {
                held(false);
                glib::Propagation::Proceed
            });
            button.connect_button_press_event(move |_, ev| {
                if ev.button() == 1 {
                    held(true);
                }
                if ev.button() != 3 {
                    return glib::Propagation::Proceed;
                }
                if let Some(p) = w.upgrade() {
                    p.column_menu(ev);
                }
                glib::Propagation::Stop
            });
        }
        // A column dragged to another place: kept in that order everywhere.
        {
            let w = w.clone();
            self.tree_view.connect_columns_changed(move |tv| {
                let Some(p) = w.upgrade() else { return };
                // A drag ends with its column put down.
                HEADER_HELD.store(false, std::sync::atomic::Ordering::Relaxed);
                // Not while columns are placed, or taken away (the pane closing).
                let all = tv.columns();
                if p.placing_columns.get() || p.columns_mode.get() || all.len() != p.detail_cols.len() + 1 {
                    return;
                }
                // The name stays first (a column dropped before it goes after).
                if all.first() != Some(&p.name_col) {
                    p.placing_columns.set(true);
                    tv.move_column_after(&p.name_col, None::<&gtk::TreeViewColumn>);
                    p.placing_columns.set(false);
                }
                let chosen = config::with(|s| crate::details::known(&s.list_columns));
                let order: Vec<&'static str> = all
                    .iter()
                    .filter_map(|c| p.detail_cols.iter().find(|(_, dc)| dc == c).map(|(id, _)| *id))
                    .filter(|id| chosen.contains(id))
                    .collect();
                if order != chosen {
                    config::update(|s| s.list_columns = order.iter().map(|id| id.to_string()).collect());
                    // The other panes, once GTK is done with the drag.
                    let w = Rc::downgrade(&p);
                    glib::idle_add_local_once(move || {
                        if let Some(p) = w.upgrade() {
                            p.emit(Event::ColumnsChanged);
                        }
                    });
                }
            });
        }
        for (id, col) in &self.sort_cols {
            let w = w.clone();
            let id = *id;
            col.connect_clicked(move |_| {
                if let Some(p) = w.upgrade() {
                    let (cur, desc) = p.sort_state();
                    let name = crate::details::sort_name(id);
                    let desc = if cur == name { !desc } else { crate::details::get(name).is_some_and(|d| d.desc_first) };
                    p.emit(Event::Sort { by: name.to_string(), desc });
                }
            });
        }

        // ---------------- DnD (both views)
        enable_drag_source_iv(&self.icon_view);
        enable_drag_source_tv(&self.tree_view);
        let views: [gtk::Widget; 2] = [self.icon_view.clone().upcast(), self.tree_view.clone().upcast()];
        for view in views {
            view.drag_dest_set(gtk::DestDefaults::empty(), &drop_targets(), dnd_actions());
            // Files are moved by ops, which the monitor picks up. Never let GTK
            // remove the dragged row from the store behind our index's back.
            view.connect_drag_data_delete(|v, _| v.stop_signal_emission_by_name("drag-data-delete"));
            {
                let w = w.clone();
                view.connect_drag_begin(move |_, _| {
                    // GTK draws the drag icon from the items' cells next:
                    // they need their pictures (see mark_drawing), until the
                    // handler below.
                    icons::set_drawing(true);
                    if let Some(p) = w.upgrade() {
                        // The whole selection is dragged (see icon_press).
                        p.press_keep.borrow_mut().take();
                        set_drag_source_dir(Some(p.current().uri().to_string()));
                        let all_dirs = p.selected().iter().all(|r| r.is_dir);
                        DRAG_ALL_DIRS.with(|d| d.set(Some(all_dirs)));
                        p.tv_block.set(false);
                        p.tv_pending.borrow_mut().take();
                    }
                });
            }
            view.connect_local("drag-begin", true, |_| {
                icons::set_drawing(false);
                None
            });
            view.connect_drag_end(|_, _| {
                set_drag_source_dir(None);
            });
            {
                let w = w.clone();
                view.connect_drag_data_get(move |_, _, sel, _, _| {
                    if let Some(p) = w.upgrade() {
                        let uris = p.selected_uris();
                        let refs: Vec<&str> = uris.iter().map(|s| s.as_str()).collect();
                        sel.set_uris(&refs);
                    }
                });
            }
            {
                let w = w.clone();
                view.connect_drag_motion(move |view, ctx, x, y, time| {
                    let Some(p) = w.upgrade() else { return false };
                    // Nothing it can take (text, or out of an archive on Wayland): say so.
                    if view.drag_dest_find_target(ctx, None).is_none() {
                        p.highlight_drop(None);
                        ctx.drag_status(gdk::DragAction::empty(), time);
                        return true;
                    }
                    let target = p.drop_target(x, y);
                    let from_self = ctx
                        .drag_get_source_widget()
                        .map(|s| &s == view)
                        .unwrap_or(false);
                    p.highlight_drop(target.as_ref().map(|t| &t.0));
                    let dest = match &target {
                        Some((_, uri)) => gio::File::for_uri(uri),
                        None => {
                            // Search results are no folder to drop into, as in Caja.
                            if from_self || p.is_searching() {
                                ctx.drag_status(gdk::DragAction::empty(), time);
                                return true;
                            }
                            p.current()
                        }
                    };
                    let a = choose_action(view, ctx, &dest);
                    ctx.drag_status(a, time);
                    true
                });
            }
            {
                let w = w.clone();
                view.connect_drag_leave(move |_, _, _| {
                    if let Some(p) = w.upgrade() {
                        p.highlight_drop(None);
                    }
                });
            }
            {
                let w = w.clone();
                view.connect_drag_drop(move |view, ctx, x, y, time| {
                    match view.drag_dest_find_target(ctx, None) {
                        // Out of an archive: the archive manager saves the files.
                        Some(t) if crate::xds::is(&t) => {
                            let dest = w.upgrade().and_then(|p| match p.drop_target(x, y) {
                                Some((_, uri)) => Some(gio::File::for_uri(&uri)),
                                None => (!p.is_searching()).then(|| p.current()),
                            });
                            if !dest.is_some_and(|d| crate::xds::start(view, ctx, &d, time)) {
                                ctx.drag_finish(false, false, time);
                            }
                        }
                        Some(t) => view.drag_get_data(ctx, &t, time),
                        None => ctx.drag_finish(false, false, time),
                    }
                    true
                });
            }
            {
                let w = w.clone();
                view.connect_drag_data_received(move |view, ctx, x, y, sel, _, time| {
                    // The views' own handlers would insert rows into the model.
                    view.stop_signal_emission_by_name("drag-data-received");
                    if crate::xds::is(&sel.target()) {
                        crate::xds::finish(ctx, sel, time);
                        if let Some(p) = w.upgrade() {
                            p.highlight_drop(None);
                        }
                        return;
                    }
                    let Some(p) = w.upgrade() else { return };
                    let mut uris: Vec<String> = sel.uris().iter().map(|s| s.to_string()).collect();
                    let target = p.drop_target(x, y);
                    if target.is_none() && p.is_searching() {
                        uris.clear();
                    }
                    let dest = target.map(|(_, u)| gio::File::for_uri(&u)).unwrap_or_else(|| p.current());
                    let action = choose_action(view, ctx, &dest);
                    p.highlight_drop(None);
                    ctx.drag_finish(!uris.is_empty(), false, time);
                    if !uris.is_empty() {
                        p.emit(Event::Drop { uris, dest, action });
                    }
                });
            }
        }

        // ---------------- inline rename
        for cell in [&self.icon_text, &self.list_text] {
            let in_icons = cell == &self.icon_text;
            {
                let w = w.clone();
                cell.connect_editing_started(move |_, editable, path| {
                    let row = w.upgrade().and_then(|p| p.row_at_filter_path(&path));
                    // Folders have no extension: select the whole name.
                    let is_dir = row.as_ref().map(|r| r.is_dir).unwrap_or(false);
                    // In the icon view the item's own name would show through
                    // the edit field: it's hidden now that the field has it.
                    if let (true, Some(p), Some(r)) = (in_icons, w.upgrade(), row.as_ref()) {
                        *p.edit_hidden.borrow_mut() = Some(r.uri.clone());
                        p.icon_view.queue_draw();
                    }
                    if let Some(entry) = editable.downcast_ref::<gtk::Entry>() {
                        // Some themes draw the field without a background.
                        entry.style_context().add_provider(&edit_field_css(), gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                        let entry = entry.clone();
                        glib::idle_add_local_once(move || {
                            let text = entry.text().to_string();
                            let (stem, _) = if is_dir { (text.as_str(), "") } else { util::split_ext(&text) };
                            entry.select_region(0, stem.chars().count() as i32);
                        });
                    }
                });
            }
            {
                let w = w.clone();
                cell.connect_edited(move |cell, path, new| {
                    cell.set_editable(false);
                    let Some(p) = w.upgrade() else { return };
                    p.editing.borrow_mut().take();
                    p.show_edited_name();
                    if let Some(r) = p.row_at_filter_path(&path) {
                        if !new.is_empty() && new != r.name {
                            p.emit(Event::Rename { uri: r.uri, new_name: new.to_string() });
                        }
                    }
                });
            }
            {
                let ed = self.editing.clone();
                let w = w.clone();
                cell.connect_editing_canceled(move |cell| {
                    cell.set_editable(false);
                    ed.borrow_mut().take();
                    if let Some(p) = w.upgrade() {
                        p.show_edited_name();
                    }
                });
            }
        }

        // ← / → in the icon views go on to the next row at a row's end (and
        // back to the previous row's end at its start), instead of stopping.
        {
            let w = w.clone();
            self.icon_view.connect_key_press_event(move |_, ev| match w.upgrade() {
                Some(p) => {
                    use gdk::keys::constants as k;
                    let nav = [k::Up, k::Down, k::Left, k::Right, k::Home, k::End, k::Page_Up, k::Page_Down, k::KP_Up, k::KP_Down, k::KP_Left, k::KP_Right];
                    if nav.contains(&ev.keyval()) {
                        if !ev.state().contains(gdk::ModifierType::SHIFT_MASK) {
                            p.range_anchor.set(None);
                        }
                        p.keyboard_focus.set(true);
                        p.icon_view.queue_draw();
                    }
                    p.icon_arrow_key(ev)
                }
                None => glib::Propagation::Proceed,
            });
        }

        // ---------------- typeahead & filter
        for view in [self.icon_view.clone().upcast::<gtk::Widget>(), self.tree_view.clone().upcast()] {
            let w = w.clone();
            view.connect_key_press_event(move |_, ev| {
                let Some(p) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if p.enter_opens(ev) {
                    return glib::Propagation::Stop;
                }
                p.typeahead_key(ev)
            });
            // Clicking or leaving the view ends typing to select.
            let w3 = Rc::downgrade(self);
            view.connect_button_press_event(move |_, _| {
                if let Some(p) = w3.upgrade() {
                    p.end_typeahead();
                }
                glib::Propagation::Proceed
            });
            let w4 = Rc::downgrade(self);
            view.connect_focus_out_event(move |_, _| {
                if let Some(p) = w4.upgrade() {
                    p.end_typeahead();
                }
                glib::Propagation::Proceed
            });
            let w2 = Rc::downgrade(self);
            view.connect_scroll_event(move |_, ev| {
                // While typing to select, the wheel goes to the previous /
                // next match, as in Caja.
                if let Some(p) = w2.upgrade() {
                    if !p.typeahead.borrow().is_empty() {
                        p.typeahead_scroll(ev);
                        return glib::Propagation::Stop;
                    }
                }
                if ev.state().contains(gdk::ModifierType::CONTROL_MASK) {
                    if let Some(p) = w2.upgrade() {
                        let dy = match ev.direction() {
                            gdk::ScrollDirection::Up => -1.0,
                            gdk::ScrollDirection::Down => 1.0,
                            gdk::ScrollDirection::Smooth => ev.delta().1,
                            _ => 0.0,
                        };
                        if dy < 0.0 {
                            p.zoom(1);
                        } else if dy > 0.0 {
                            p.zoom(-1);
                        }
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
    }

    // --------------------------------------------------------------- basics

    pub fn current(&self) -> gio::File {
        self.current.borrow().clone()
    }

    pub fn is_trash(&self) -> bool {
        self.current().uri().starts_with("trash:")
    }

    /// Clear History: forgets where this tab has been.
    pub fn clear_history(&self) {
        self.back.borrow_mut().clear();
        self.fwd.borrow_mut().clear();
    }

    /// Stop, as in Caja: what's loaded so far stays (and is kept up to
    /// date), the rest isn't read; a search ends with what it found.
    pub fn stop(self: &Rc<Self>) {
        if !self.loading.get() {
            return;
        }
        self.search_gen.store(0, std::sync::atomic::Ordering::SeqCst);
        // The loading sees a newer generation and gives up.
        let gen = self.gen.get() + 1;
        self.gen.set(gen);
        if self.is_searching() {
            self.loading.set(false);
            self.emit(Event::Loading(false));
        } else {
            self.finish_load(gen);
        }
    }

    pub fn can_back(&self) -> bool {
        !self.back.borrow().is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.fwd.borrow().is_empty()
    }

    /// Back and forward history as stored (oldest first / next first last).
    pub fn history(&self) -> (Vec<Place>, Vec<Place>) {
        (self.back.borrow().clone(), self.fwd.borrow().clone())
    }

    pub fn set_history(&self, back: Vec<Place>, fwd: Vec<Place>) {
        *self.back.borrow_mut() = back;
        *self.fwd.borrow_mut() = fwd;
    }

    pub fn history_back(&self) -> Vec<Place> {
        self.back.borrow().iter().rev().cloned().collect()
    }

    /// Where the pane is: its folder, or a search started there.
    pub fn place(&self) -> Place {
        Place { folder: self.current(), search: self.search.borrow().clone() }
    }

    fn set_place(&self, place: Place) {
        *self.current.borrow_mut() = place.folder;
        *self.search.borrow_mut() = place.search;
    }

    /// Shows `place` without adding to the history (a reopened tab).
    pub fn show_place(self: &Rc<Self>, place: Place) {
        self.set_place(place);
        self.load();
    }

    pub fn mode(&self) -> ViewMode {
        self.mode.get()
    }

    pub fn is_loading(&self) -> bool {
        self.loading.get()
    }

    pub fn active_view(&self) -> gtk::Widget {
        match self.mode.get() {
            ViewMode::List | ViewMode::Columns => self.tree_view.clone().upcast(),
            _ => self.icon_view.clone().upcast(),
        }
    }

    /// Calls `f` when the pointer moves into this pane's file view.
    /// Calls `f` with true when the pointer comes over the pane's view,
    /// false when it leaves it.
    pub fn connect_pointer_hover(&self, f: impl Fn(bool) + 'static) {
        let f = Rc::new(f);
        for view in [self.icon_view.clone().upcast::<gtk::Widget>(), self.tree_view.clone().upcast()] {
            view.add_events(gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
            // Moving between the view's own windows isn't coming or leaving.
            let f1 = f.clone();
            view.connect_enter_notify_event(move |_, ev| {
                if ev.detail() != gdk::NotifyType::Inferior {
                    f1(true);
                }
                glib::Propagation::Proceed
            });
            let f2 = f.clone();
            view.connect_leave_notify_event(move |_, ev| {
                if ev.detail() != gdk::NotifyType::Inferior {
                    f2(false);
                }
                glib::Propagation::Proceed
            });
        }
    }

    pub fn grab_focus(&self) {
        let view = self.active_view();
        if view.is_realized() {
            view.grab_focus();
            return;
        }
        // A new tab's view isn't on screen yet: it takes the focus once it
        // is (keys sent to it before, the release of the one that opened
        // it say, make GTK complain).
        let id: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
        let id2 = id.clone();
        *id.borrow_mut() = Some(view.connect_realize(move |v| {
            v.grab_focus();
            let taken = id2.borrow_mut().take();
            if let Some(id) = taken {
                v.disconnect(id);
            }
        }));
    }

    pub fn title(&self) -> String {
        if let Some(q) = self.search.borrow().as_ref() {
            return q.title();
        }
        let f = self.current();
        if f.uri() == "trash:///" {
            return "Trash".into();
        }
        if f.path().map(|p| p == std::path::Path::new("/")).unwrap_or(false) {
            return "File System".into();
        }
        if f.equal(&util::home_file()) {
            return "Home".into();
        }
        self.folder_display_name()
    }

    /// The open folder's display name, looked up once per folder.
    fn folder_display_name(&self) -> String {
        let uri = self.current().uri().to_string();
        if let Some((u, name)) = self.folder_name.borrow().as_ref() {
            if *u == uri {
                return name.clone();
            }
        }
        let name = util::display_name(&self.current());
        *self.folder_name.borrow_mut() = Some((uri, name.clone()));
        name
    }

    // --------------------------------------------------------------- navigation

    pub fn navigate(self: &Rc<Self>, file: &gio::File) {
        let cur = self.current();
        if !cur.equal(file) || self.is_searching() {
            self.back.borrow_mut().push(self.place());
            self.fwd.borrow_mut().clear();
            // Select the folder we came from when going up.
            if cur.has_parent(Some(file)) && !self.is_searching() {
                *self.pending_select.borrow_mut() = vec![cur.uri().to_string()];
            }
        }
        self.set_place(Place { folder: file.clone(), search: None });
        self.load();
    }

    pub fn navigate_initial(self: &Rc<Self>) {
        self.load();
    }

    pub fn go_back(self: &Rc<Self>) {
        let Some(prev) = self.back.borrow_mut().pop() else { return };
        let cur = self.place();
        if cur.search.is_none() && prev.search.is_none() && cur.folder.has_parent(Some(&prev.folder)) {
            *self.pending_select.borrow_mut() = vec![cur.folder.uri().to_string()];
        }
        self.fwd.borrow_mut().push(cur);
        self.set_place(prev);
        self.keep_columns.set(true);
        self.load();
    }

    pub fn go_back_n(self: &Rc<Self>, n: usize) {
        for _ in 0..n {
            let Some(prev) = self.back.borrow_mut().pop() else { break };
            let cur = self.place();
            self.fwd.borrow_mut().push(cur);
            self.set_place(prev);
        }
        self.load();
    }

    pub fn go_forward(self: &Rc<Self>) {
        let Some(next) = self.fwd.borrow_mut().pop() else { return };
        let cur = self.place();
        self.back.borrow_mut().push(cur);
        self.set_place(next);
        self.keep_columns.set(true);
        self.load();
    }

    pub fn go_up(self: &Rc<Self>) {
        if let Some(parent) = self.current().parent() {
            self.navigate(&parent);
        }
    }

    pub fn reload(self: &Rc<Self>) {
        *self.pending_select.borrow_mut() = self.selected_uris();
        self.load();
    }

    /// Empties the folder's rows with the views detached, as in refilter():
    /// clearing under an attached view intermittently trips the same
    /// gtk_tree_model_filter_real_unref_node assertion (seen when going to
    /// another folder in the list view).
    fn clear_rows(&self) {
        let many = self.index.borrow().len() > 1000;
        self.index.borrow_mut().clear();
        self.detach_model();
        self.store.clear();
        self.attach_model();
        if many {
            util::release_memory_soon();
        }
    }

    fn load(self: &Rc<Self>) {
        let gen = self.gen.get() + 1;
        self.gen.set(gen);
        // A search still running stops.
        self.search_gen.store(0, std::sync::atomic::Ordering::SeqCst);
        let taken = self.monitor.borrow_mut().take();
        if let Some(m) = taken {
            m.cancel();
        }
        self.info_bar.hide();
        self.trash_bar.set_visible(self.current().uri().starts_with("trash:"));
        self.end_typeahead();
        self.close_rename_editor(false);
        let query = self.search.borrow().clone();
        if let Some(q) = query {
            self.run_search(gen, &q);
            return;
        }
        self.query_editor.widget.hide();
        self.clear_rows();
        self.loading.set(true);
        *self.free_space.borrow_mut() = None;
        let file = self.current();
        self.apply_column_choice();
        self.tab_label.set_text(&self.title());
        self.tab_label.set_tooltip_text(Some(&util::pretty_location(&file)));
        self.apply_folder_settings();
        if self.columns_mode.get() {
            self.rebuild_columns();
        }
        self.emit(Event::Location);
        self.emit(Event::Loading(true));

        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let res = file
                .enumerate_children_future(ATTRS, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT)
                .await;
            let en = match res {
                Ok(en) => en,
                Err(e) => {
                    if let Some(p) = weak.upgrade() {
                        if p.gen.get() == gen {
                            p.show_error(&format!("Could not open “{}”: {}", util::pretty_location(&file), e.message()));
                            p.finish_load(gen);
                        }
                    }
                    return;
                }
            };
            // The first items show at once, the rest a second's worth at a
            // time: each addition makes the icon view measure all its items.
            let mut first = true;
            let mut waiting: Vec<gio::FileInfo> = vec![];
            let mut shown = std::time::Instant::now();
            loop {
                let batch = en.next_files_future(if first { 200 } else { 1000 }, glib::Priority::DEFAULT).await;
                first = false;
                let Some(p) = weak.upgrade() else { return };
                if p.gen.get() != gen {
                    let _ = en.close(gio::Cancellable::NONE);
                    return;
                }
                let done = match batch {
                    Ok(v) if v.is_empty() => true,
                    Ok(v) => {
                        waiting.extend(v);
                        false
                    }
                    Err(e) => {
                        p.show_error(&format!("Error while reading folder: {}", e.message()));
                        true
                    }
                };
                if !waiting.is_empty() && (done || p.store.iter_n_children(None) == 0 || shown.elapsed() >= Duration::from_secs(1)) {
                    p.add_infos(&file, std::mem::take(&mut waiting), false);
                    shown = std::time::Instant::now();
                    p.emit(Event::Selection);
                }
                if done {
                    break;
                }
            }
            let _ = en.close(gio::Cancellable::NONE);
            if let Some(p) = weak.upgrade() {
                if p.gen.get() == gen {
                    p.finish_load(gen);
                }
            }
        });
    }

    fn finish_load(self: &Rc<Self>, gen: u64) {
        self.loading.set(false);
        // The columns view's "Empty folder" waits for the loading to end.
        self.tree_view.queue_draw();
        let file = self.current();
        // monitor
        if let Ok(m) = file.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
            m.set_rate_limit(400);
            let w = Rc::downgrade(self);
            m.connect_changed(move |_, f, other, ev| {
                if let Some(p) = w.upgrade() {
                    if p.gen.get() == gen {
                        p.on_monitor(f, other, ev);
                    }
                }
            });
            *self.monitor.borrow_mut() = Some(m);
        }
        // selection
        let sel = std::mem::take(&mut *self.pending_select.borrow_mut());
        if !sel.is_empty() {
            self.select_uris(&sel);
        } else if self.select_first.replace(false) && self.filter.iter_first().is_some() {
            self.select_filter_paths(&[gtk::TreePath::from_indicesv(&[0])]);
        } else {
            self.scroll_to_top();
        }
        // A right-click in a side column that brought us here.
        let taken = self.pending_context.borrow_mut().take();
        if let Some((event, on_item)) = taken {
            self.emit(Event::Context { event: Some(event), on_item });
        }
        // Taken first: borrowed in the condition, it would stay borrowed
        // in the block (Rust 2021), and code called there may borrow it again.
        let taken = self.pending_rename.borrow_mut().take();
        if let Some(uri) = taken {
            self.start_rename(&uri);
        }
        self.count_dir_items(gen);
        // free space
        let w = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            if let Ok(info) = file
                .query_filesystem_info_future("filesystem::free", glib::Priority::LOW)
                .await
            {
                if let Some(p) = w.upgrade() {
                    if p.gen.get() == gen && info.has_attribute("filesystem::free") {
                        *p.free_space.borrow_mut() = Some(info.attribute_uint64("filesystem::free"));
                        p.emit(Event::Selection);
                    }
                }
            }
        });
        self.emit(Event::Loading(false));
        self.emit(Event::Selection);
    }

    fn scroll_to_top(&self) {
        if let Some(it) = self.filter.iter_first() {
            let path = self.filter.path(&it).unwrap();
            // Only the view in use has the model (see attach_model).
            match self.mode.get() {
                ViewMode::List | ViewMode::Columns => {
                    util::scroll_tree_to(&self.tree_view, &path, false, 0.0, 0.0);
                    util::set_tree_cursor(&self.tree_view, &path);
                    self.tree_view.selection().unselect_all();
                }
                _ => {
                    self.scroll_icon_view(&path, false, 0.0, true);
                    self.icon_view.unselect_all();
                }
            }
        }
    }

    fn show_error(&self, msg: &str) {
        self.info_label.set_text(msg);
        self.info_bar.show();
    }

    /// Count entries of sub-folders in a background thread ("12 items").
    fn count_dir_items(self: &Rc<Self>, gen: u64) {
        let mut dirs = vec![];
        let store = &self.store;
        for it in store.rows() {
            if store.gv::<bool>(&it, COL_IS_DIR as i32) {
                let uri: String = store.gv(&it, COL_URI as i32);
                if let Some(p) = gio::File::for_uri(&uri).path() {
                    dirs.push((uri, p));
                }
            }
        }
        self.count_dirs(gen, dirs);
    }

    /// Count the entries of these folders in a thread and fill in their size column.
    fn count_dirs(self: &Rc<Self>, gen: u64, dirs: Vec<(String, std::path::PathBuf)>) {
        if dirs.is_empty() {
            return;
        }
        let hidden = self.show_hidden.get();
        let (tx, rx) = async_channel::unbounded::<Vec<(String, Option<u64>)>>();
        std::thread::spawn(move || {
            // Sent every half second: each change makes the icon view
            // measure all its items again.
            let mut batch = vec![];
            let mut sent = std::time::Instant::now();
            for (uri, path) in dirs {
                batch.push((uri, count_entries(&path, hidden)));
                if sent.elapsed() >= Duration::from_millis(500) {
                    sent = std::time::Instant::now();
                    if tx.send_blocking(std::mem::take(&mut batch)).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send_blocking(batch);
        });
        let w = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(batch) = rx.recv().await {
                let Some(p) = w.upgrade() else { return };
                if p.gen.get() != gen {
                    return;
                }
                p.quietly(|| {
                    for (uri, n) in batch {
                        p.set_dir_count(&uri, n);
                    }
                });
            }
        });
    }

    /// Changes rows without the icon view noticing, then redraws it: for
    /// what doesn't change the size of its items (item counts it doesn't
    /// show, thumbnails drawn at a fixed size). Any row change makes
    /// GtkIconView measure all its items again: with thousands that takes
    /// long, and these come in all the time while a folder is loading.
    fn quietly(&self, f: impl FnOnce()) {
        use glib::gobject_ffi;
        let model = self.filter.upcast_ref::<glib::Object>().as_ptr();
        let data = self.icon_view.upcast_ref::<glib::Object>().as_ptr() as glib::ffi::gpointer;
        let mask = gobject_ffi::G_SIGNAL_MATCH_ID | gobject_ffi::G_SIGNAL_MATCH_DATA;
        unsafe {
            let id = gobject_ffi::g_signal_lookup(c"row-changed".as_ptr(), gtk::TreeModel::static_type().into_glib());
            gobject_ffi::g_signal_handlers_block_matched(model as _, mask, id, 0, std::ptr::null_mut(), std::ptr::null_mut(), data);
            f();
            gobject_ffi::g_signal_handlers_unblock_matched(model as _, mask, id, 0, std::ptr::null_mut(), std::ptr::null_mut(), data);
        }
        self.icon_view.queue_draw();
    }

    fn set_dir_count(&self, uri: &str, n: Option<u64>) {
        if self.editing.borrow().as_deref() == Some(uri) {
            return;
        }
        let it = self.index.borrow().get(uri).cloned();
        if let Some(it) = it {
            let text = match n {
                Some(1) => "1 item".to_string(),
                Some(n) => format!("{n} items"),
                None => "? items".into(),
            };
            self.store
                .set(&it, &[(COL_SIZE, &n.unwrap_or(0)), (COL_SIZE_TEXT, &text)]);
        }
    }

    fn set_thumb(&self, uri: &str, path: &str) {
        let it = self.index.borrow().get(uri).cloned();
        if let Some(it) = it {
            self.store.set(&it, &[(COL_THUMB, &path)]);
        }
    }

    fn on_monitor(self: &Rc<Self>, f: &gio::File, other: Option<&gio::File>, ev: gio::FileMonitorEvent) {
        use gio::FileMonitorEvent as E;
        match ev {
            E::Created | E::MovedIn | E::ChangesDoneHint | E::AttributeChanged => self.refresh_file(f),
            E::Deleted | E::MovedOut => self.remove_uri(&f.uri()),
            E::Renamed => {
                self.remove_uri(&f.uri());
                if let Some(o) = other {
                    self.refresh_file(o);
                }
            }
            _ => {}
        }
    }

    /// Reads a file again and adds or updates its row. Changes arriving
    /// together (files copied in: an event or two each) are done as one
    /// batch, like removals: each row added on its own makes the icon view
    /// measure all its items again.
    pub fn refresh_file(self: &Rc<Self>, f: &gio::File) {
        let first = {
            let mut q = self.pending_refresh.borrow_mut();
            let first = q.is_empty();
            if self.pending_refresh_uris.borrow_mut().insert(f.uri().to_string()) {
                q.push(f.clone());
            }
            first
        };
        if !first {
            return;
        }
        let w = Rc::downgrade(self);
        let gen = self.gen.get();
        let delay = self.batch_delay(&self.last_refresh, Duration::from_millis(100));
        glib::timeout_add_local_once(delay, move || {
            let Some(p) = w.upgrade() else { return };
            p.last_refresh.set(Some(Instant::now()));
            p.pending_refresh_uris.borrow_mut().clear();
            let files = std::mem::take(&mut *p.pending_refresh.borrow_mut());
            if p.gen.get() != gen {
                return;
            }
            glib::MainContext::default().spawn_local(async move {
                let mut found = vec![];
                let mut gone = vec![];
                for f in files {
                    match f.query_info_future(ATTRS, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await {
                        Ok(info) => found.push((f, info)),
                        Err(_) => gone.push(f.uri().to_string()),
                    }
                }
                let Some(p) = w.upgrade() else { return };
                if p.gen.get() != gen {
                    return;
                }
                for uri in gone {
                    p.remove_uri(&uri);
                }
                let parent = p.current();
                let infos: Vec<gio::FileInfo> = found
                    .into_iter()
                    .filter(|(f, _)| f.parent().is_some_and(|x| x.equal(&parent)))
                    .map(|(_, info)| info)
                    .collect();
                p.add_infos(&parent, infos, true);
                p.emit(Event::Selection);
            });
        });
    }

    /// Synchronously add a file (used right after creating it).
    pub fn add_file_now(self: &Rc<Self>, f: &gio::File) {
        if let Ok(info) = f.query_info(ATTRS, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
            let parent = self.current();
            self.add_info(&parent, &info, true);
        }
    }

    /// add_file_now for many files (a paste's results), in one batch, and
    /// only those not shown yet: the folder's monitor has added most by
    /// the time an operation ends, and adding thousands again costs time.
    pub fn add_files_now(self: &Rc<Self>, files: &[gio::File]) {
        let infos = files
            .iter()
            .filter(|f| !self.index.borrow().contains_key(f.uri().as_str()))
            .filter_map(|f| f.query_info(ATTRS, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok())
            .collect();
        self.add_infos(&self.current(), infos, true);
    }

    /// How long to wait before the next batch of removals or changes: at
    /// least `base` (for the events that come together to arrive), and in
    /// a big folder until a while after the last batch. Each batch costs
    /// time in proportion to all the rows there are, so a stream of
    /// changes (thousands of files copied in or deleted) comes in fewer,
    /// bigger batches the more rows there are: up to one a second. The
    /// first change after a quiet while is still shown at once.
    fn batch_delay(&self, last: &Cell<Option<Instant>>, base: Duration) -> Duration {
        let rows = self.store.iter_n_children(None).max(0) as u64;
        let window = if rows < 2000 { Duration::ZERO } else { Duration::from_millis((rows / 10).min(1000)) };
        let since = last.get().map_or(Duration::MAX, |t| t.elapsed());
        base.max(window.saturating_sub(since))
    }

    /// Removes a deleted file's row. Removals arriving together (a folder
    /// emptied, a phone unmounted: one event per file) are done as one
    /// batch with the views detached, as in refilter(): removing many rows
    /// one by one under an attached view tripped
    /// gtk_tree_model_filter_real_unref_node's ref-count assertion.
    fn remove_uri(self: &Rc<Self>, uri: &str) {
        let first = {
            let mut q = self.pending_removals.borrow_mut();
            q.push(uri.to_string());
            q.len() == 1
        };
        if first {
            let w = Rc::downgrade(self);
            let flush = move || {
                if let Some(p) = w.upgrade() {
                    p.flush_removals();
                }
            };
            match self.batch_delay(&self.last_removals, Duration::ZERO) {
                Duration::ZERO => {
                    glib::idle_add_local_once(flush);
                }
                delay => {
                    glib::timeout_add_local_once(delay, flush);
                }
            }
        }
    }

    fn flush_removals(&self) {
        self.last_removals.set(Some(Instant::now()));
        let uris = std::mem::take(&mut *self.pending_removals.borrow_mut());
        // Rows of a folder left meanwhile are already gone from the index.
        let rows: Vec<gtk::TreeIter> = {
            let mut index = self.index.borrow_mut();
            uris.iter().filter_map(|u| index.remove(u)).collect()
        };
        match rows.len() {
            0 => return,
            // One at a time while attached (see the filter's row-deleted
            // handler in new for the icon view's references).
            1 => {
                self.store.remove(&rows[0]);
            }
            _ => self.detached(|| self.remove_rows(&rows)),
        }
        self.emit(Event::Selection);
    }

    /// Removes these rows from the store. The filter model moves every row
    /// after a removed one (time for all rows, for each row removed), so
    /// many are first moved to the end, keeping the order of the others,
    /// and removed from there, last first: then none has rows after it.
    fn remove_rows(&self, rows: &[gtk::TreeIter]) {
        let store = &self.store;
        if rows.len() < 50 {
            for it in rows {
                store.remove(it);
            }
            return;
        }
        let n = store.iter_n_children(None).max(0) as u32;
        let mut gone: Vec<(u32, &gtk::TreeIter)> = rows
            .iter()
            .filter_map(|it| Some((store.path(it)?.indices().first().copied()? as u32, it)))
            .collect();
        gone.sort_by_key(|(i, _)| *i);
        let doomed: std::collections::HashSet<u32> = gone.iter().map(|(i, _)| *i).collect();
        // new_order[new place] = old place. (The iters stay valid.)
        let order: Vec<u32> = (0..n).filter(|i| !doomed.contains(i)).chain(gone.iter().map(|(i, _)| *i)).collect();
        let sort = store.sort_column_id();
        store.set_unsorted();
        store.reorder(&order);
        for (_, it) in gone.iter().rev() {
            store.remove(it);
        }
        // The rest kept their order: sorting again changes nothing.
        if let Some((col, dir)) = sort {
            store.set_sort_column_id(col, dir);
        }
    }

    /// Adds or updates rows for these items of `parent` (and counts the
    /// items in the folders among them, with `counts`). Many at once are
    /// added with the views detached (see there), except into an empty
    /// view, which shows them at once, and a few compared with the rows
    /// there are: detaching costs time for all rows (sorting them again,
    /// selecting the selected again), each row added attached only its own
    /// share.
    fn add_infos(self: &Rc<Self>, parent: &gio::File, infos: Vec<gio::FileInfo>, counts: bool) {
        self.add_rows(infos.into_iter().map(|i| (parent.clone(), i)).collect(), counts);
    }

    /// add_infos for items of any folders: (folder, item) pairs.
    fn add_rows(self: &Rc<Self>, rows: Vec<(gio::File, gio::FileInfo)>, counts: bool) {
        let there = self.store.iter_n_children(None).max(0) as usize;
        if rows.len() <= 20 || there == 0 || rows.len() * 4 <= there {
            for (parent, info) in &rows {
                self.add_info(parent, info, false);
            }
        } else {
            self.detached(|| {
                // Added at the end and then sorted all at once: each row
                // sorted in on its own moves all the rows after it in the
                // filter model.
                let sort = self.store.sort_column_id();
                self.store.set_unsorted();
                for (parent, info) in &rows {
                    self.add_info(parent, info, false);
                }
                if let Some((col, order)) = sort {
                    self.store.set_sort_column_id(col, order);
                }
            });
        }
        if counts {
            let dirs: Vec<(String, std::path::PathBuf)> = rows
                .iter()
                .filter(|(_, i)| i.file_type() == gio::FileType::Directory)
                .filter_map(|(parent, i)| {
                    let f = parent.child(i.name());
                    Some((f.uri().to_string(), f.path()?))
                })
                .collect();
            self.count_dirs(self.gen.get(), dirs);
        }
    }

    /// Runs `f`, which changes many rows, with the views detached from the
    /// model: attached, each row added or removed costs them time in
    /// proportion to all the rows there are.
    fn detached(&self, f: impl FnOnce()) {
        // Detaching resets scrolling and selection. Keep the selection, and
        // the first row in view that stays there (as GTK does when rows
        // change one at a time), or the top.
        let sel = self.selected_uris();
        let in_view = |range: Option<(gtk::TreePath, gtk::TreePath)>| -> Vec<String> {
            let mut out = vec![];
            if let Some((mut p, last)) = range {
                while let Some(it) = self.filter.iter(&p) {
                    out.push(self.filter.gv::<String>(&it, COL_URI as i32));
                    if p == last || out.len() > 1000 {
                        break;
                    }
                    p.next();
                }
            }
            out
        };
        let at_top = |a: Option<gtk::Adjustment>| a.is_none_or(|a| a.value() <= a.lower());
        let icons_top = if at_top(self.icon_view.vadjustment()) { vec![] } else { in_view(self.icon_view.visible_range()) };
        let list_top = if at_top(self.tree_view.vadjustment()) { vec![] } else { in_view(self.tree_view.visible_range()) };
        self.detach_model();
        f();
        self.attach_model();
        if !sel.is_empty() {
            self.select_uris(&sel);
        }
        let path_of = |uris: &[String]| {
            let index = self.index.borrow();
            uris.iter()
                .filter_map(|u| index.get(u))
                .find_map(|it| self.filter.convert_child_iter_to_iter(it))
                .and_then(|it| self.filter.path(&it))
        };
        if let Some(p) = path_of(&icons_top) {
            self.scroll_icon_view(&p, true, 0.0, false);
        }
        if let Some(p) = path_of(&list_top) {
            util::scroll_tree_to(&self.tree_view, &p, true, 0.0, 0.0);
        }
    }

    fn add_info(self: &Rc<Self>, parent: &gio::File, info: &gio::FileInfo, update_counts: bool) {
        let name = info.name();
        let child = parent.child(&name);
        let uri = child.uri().to_string();
        let display = info.display_name().to_string();
        let ftype = info.file_type();
        let is_dir = ftype == gio::FileType::Directory
            || ftype == gio::FileType::Mountable;
        let ctype = util::info_content_type(info).unwrap_or_else(|| "application/octet-stream".into());
        let size = info.size().max(0) as u64;
        let mtime = info.attribute_uint64("time::modified") as i64;
        let hidden = util::info_is_hidden(info) || util::info_is_backup(info);
        let mut icon = info.icon();
        let unreadable = info.has_attribute("access::can-read") && !info.boolean("access::can-read");
        if let Some(base) = icon.clone() {
            let mut emblem = None;
            if util::info_is_symlink(info) {
                emblem = Some("emblem-symbolic-link");
            } else if unreadable {
                emblem = Some("emblem-unreadable");
            }
            if let Some(e) = emblem {
                let em = gio::Emblem::new(&gio::ThemedIcon::new(e));
                icon = Some(gio::EmblemedIcon::new(&base, Some(&em)).upcast());
            }
        }
        let mut thumb = String::new();
        // Missing ones are asked for once the item is shown (see row_pixbuf).
        if config::with(|s| s.thumbnails) && !is_dir && icons::can_thumbnail(&ctype) && child.path().is_some() {
            thumb = icons::existing_thumbnail(info, &uri, mtime).unwrap_or_default();
        }
        let orig = info
            .attribute_byte_string("trash::orig-path")
            .map(|p| {
                std::path::Path::new(p.as_str())
                    .parent()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let (size_val, size_text) = if is_dir { (0u64, String::new()) } else { (size, util::format_size(size)) };
        let tdesc = if util::info_is_symlink(info) && !is_dir {
            format!("Link ({})", type_description(&ctype, false))
        } else {
            type_description(&ctype, is_dir)
        };
        let key = collate_key(&display);
        let mtext = util::format_time(mtime);
        let atime = info.attribute_uint64("time::access") as i64;
        let btime = info.attribute_uint64("time::created") as i64;
        let owner = info.attribute_string("owner::user").map(|s| s.to_string()).unwrap_or_default();
        let group = info.attribute_string("owner::group").map(|s| s.to_string()).unwrap_or_default();
        let mode = info.attribute_uint32("unix::mode");
        let alloc = info.attribute_uint64("standard::allocated-size");
        let trashed = trash_time(info);

        if self.editing.borrow().as_deref() == Some(uri.as_str()) {
            return; // don't disturb an in-progress rename
        }
        let existing = self.index.borrow().get(&uri).cloned();
        match existing {
            Some(it) => {
                let (sv, st) = if is_dir {
                    (
                        self.store.gv::<u64>(&it, COL_SIZE as i32),
                        self.store.gv::<String>(&it, COL_SIZE_TEXT as i32),
                    )
                } else {
                    (size_val, size_text.clone())
                };
                // GtkIconView measures all its items again on any row
                // changing: leave rows read again unchanged be (a file
                // copied in is reported when created and when done).
                let st_ = &self.store;
                let same = st_.gv::<String>(&it, COL_NAME as i32) == display
                    && st_.gv::<u64>(&it, COL_SIZE as i32) == sv
                    && st_.gv::<i64>(&it, COL_MTIME as i32) == mtime
                    && st_.gv::<String>(&it, COL_TYPE as i32) == tdesc
                    && st_.gv::<bool>(&it, COL_IS_DIR as i32) == is_dir
                    && st_.gv::<bool>(&it, COL_HIDDEN as i32) == hidden
                    && st_.gv::<String>(&it, COL_THUMB as i32) == thumb
                    && st_.gv::<String>(&it, COL_CTYPE as i32) == ctype
                    && st_.gv::<String>(&it, COL_ORIG as i32) == orig
                    && st_.gv::<i64>(&it, COL_ATIME as i32) == atime
                    && st_.gv::<i64>(&it, COL_BTIME as i32) == btime
                    && st_.gv::<String>(&it, COL_OWNER as i32) == owner
                    && st_.gv::<String>(&it, COL_GROUP as i32) == group
                    && st_.gv::<u32>(&it, COL_MODE as i32) == mode
                    && st_.gv::<u64>(&it, COL_ALLOC as i32) == alloc
                    && st_.gv::<i64>(&it, COL_TRASHED as i32) == trashed
                    && match (st_.gv::<Option<gio::Icon>>(&it, COL_ICON as i32), &icon) {
                        (Some(a), Some(b)) => a.equal(Some(b)),
                        (a, b) => a.is_none() && b.is_none(),
                    };
                if same {
                    return;
                }
                self.store.set(
                    &it,
                    &[
                        (COL_ICON, &icon),
                        (COL_NAME, &display),
                        (COL_SIZE, &sv),
                        (COL_SIZE_TEXT, &st),
                        (COL_MTIME, &mtime),
                        (COL_MTIME_TEXT, &mtext),
                        (COL_TYPE, &tdesc),
                        (COL_IS_DIR, &is_dir),
                        (COL_HIDDEN, &hidden),
                        (COL_THUMB, &thumb),
                        (COL_CTYPE, &ctype),
                        (COL_KEY, &key),
                        (COL_ORIG, &orig),
                        (COL_ATIME, &atime),
                        (COL_BTIME, &btime),
                        (COL_OWNER, &owner),
                        (COL_GROUP, &group),
                        (COL_MODE, &mode),
                        (COL_ALLOC, &alloc),
                        (COL_TRASHED, &trashed),
                    ],
                );
            }
            None => {
                let it = self.store.insert_with_values(
                    None,
                    &[
                        (COL_ICON, &icon),
                        (COL_NAME, &display),
                        (COL_URI, &uri),
                        (COL_SIZE, &size_val),
                        (COL_SIZE_TEXT, &size_text),
                        (COL_MTIME, &mtime),
                        (COL_MTIME_TEXT, &mtext),
                        (COL_TYPE, &tdesc),
                        (COL_IS_DIR, &is_dir),
                        (COL_HIDDEN, &hidden),
                        (COL_THUMB, &thumb),
                        (COL_CTYPE, &ctype),
                        (COL_KEY, &key),
                        (COL_ORIG, &orig),
                        (COL_ATIME, &atime),
                        (COL_BTIME, &btime),
                        (COL_OWNER, &owner),
                        (COL_GROUP, &group),
                        (COL_MODE, &mode),
                        (COL_ALLOC, &alloc),
                        (COL_TRASHED, &trashed),
                    ],
                );
                self.index.borrow_mut().insert(uri.clone(), it);
            }
        }
        if update_counts && is_dir {
            if let Some(p) = child.path() {
                self.count_dirs(self.gen.get(), vec![(uri, p)]);
            }
        }
    }

    // --------------------------------------------------------------- selection

    fn selected_paths(&self) -> Vec<gtk::TreePath> {
        match self.mode.get() {
            // No model while refilter() has the views detached.
            ViewMode::List | ViewMode::Columns if self.tree_view.model().is_none() => vec![],
            ViewMode::List | ViewMode::Columns => self.tree_view.selection().selected_rows().0,
            _ => {
                let mut v = self.icon_view.selected_items();
                v.sort_by_key(|a| a.indices());
                v
            }
        }
    }

    fn row_at_filter_path(&self, path: &gtk::TreePath) -> Option<RowInfo> {
        let it = self.filter.iter(path)?;
        Some(self.row_info(self.filter.upcast_ref(), &it))
    }

    fn row_info(&self, m: &gtk::TreeModel, it: &gtk::TreeIter) -> RowInfo {
        RowInfo {
            uri: m.gv(it, COL_URI as i32),
            name: m.gv(it, COL_NAME as i32),
            is_dir: m.gv(it, COL_IS_DIR as i32),
            ctype: m.gv(it, COL_CTYPE as i32),
            size: m.gv(it, COL_SIZE as i32),
            orig: m.gv(it, COL_ORIG as i32),
        }
    }

    pub fn selected(&self) -> Vec<RowInfo> {
        self.selected_paths()
            .iter()
            .filter_map(|p| self.row_at_filter_path(p))
            .collect()
    }

    pub fn selected_uris(&self) -> Vec<String> {
        self.selected().into_iter().map(|r| r.uri).collect()
    }

    pub fn selected_files(&self) -> Vec<gio::File> {
        self.selected_uris().iter().map(|u| gio::File::for_uri(u)).collect()
    }

    /// ← / → in the icon and compact views: the previous / next item in
    /// order, which runs on across the ends of the rows; with Shift the
    /// selection grows or shrinks that way. Shift+Home / End select up to
    /// the first / last item, as in the list view. (Plain Home / End and
    /// Ctrl combinations are GTK's.)
    fn icon_arrow_key(&self, ev: &gdk::EventKey) -> glib::Propagation {
        use gdk::keys::constants as k;
        let shift = ev.state().contains(gdk::ModifierType::SHIFT_MASK);
        let mods = ev.state() & (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::MOD1_MASK | gdk::ModifierType::SUPER_MASK);
        let key = ev.keyval();
        let n = self.filter.iter_n_children(None);
        // Where the cursor goes: a step, or (with Shift) to an end.
        enum To {
            Step(i32),
            End(i32),
        }
        let to = if key == k::Right || key == k::KP_Right {
            To::Step(1)
        } else if key == k::Left || key == k::KP_Left {
            To::Step(-1)
        } else if shift && (key == k::Home || key == k::KP_Home) {
            To::End(0)
        } else if shift && (key == k::End || key == k::KP_End) {
            To::End(n - 1)
        } else {
            return glib::Propagation::Proceed;
        };
        if !mods.is_empty() || !matches!(self.mode.get(), ViewMode::Icon | ViewMode::Compact) || n == 0 {
            return glib::Propagation::Proceed;
        }
        let cur = self.icon_view.cursor().and_then(|(p, _)| p.indices().first().copied());
        let next = match (to, cur) {
            (To::End(i), _) => i,
            (To::Step(step), Some(i)) => (i + step).clamp(0, n - 1),
            (To::Step(_), None) => 0,
        };
        // Shift+Home / End without a cursor yet start from the first item.
        let cur = if shift { cur.or(Some(0)) } else { cur };
        let path = |i: i32| gtk::TreePath::from_indicesv(&[i]);
        match cur.filter(|_| shift) {
            // The selection runs from its far end (the end the cursor isn't
            // at, as with GTK's Shift+↑/↓) to the new cursor position.
            Some(c) => {
                let sel: Vec<i32> = self.icon_view.selected_items().iter().filter_map(|p| p.indices().first().copied()).collect();
                let (lo, hi) = (sel.iter().min().copied().unwrap_or(c), sel.iter().max().copied().unwrap_or(c));
                let anchor = if c == hi { lo } else if c == lo { hi } else { c };
                self.icon_view.set_cursor(&path(next), None::<&gtk::CellRenderer>, false);
                self.icon_view.unselect_all();
                for i in anchor.min(next)..=anchor.max(next) {
                    self.icon_view.select_path(&path(i));
                }
                self.scroll_icon_view(&path(next), false, 0.0, false);
            }
            None => self.select_filter_paths(&[path(next)]),
        }
        glib::Propagation::Stop
    }

    /// Scrolls the icon view to the item at `path` (and with `cursor` puts
    /// the cursor on it, which scrolls too). Not laid out yet (just added,
    /// the view just attached), GtkIconView (GTK 3.24) would keep the path
    /// in a row reference that it doesn't move when rows above are removed:
    /// freed later, it lets go of another row's reference (the filter
    /// model's "elt->ref_count > 0" assertion). So the item is remembered
    /// by its URI here and scrolled to once it is laid out.
    fn scroll_icon_view(&self, path: &gtk::TreePath, center: bool, row_align: f32, cursor: bool) {
        if self.icon_view.is_realized() && cell_rect(&self.icon_view, path, &self.icon_text).is_some() {
            *self.pending_scroll.borrow_mut() = None;
            if cursor {
                self.icon_view.set_cursor(path, None::<&gtk::CellRenderer>, false);
            }
            self.icon_view.scroll_to_path(path, center, row_align, 0.0);
            return;
        }
        let Some(uri) = self.filter.iter(path).map(|it| self.filter.gv::<String>(&it, COL_URI as i32)) else { return };
        let first = self.pending_scroll.replace(Some((uri, center, row_align, cursor))).is_none();
        self.scroll_tries.set(0);
        if first {
            self.retry_scroll();
        }
    }

    fn retry_scroll(&self) {
        let w = self.me.borrow().clone();
        glib::timeout_add_local_once(Duration::from_millis(30), move || {
            let Some(p) = w.upgrade() else { return };
            let Some((uri, center, align, cursor)) = p.pending_scroll.borrow().clone() else { return };
            let path = p.index.borrow().get(&uri).and_then(|it| p.filter.convert_child_iter_to_iter(it)).and_then(|f| p.filter.path(&f));
            let Some(path) = path.filter(|_| p.icon_view.model().is_some()) else {
                // Gone, or the view shows another model: nothing to scroll to.
                *p.pending_scroll.borrow_mut() = None;
                return;
            };
            if p.icon_view.is_realized() && cell_rect(&p.icon_view, &path, &p.icon_text).is_some() {
                *p.pending_scroll.borrow_mut() = None;
                if cursor {
                    p.icon_view.set_cursor(&path, None::<&gtk::CellRenderer>, false);
                }
                p.icon_view.scroll_to_path(&path, center, align, 0.0);
            } else if p.scroll_tries.get() < 100 {
                p.scroll_tries.set(p.scroll_tries.get() + 1);
                p.retry_scroll();
            } else {
                *p.pending_scroll.borrow_mut() = None;
            }
        });
    }

    fn select_filter_paths(&self, paths: &[gtk::TreePath]) {
        self.select_filter_paths_scrolling(paths, true);
    }

    /// Selects `paths`, scrolled to the first: into the middle with
    /// `center`, else only as far as needed to see it (not at all when it's
    /// in view: typing to select and its wheel steps leave the view be).
    fn select_filter_paths_scrolling(&self, paths: &[gtk::TreePath], center: bool) {
        let sel = self.tree_view.selection();
        sel.unselect_all();
        match self.mode.get() {
            ViewMode::List | ViewMode::Columns => {
                self.icon_view.unselect_all();
                for (i, p) in paths.iter().enumerate() {
                    if i == 0 {
                        util::set_tree_cursor(&self.tree_view, p);
                        util::scroll_tree_to(&self.tree_view, p, center, 0.4, 0.0);
                        sel.unselect_all();
                    }
                    sel.select_path(p);
                }
            }
            _ => {
                if let Some(first) = paths.first() {
                    self.scroll_icon_view(first, center, 0.4, true);
                }
                // All at once (see iconview::set_selection), else item by item.
                let mut indices: Vec<i32> = paths.iter().filter_map(|p| p.indices().first().copied()).collect();
                indices.sort_unstable();
                indices.dedup();
                if !crate::iconview::IconView::set_selection(&self.icon_view, &indices) {
                    self.icon_view.unselect_all();
                    for p in paths {
                        self.icon_view.select_path(p);
                    }
                }
            }
        }
    }

    pub fn select_uris(&self, uris: &[String]) {
        let mut paths = vec![];
        for u in uris {
            let it = self.index.borrow().get(u).cloned();
            if let Some(it) = it {
                if let Some(fit) = self.filter.convert_child_iter_to_iter(&it) {
                    paths.push(self.filter.path(&fit).unwrap());
                }
            }
        }
        if !paths.is_empty() {
            paths.sort_by_key(|a| a.indices());
            self.select_filter_paths(&paths);
        }
    }

    /// Select once loading finishes (or now, if already loaded).
    pub fn select_uris_later(&self, uris: Vec<String>) {
        if self.loading.get() {
            *self.pending_select.borrow_mut() = uris;
        } else {
            self.select_uris(&uris);
        }
    }

    pub fn select_all(&self) {
        match self.mode.get() {
            ViewMode::List | ViewMode::Columns => self.tree_view.selection().select_all(),
            _ => self.icon_view.select_all(),
        }
    }

    pub fn invert_selection(&self) {
        let selected: std::collections::HashSet<Vec<i32>> =
            self.selected_paths().iter().map(|p| p.indices()).collect();
        let mut paths = vec![];
        for it in self.filter.rows() {
            let p = self.filter.path(&it).unwrap();
            if !selected.contains(&p.indices()) {
                paths.push(p);
            }
        }
        self.icon_view.unselect_all();
        self.tree_view.selection().unselect_all();
        for p in &paths {
            match self.mode.get() {
                ViewMode::List | ViewMode::Columns => self.tree_view.selection().select_path(p),
                _ => self.icon_view.select_path(p),
            }
        }
    }

    /// Select items whose name matches a glob pattern like "*.jpg".
    pub fn select_pattern(&self, pattern: &str) {
        let pat: Vec<char> = pattern.to_lowercase().chars().collect();
        let mut paths = vec![];
        for it in self.filter.rows() {
            let name: String = self.filter.gv(&it, COL_NAME as i32);
            if util::glob_match(&pat, &name.to_lowercase().chars().collect::<Vec<_>>()) {
                paths.push(self.filter.path(&it).unwrap());
            }
        }
        self.icon_view.unselect_all();
        self.tree_view.selection().unselect_all();
        if !paths.is_empty() {
            self.select_filter_paths(&paths);
        }
    }

    /// Typing a name selects the first item starting with it; what's typed
    /// shows bottom right for 5 seconds after the last key (GTK's and
    /// Caja's search timeout). Backspace edits it, Up / Down go to the
    /// previous / next match, Escape ends it.
    /// Enter (without Ctrl and such) opens the selected items. The views'
    /// own binding opens the cursor item only: after selecting with the
    /// rubber band there's none (so nothing opened), and it needn't be one
    /// of those selected at all.
    fn enter_opens(self: &Rc<Self>, ev: &gdk::EventKey) -> bool {
        use gdk::keys::constants as k;
        let key = ev.keyval();
        let mods = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::MOD1_MASK | gdk::ModifierType::SUPER_MASK;
        if !(key == k::Return || key == k::KP_Enter || key == k::ISO_Enter) || ev.state().intersects(mods) || self.selected().is_empty() {
            return false;
        }
        self.end_typeahead();
        self.emit(Event::Activate);
        true
    }

    pub fn typeahead_key(self: &Rc<Self>, ev: &gdk::EventKey) -> glib::Propagation {
        use gdk::keys::constants as k;
        let mods = ev.state()
            & (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::MOD1_MASK | gdk::ModifierType::SUPER_MASK);
        if !mods.is_empty() {
            return glib::Propagation::Proceed;
        }
        let key = ev.keyval();
        if !self.typeahead.borrow().is_empty() {
            if key == k::Escape {
                self.end_typeahead();
                return glib::Propagation::Stop;
            }
            if key == k::BackSpace {
                self.typeahead.borrow_mut().pop();
                if self.typeahead.borrow().is_empty() {
                    self.end_typeahead();
                } else {
                    self.typeahead_changed();
                }
                return glib::Propagation::Stop;
            }
            if key == k::Up || key == k::Down || key == k::KP_Up || key == k::KP_Down {
                self.typeahead_step(key == k::Down || key == k::KP_Down);
                self.typeahead_timeout();
                return glib::Propagation::Stop;
            }
        }
        let Some(ch) = key.to_unicode() else {
            return glib::Propagation::Proceed;
        };
        if ch.is_control() || (ch == ' ' && self.typeahead.borrow().is_empty()) {
            return glib::Propagation::Proceed;
        }
        self.typeahead.borrow_mut().push(ch);
        self.typeahead_changed();
        glib::Propagation::Stop
    }

    /// Shows the typed text and selects the first match (the text turns
    /// red when nothing matches).
    fn typeahead_changed(self: &Rc<Self>) {
        let text = self.typeahead.borrow().clone();
        let e = &self.typeahead_entry;
        e.set_text(&text);
        e.set_position(-1);
        e.show();
        self.typeahead_timeout();
        match self.typeahead_match(None, true) {
            Some(p) => {
                self.select_filter_paths_scrolling(&[p], false);
                e.style_context().remove_class("error");
            }
            None => e.style_context().add_class("error"),
        }
    }

    /// The first row after `from` (or from the top) whose name starts
    /// with the typed text, going down or up.
    fn typeahead_match(&self, from: Option<gtk::TreePath>, down: bool) -> Option<gtk::TreePath> {
        let prefix = self.typeahead.borrow().to_lowercase();
        let n = self.filter.iter_n_children(None);
        let start = match &from {
            Some(p) => p.indices().first().copied().unwrap_or(0) + if down { 1 } else { -1 },
            None => 0,
        };
        let mut i = start;
        while (0..n).contains(&i) {
            let p = gtk::TreePath::from_indicesv(&[i]);
            if let Some(it) = self.filter.iter(&p) {
                let name: String = self.filter.gv(&it, COL_NAME as i32);
                if name.to_lowercase().starts_with(&prefix) {
                    return Some(p);
                }
            }
            i += if down { 1 } else { -1 };
        }
        None
    }

    /// Up / Down while typing: the previous / next match (staying put at
    /// the first / last one).
    fn typeahead_step(&self, down: bool) {
        let cur = self.selected_paths().into_iter().next();
        if let Some(p) = self.typeahead_match(cur, down) {
            self.select_filter_paths_scrolling(&[p], false);
        }
    }

    /// The wheel while typing: a match up or down for each notch (a
    /// touchpad's small steps are added up to whole ones).
    fn typeahead_scroll(self: &Rc<Self>, ev: &gdk::EventScroll) {
        let dy = match ev.direction() {
            gdk::ScrollDirection::Up => -1.0,
            gdk::ScrollDirection::Down => 1.0,
            gdk::ScrollDirection::Smooth => ev.delta().1,
            _ => 0.0,
        };
        let mut steps = self.typeahead_wheel.get() + dy;
        while steps.abs() >= 1.0 {
            self.typeahead_step(steps > 0.0);
            steps -= steps.signum();
        }
        self.typeahead_wheel.set(steps);
        self.typeahead_timeout();
    }

    fn typeahead_timeout(self: &Rc<Self>) {
        let g = self.typeahead_gen.get() + 1;
        self.typeahead_gen.set(g);
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            if let Some(p) = w.upgrade() {
                if p.typeahead_gen.get() == g {
                    p.end_typeahead();
                }
            }
        });
    }

    pub fn info_panel_on(&self) -> bool {
        self.info_on.get()
    }

    /// Shows or hides the info panel in this folder (remembered for it, or
    /// for every folder when folders don't keep their own settings).
    pub fn set_info_panel(&self, on: bool) {
        if config::with(|s| s.per_folder) {
            folders::set_info_panel(&self.current().uri(), on);
        } else {
            config::update(|s| s.info_panel = on);
        }
        self.info_on.set(on);
        self.update_info();
    }

    /// Shows the info panel again after its settings changed.
    pub fn refresh_info(&self) {
        self.info.reset();
        self.update_info();
    }

    /// What the info panel shows for the selection: the selected folder
    /// (its cover) or file (its picture or thumbnail); with nothing
    /// selected the open folder; with several items selected, what the
    /// settings say (a summary, by default).
    fn update_info(&self) {
        if !self.info_on.get() {
            self.info.widget.hide();
            return;
        }
        let (no_cover, several) = config::with(|s| (s.info_no_cover == "none", s.info_several.clone()));
        let missing = if no_cover { crate::info::Missing::Nothing } else { crate::info::Missing::Icon };
        let current = || (self.current(), self.folder_display_name());
        let sel = self.selected();
        self.show_info_widget_unless(sel.len() > 1 && several == "hide");
        match sel.as_slice() {
            [] => {
                let (f, name) = current();
                self.info.show_folder(&f, &name, missing);
            }
            [r] if r.is_dir => self.info.show_folder(&gio::File::for_uri(&r.uri), &r.name, missing),
            [r] => self.info.show_file(&gio::File::for_uri(&r.uri), &r.name),
            _ => match several.as_str() {
                "summary" => {
                    let items: Vec<(String, bool, u64)> = sel.iter().map(|r| (r.uri.clone(), r.is_dir, r.size)).collect();
                    self.info.show_summary(&items);
                }
                "current" => {
                    let (f, name) = current();
                    self.info.show_folder(&f, &name, missing);
                }
                // "last": the last item stays; "hide": hidden above.
                _ => {}
            },
        }
    }

    fn show_info_widget_unless(&self, hide: bool) {
        if hide {
            self.info.widget.hide();
        } else {
            self.show_info_widget();
        }
    }

    /// Makes the panel visible at its remembered width.
    fn show_info_widget(&self) {
        if self.info.widget.is_visible() {
            return;
        }
        self.info.widget.show();
        let paned = self.info_paned.clone();
        let place = move || {
            let total = paned.allocated_width();
            if total > 1 {
                paned.set_position((total - config::with(|s| s.info_panel_width)).max(100));
            }
            total > 1
        };
        if !place() {
            // Not laid out yet (a new tab): once it is.
            glib::idle_add_local_once(move || {
                place();
            });
        }
    }

    /// The column whose header's right edge (the divider for resizing it)
    /// is at `root` (screen coordinates).
    fn column_divider_at(&self, root: (f64, f64)) -> Option<gtk::TreeViewColumn> {
        let mut best: Option<(f64, gtk::TreeViewColumn)> = None;
        for (col, _, _) in &self.fit_cols {
            if !col.is_visible() {
                continue;
            }
            let Some(button) = col.button() else { continue };
            let Some(win) = button.window() else { continue };
            let (_, ox, oy) = win.origin();
            let a = button.allocation();
            let (x, y) = (root.0 - ox as f64, root.1 - oy as f64);
            if y < a.y() as f64 || y > (a.y() + a.height()) as f64 {
                continue;
            }
            let d = (x - (a.x() + a.width()) as f64).abs();
            if d <= 8.0 && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, col.clone()));
            }
        }
        best.map(|(_, c)| c)
    }

    /// Makes `col` exactly as wide as its widest text (and the header's),
    /// for every row in the folder, not just those in view.
    fn fit_column(&self, col: &gtk::TreeViewColumn) {
        let Some((_, id, icon)) = self.fit_cols.iter().find(|(c, _, _)| c == col) else { return };
        let tv = &self.tree_view;
        let text_width = |t: &str| tv.create_pango_layout(Some(t)).pixel_size().0;
        let mut width = 0;
        let m: &gtk::TreeModel = self.filter.upcast_ref();
        for it in self.filter.rows() {
            let text = if *id == "name" { m.gv::<String>(&it, COL_NAME as i32) } else { crate::details::text(id, m, &it) };
            width = width.max(text_width(&text));
        }
        // Cell padding, and the icon with its gap in the Name column.
        width += 12;
        if *icon {
            width += self.list_size.get() + 8;
        }
        // The header's title with room for the sort arrow.
        width = width.max(text_width(col.title().as_deref().unwrap_or("")) + 32);
        if col.min_width() > width {
            col.set_min_width(width);
        }
        col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        col.set_fixed_width(width);
    }

    // ------------------------------------------------------------ columns view

    /// Shows the list view's columns as chosen (Settings::list_columns), in
    /// that order after the name; in search results also each one's folder,
    /// and in the Trash when and from where each item was put there. Just
    /// the names in the columns view.
    pub fn apply_column_choice(&self) {
        let mut shown = config::with(|s| crate::details::known(&s.list_columns));
        if self.is_searching() {
            if !shown.contains(&"location") {
                shown.push("location");
            }
        } else if self.is_trash() {
            shown.extend(["trashed", "orig"]);
        }
        if self.columns_mode.get() {
            shown.clear();
        }
        self.placing_columns.set(true);
        let mut prev = self.name_col.clone();
        for id in &shown {
            if let Some((_, c)) = self.detail_cols.iter().find(|(i, _)| i == id) {
                self.tree_view.move_column_after(c, Some(&prev));
                c.set_visible(true);
                prev = c.clone();
            }
        }
        for (id, c) in &self.detail_cols {
            if !shown.contains(id) {
                c.set_visible(false);
            }
        }
        self.placing_columns.set(false);
    }

    /// The column headers' menu: which columns to show, and the chooser.
    fn column_menu(self: &Rc<Self>, event: &gdk::Event) {
        let menu = gtk::Menu::new();
        let chosen = config::with(|s| crate::details::known(&s.list_columns));
        for d in crate::details::choosable() {
            let item = gtk::CheckMenuItem::with_label(d.title);
            item.set_active(chosen.contains(&d.id));
            let w = Rc::downgrade(self);
            let id = d.id;
            item.connect_toggled(move |item| {
                let on = item.is_active();
                config::update(|s| {
                    s.list_columns.retain(|c| c != id);
                    if on {
                        s.list_columns.push(id.to_string());
                    }
                });
                if let Some(p) = w.upgrade() {
                    p.emit(Event::ColumnsChanged);
                }
            });
            menu.append(&item);
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        let more = gtk::MenuItem::with_mnemonic("_Visible Columns…");
        more.set_action_name(Some("win.visible-columns"));
        menu.append(&more);
        menu.show_all();
        menu.set_attach_widget(Some(&self.tree_view));
        menu.popup_at_pointer(Some(event));
    }

    /// Switches the list view's place between the list and the columns.
    fn apply_columns_layout(&self) {
        let on = self.mode.get() == ViewMode::Columns;
        self.columns_mode.set(on);
        // Just the names in the open folder's column.
        self.apply_column_choice();
        self.tree_view.set_headers_visible(!on);
        if on {
            // A width fitted in the list view doesn't fit a column.
            self.tree_view.set_fixed_height_mode(false);
            self.name_col.set_sizing(gtk::TreeViewColumnSizing::GrowOnly);
            self.name_col.set_fixed_width(-1);
            self.name_col.set_min_width(0);
        } else {
            self.name_col.set_min_width(200);
            // Rows are all as high (an icon of a set size, a line of text),
            // and the texts are ellipsized, so the columns are as wide as
            // their minimum or what the user made them: given that, GTK
            // measures the rows in view only, not every row of a folder.
            for (c, _, _) in &self.fit_cols {
                if c.sizing() != gtk::TreeViewColumnSizing::Fixed || c.fixed_width() <= 0 {
                    c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
                    c.set_fixed_width(c.fixed_width().max(c.min_width()));
                }
            }
            self.tree_view.set_fixed_height_mode(true);
        }
        self.sw_list.set_policy(if on { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic }, gtk::PolicyType::Automatic);
        self.sw_list.set_size_request(if on { crate::columns::WIDTH } else { -1 }, -1);
        crate::columns::style_column(&self.sw_list, on);
        self.strip.set_child_packing(&self.sw_list, !on, true, 0, gtk::PackType::Start);
        self.strip_sw.set_policy(if on { gtk::PolicyType::Automatic } else { gtk::PolicyType::Never }, gtk::PolicyType::Never);
        self.anc_box.set_visible(on);
        self.preview_box.set_visible(on);
        if on {
            self.rebuild_columns();
            self.update_column_preview();
        } else {
            for c in self.anc_box.children() {
                self.anc_box.remove(&c);
            }
            self.ancestors.borrow_mut().clear();
            self.clear_column_preview();
        }
        self.tree_view.queue_draw();
    }

    fn column_hooks(&self) -> Rc<crate::columns::Hooks> {
        let w = self.me.borrow().clone();
        let (w1, w2, w3) = (w.clone(), w.clone(), w);
        Rc::new(crate::columns::Hooks {
            click: Box::new(move |folder, uri| {
                if let Some(p) = w1.upgrade() {
                    p.column_clicked(folder, Some(uri), None);
                }
            }),
            context: Box::new(move |folder, uri, ev| {
                if let Some(p) = w2.upgrade() {
                    p.column_clicked(folder, uri, Some(std::ops::Deref::deref(ev).clone()));
                }
            }),
            drop: Box::new(move |uris, dest, action| {
                if let Some(p) = w3.upgrade() {
                    p.emit(Event::Drop { uris, dest, action });
                }
            }),
        })
    }

    /// A click in a side column: go to its folder with the item (if any)
    /// selected; for a right-click (`menu`) also open the context menu
    /// there, once the folder is loaded.
    fn column_clicked(self: &Rc<Self>, folder: &gio::File, uri: Option<&str>, menu: Option<gdk::Event>) {
        self.keep_columns.set(true);
        let uris: Vec<String> = uri.map(|u| vec![u.to_string()]).unwrap_or_default();
        let on_item = uri.is_some();
        if self.current().equal(folder) {
            self.select_uris(&uris);
            if uris.is_empty() {
                self.select_filter_paths(&[]);
            }
            if let Some(event) = menu {
                self.emit(Event::Context { event: Some(event), on_item });
            }
        } else {
            self.navigate(folder);
            *self.pending_select.borrow_mut() = uris;
            *self.pending_context.borrow_mut() = menu.map(|e| (e, on_item));
        }
        self.tree_view.grab_focus();
    }

    /// The columns of the folders from the start one down to the open
    /// one's parent, each showing the next one on the way as selected.
    fn rebuild_columns(&self) {
        let cur = self.current();
        let keep = self.keep_columns.replace(false);
        let root = match self.column_root.borrow().clone() {
            Some(r) if keep && (cur.equal(&r) || cur.has_prefix(&r)) => r,
            _ => cur.clone(),
        };
        *self.column_root.borrow_mut() = Some(root.clone());
        let mut chain = vec![];
        let mut f = cur.clone();
        while !f.equal(&root) {
            let Some(p) = f.parent() else { break };
            chain.push(p.clone());
            f = p;
        }
        chain.reverse();
        let old = std::mem::take(&mut *self.ancestors.borrow_mut());
        for c in self.anc_box.children() {
            self.anc_box.remove(&c);
        }
        let mut cols = vec![];
        for (i, folder) in chain.iter().enumerate() {
            // Columns already there stay (no reloading), the rest are made.
            let col = old.iter().find(|c| c.folder.equal(folder)).cloned().unwrap_or_else(|| {
                crate::columns::Column::new(folder, self.column_hooks(), self.show_hidden.get(), self.folders_first.get())
            });
            let next = chain.get(i + 1).unwrap_or(&cur);
            col.highlight(Some(&next.uri()));
            self.anc_box.pack_start(&col.widget, false, false, 0);
            cols.push(col);
        }
        self.anc_box.show_all();
        *self.ancestors.borrow_mut() = cols;
        self.scroll_columns_to_end();
    }

    fn clear_column_preview(&self) {
        for c in self.preview_slot.children() {
            self.preview_slot.remove(&c);
        }
        self.preview_col.borrow_mut().take();
        self.preview_info.widget.hide();
    }

    /// Right of the open folder: the selected folder's contents, or the
    /// selected file's preview (as in the info panel).
    fn update_column_preview(&self) {
        let sel = self.selected();
        match sel.as_slice() {
            [r] if r.is_dir => {
                let f = gio::File::for_uri(&r.uri);
                if self.preview_col.borrow().as_ref().is_some_and(|c| c.folder.equal(&f)) {
                    return;
                }
                self.clear_column_preview();
                let col = crate::columns::Column::new(&f, self.column_hooks(), self.show_hidden.get(), self.folders_first.get());
                self.preview_slot.pack_start(&col.widget, false, false, 0);
                self.preview_slot.show_all();
                *self.preview_col.borrow_mut() = Some(col);
            }
            [r] => {
                self.clear_column_preview();
                self.preview_info.widget.show();
                self.preview_info.show_file(&gio::File::for_uri(&r.uri), &r.name);
            }
            _ => self.clear_column_preview(),
        }
        self.scroll_columns_to_end();
    }

    /// Scrolls the strip of columns all the way right, following it for a
    /// moment while the new columns get their sizes.
    fn scroll_columns_to_end(&self) {
        // Not while the strip isn't in a shown window (a tab not shown
        // yet, or closed): GTK 3 lets a scrolled window that was taken out
        // of one keep handling its scrollbar's changes, and the scrollbar
        // it then looks for is gone (GTK warnings). Done once it shows.
        if !self.strip_sw.is_realized() {
            self.columns_end_on_map.set(true);
            return;
        }
        let adj = self.strip_sw.hadjustment();
        adj.set_value(adj.upper() - adj.page_size());
        self.columns_to_end.set(true);
        let to_end = self.columns_to_end.clone();
        glib::timeout_add_local_once(Duration::from_millis(400), move || to_end.set(false));
    }

    /// ← / → in the columns view: up to the folder on the left (with this
    /// one selected), or into the selected folder (its first item selected).
    fn columns_key(self: &Rc<Self>, ev: &gdk::EventKey) -> glib::Propagation {
        use gdk::keys::constants as k;
        let mods = ev.state() & (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::MOD1_MASK | gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::SUPER_MASK);
        if !self.columns_mode.get() || !mods.is_empty() {
            return glib::Propagation::Proceed;
        }
        let key = ev.keyval();
        if key == k::Right || key == k::KP_Right {
            if let [r] = self.selected().as_slice() {
                if r.is_dir {
                    self.keep_columns.set(true);
                    self.select_first.set(true);
                    self.navigate(&gio::File::for_uri(&r.uri));
                }
            }
            return glib::Propagation::Stop;
        }
        if key == k::Left || key == k::KP_Left {
            let cur = self.current();
            let at_start = self.column_root.borrow().as_ref().is_none_or(|r| r.equal(&cur));
            if let (false, Some(parent)) = (at_start, cur.parent()) {
                self.keep_columns.set(true);
                self.navigate(&parent);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    /// Whether typed text is being used to select an item right now.
    pub fn typeahead_active(&self) -> bool {
        !self.typeahead.borrow().is_empty()
    }

    fn end_typeahead(&self) {
        self.typeahead_gen.set(self.typeahead_gen.get() + 1);
        self.typeahead.borrow_mut().clear();
        self.typeahead_entry.hide();
        self.typeahead_entry.style_context().remove_class("error");
    }

    // --------------------------------------------------------------- view options

    pub fn set_mode(&self, mode: ViewMode) {
        self.close_rename_editor(false);
        let sel = self.selected_uris();
        self.mode.set(mode);
        match mode {
            ViewMode::List | ViewMode::Columns => self.stack.set_visible_child_name("list"),
            _ => {
                self.stack.set_visible_child_name("icons");
            }
        }
        self.attach_model();
        self.apply_columns_layout();
        self.apply_zoom();
        self.select_uris(&sel);
    }

    /// Gives the model to the view in use, and takes it from the other: a
    /// view that isn't shown would still measure its rows, and follow every
    /// row added, changed or sorted.
    fn attach_model(&self) {
        let list = matches!(self.mode.get(), ViewMode::List | ViewMode::Columns);
        if list {
            self.icon_view.set_model(None::<&gtk::TreeModel>);
            if self.tree_view.model().is_none() {
                self.tree_view.set_model(Some(&self.filter));
            }
        } else {
            self.tree_view.set_model(None::<&gtk::TreeModel>);
            if self.icon_view.model().is_none() {
                self.icon_view.set_model(Some(&self.filter));
            }
        }
    }

    fn detach_model(&self) {
        self.icon_view.set_model(None::<&gtk::TreeModel>);
        self.tree_view.set_model(None::<&gtk::TreeModel>);
    }

    pub fn zoom(&self, delta: i32) {
        config::update(|s| match self.mode.get() {
            ViewMode::Icon => s.icon_size = config::step(&s.icon_levels, s.icon_size, delta),
            ViewMode::List | ViewMode::Columns => s.list_size = config::step(&s.list_levels, s.list_size, delta),
            ViewMode::Compact => s.compact_size = config::step(&s.compact_levels, s.compact_size, delta),
        });
        self.apply_zoom();
    }

    pub fn zoom_reset(&self) {
        config::update(|s| match self.mode.get() {
            ViewMode::Icon => s.icon_size = config::snap(&s.icon_levels, config::NORMAL_ICON),
            ViewMode::List | ViewMode::Columns => s.list_size = config::snap(&s.list_levels, config::NORMAL_LIST),
            ViewMode::Compact => s.compact_size = config::snap(&s.compact_levels, config::NORMAL_COMPACT),
        });
        self.apply_zoom();
    }

    pub fn apply_zoom(&self) {
        let s = config::get();
        self.icon_size.set(s.icon_size);
        self.list_size.set(s.list_size);
        let iv = &self.icon_view;
        match self.mode.get() {
            ViewMode::Compact => {
                let size = s.compact_size;
                iv.set_item_orientation(gtk::Orientation::Horizontal);
                self.icon_pix.set_fixed_size(size, size);
                CellRendererExt::set_alignment(&self.icon_pix, 0.5, 0.5);
                CellRendererExt::set_alignment(&self.icon_text, 0.0, 0.5);
                self.icon_text.set_xalign(0.0);
                self.icon_text.set_property("alignment", pango::Alignment::Left);
                self.icon_text.set_property("wrap-width", -1);
                self.icon_text.set_property("ellipsize", pango::EllipsizeMode::End);
                self.icon_text.set_property("width-chars", 22);
                self.icon_text.set_property("max-width-chars", 22);
                iv.set_item_width(-1);
                iv.set_item_width(size + 200);
                iv.set_row_spacing(0);
                IconViewExt::set_margin(iv, 8);
                iv.set_item_padding(4);
                iv.set_column_spacing(4);
                iv.set_spacing(2);
                self.labels_shown.set(true);
                self.captions.borrow_mut().clear();
                self.image.set_tint(false);
                self.image.set_x_shift(0);
                self.label.set_x_shift(0);
                iv.style_context().remove_class("caja-like");
            }
            _ => {
                // Caja's icon view: sizes and spaces grow with the icons (48
                // px being its 100%), the font doesn't.
                let size = self.icon_size.get();
                let px = |v: f64| (v * size as f64 / 48.0).round() as i32;
                iv.set_item_orientation(gtk::Orientation::Vertical);
                CellRendererExt::set_alignment(&self.icon_pix, 0.5, 1.0);
                CellRendererExt::set_alignment(&self.icon_text, 0.5, 0.0);
                self.icon_text.set_xalign(0.5);
                self.icon_text.set_property("alignment", pango::Alignment::Center);
                self.icon_text.set_property("ellipsize", pango::EllipsizeMode::None);
                self.icon_text.set_property("width-chars", -1);
                self.icon_text.set_property("max-width-chars", -1);
                self.icon_text.set_property("wrap-mode", pango::WrapMode::WordChar);
                // Caja's label width: 135 (scaled), the fraction cut off.
                self.icon_text.set_property("wrap-width", (135.0 * size as f64 / 48.0).floor() as i32);
                // No names at the smallest size, a smaller font at the
                // next, and at most three lines, then "…" (Caja's default
                // text ellipsis limit).
                self.labels_shown.set(size > 16);
                self.label.set_style(3, if size <= 24 { -2 } else { 0 });
                // The text starts at the icon's bottom, 1 px in.
                self.icon_text.set_padding(self.icon_text.padding().0, 1);
                *self.captions.borrow_mut() = config::with(|s| crate::details::shown_captions(&s.icon_captions, size));
                self.thumb_size.set(px(64.0));
                self.image.set_tint(true);
                iv.style_context().add_class("caja-like");
                // Caja's icons start 8 px (scaled) in, a fraction cut off;
                // at 24 px (its 50 %) 6, as measured.
                let padding = px(4.0);
                let top = if size == 24 { 6 } else { (8.0 * size as f64 / 48.0).floor() as i32 };
                IconViewExt::set_margin(iv, top - padding);
                iv.set_item_padding(padding);
                iv.set_row_spacing(2);
                iv.set_spacing(0);
                self.fitted_width.set(0);
                self.fit_icon_columns();
            }
        }
        // Only if it changed: the list measures all its rows again.
        let ls = self.list_size.get();
        if self.list_pix.fixed_size() != (ls, ls) || self.list_pix.is_visible() != s.list_icons {
            self.list_pix.set_fixed_size(ls, ls);
            self.list_pix.set_visible(s.list_icons);
            for c in self.tree_view.columns() {
                c.queue_resize();
            }
        }
        self.icon_view.queue_resize();
        self.tree_view.queue_draw();
    }

    /// Where Caja counts an icon view item as hit: its picture and its
    /// name's text, as (x, y, width, height) in widget coordinates.
    fn item_boxes(&self, path: &gtk::TreePath) -> Vec<(f64, f64, f64, f64)> {
        let iv = &self.icon_view;
        let mut boxes = vec![];
        let (Some(model), true) = (iv.model(), iv.item_orientation() == gtk::Orientation::Vertical) else { return boxes };
        let Some(it) = model.iter(path) else { return boxes };
        if let Some(r) = cell_rect(iv, path, &self.icon_pix) {
            let (w, h) = icon_view_dims(icon_view_thumb(&model, &it).as_ref(), self.icon_size.get(), self.thumb_size.get());
            let left = r.x() + self.image.x_shift() + (r.width() - w) / 2;
            boxes.push((left as f64, (r.y() + r.height() - h) as f64, w as f64, h as f64));
        }
        if self.labels_shown.get() {
            if let Some(r) = cell_rect(iv, path, &self.icon_text) {
                let text = icon_cell_text(&model, &it, &self.captions.borrow(), self.icon_columns.get());
                boxes.extend(self.label.text_box(iv.upcast_ref(), &r, &text, iv.path_is_selected(path)));
            }
        }
        boxes
    }

    /// The icon view's item at (`x`, `y`) (widget coordinates), hit as
    /// Caja hits items: on its picture or its name's text, not the room
    /// around them, which GtkIconView counts as the item too.
    fn icon_hit(&self, x: i32, y: i32) -> Option<gtk::TreePath> {
        let iv = &self.icon_view;
        let (bx, by) = iv.convert_widget_to_bin_window_coords(x, y);
        let path = iv.path_at_pos(bx, by)?;
        if iv.item_orientation() != gtk::Orientation::Vertical {
            return Some(path);
        }
        let (x, y) = (x as f64, y as f64);
        self.item_boxes(&path).iter().any(|(l, t, w, h)| x >= *l && x < l + w && y >= *t && y < t + h).then_some(path)
    }

    /// While a rubber band is drawn in the icon view: GtkIconView selects
    /// the items whose room it touches, Caja those whose picture or name it
    /// does. The others are unselected again (unless they were before).
    fn trim_rubber_band(&self) {
        let iv = &self.icon_view;
        let Some((start, before)) = self.band.borrow().clone() else { return };
        let Some(pointer) = iv.display().default_seat().and_then(|s| s.pointer()) else { return };
        let Some(win) = iv.window() else { return };
        let (_, px, py, _) = win.device_position(&pointer);
        // The band's start is kept in the bin window's (scrolled) terms.
        let (ox, oy) = iv.convert_widget_to_bin_window_coords(0, 0);
        let (sx, sy) = ((start.0 - ox) as f64, (start.1 - oy) as f64);
        let (l, t, r, b) = (sx.min(px as f64), sy.min(py as f64), sx.max(px as f64), sy.max(py as f64));
        for path in iv.selected_items() {
            let touched = self.item_boxes(&path).iter().any(|(x, y, w, h)| *x < r && x + w > l && *y < b && y + h > t);
            if !touched && !before.contains(&path) {
                iv.unselect_path(&path);
            }
        }
    }

    /// A left click in the icon views (`path`: the item hit, see icon_hit;
    /// `over_item`: within an item's room all the same). What GTK would do
    /// differently from Caja is done here.
    fn icon_press(&self, ev: &gdk::EventButton, over_item: bool, path: Option<gtk::TreePath>) -> glib::Propagation {
        let iv = &self.icon_view;
        let state = ev.state();
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let single = ev.event_type() == gdk::EventType::ButtonPress;
        let Some(path) = path else {
            if over_item {
                // Beside an item's picture and name: empty space, as in Caja.
                if !shift && !ctrl {
                    iv.unselect_all();
                }
                self.ignore_items_once();
            }
            if single && iv.item_orientation() == gtk::Orientation::Vertical {
                let (x, y) = ev.position();
                let before = if ctrl || shift { iv.selected_items() } else { vec![] };
                *self.band.borrow_mut() = Some(((x as i32, y as i32), before));
            }
            return glib::Propagation::Proceed;
        };
        let index = path.indices().first().copied().unwrap_or(0);
        if !single {
            // A double click on one of several selected items opens them all:
            // the first click's release left just it selected.
            let taken = self.released_keep.borrow_mut().take();
            if let Some((keep, clicked)) = taken {
                if clicked == path {
                    for k in &keep {
                        iv.select_path(k);
                    }
                }
            }
            return glib::Propagation::Proceed;
        }
        self.released_keep.borrow_mut().take();
        if shift {
            // From the anchor to this item in their order, running on across
            // the ends of rows (GTK selects the rectangle between them).
            let cursor = iv.cursor().and_then(|(p, _)| p.indices().first().copied());
            let anchor = self.range_anchor.get().or(cursor).unwrap_or(index);
            self.range_anchor.set(Some(anchor));
            if !ctrl {
                iv.unselect_all();
            }
            for i in anchor.min(index)..=anchor.max(index) {
                iv.select_path(&gtk::TreePath::from_indicesv(&[i]));
            }
            iv.set_cursor(&path, None::<&gtk::CellRenderer>, false);
            iv.grab_focus();
            return glib::Propagation::Stop;
        }
        self.range_anchor.set(Some(index));
        if !ctrl && iv.path_is_selected(&path) && iv.selected_items().len() > 1 {
            // GTK selects just this item on the press: keep all of them for
            // a drag (the release narrows it down if none comes).
            let keep = iv.selected_items();
            *self.press_keep.borrow_mut() = Some((keep, path));
            let w = self.me.borrow().clone();
            glib::idle_add_local_full(glib::Priority::HIGH, move || {
                if let Some(p) = w.upgrade() {
                    if let Some((keep, _)) = p.press_keep.borrow().as_ref() {
                        for k in keep {
                            p.icon_view.select_path(k);
                        }
                    }
                }
                glib::ControlFlow::Break
            });
        }
        glib::Propagation::Proceed
    }

    /// GtkIconView finds the item clicked by its cells: hidden while it
    /// looks, the click is on empty space to it (deselecting, starting a
    /// rubber band), shown again before anything is drawn.
    fn ignore_items_once(&self) {
        self.hit_off.set(true);
        let w = self.me.borrow().clone();
        glib::idle_add_local_full(glib::Priority::HIGH, move || {
            if let Some(p) = w.upgrade() {
                p.hit_off.set(false);
            }
            glib::ControlFlow::Break
        });
    }

    /// Caja's columns in the icon view: as many as fit at 155 px (with 48
    /// px icons), sharing the width out evenly (caja-icon-container.c).
    fn fit_icon_columns(&self) {
        let iv = &self.icon_view;
        let width = iv.allocated_width();
        let sw = iv.parent().and_downcast::<gtk::ScrolledWindow>();
        let full = sw.as_ref().map_or(width, |sw| sw.allocated_width());
        // Fitted again when the place changes size, not when the scrollbar
        // comes or goes: the columns would decide that, and back.
        if iv.item_orientation() != gtk::Orientation::Vertical || width <= 1 || full == self.fitted_width.get() {
            return;
        }
        self.fitted_width.set(full);
        // As caja-icon-container.c: columns of at least 155 (at 48 px icons)
        // in the width (less 6 px), each a unit narrower than its share.
        let scale = self.icon_size.get() as f64 / 48.0;
        // The width as Caja has it: its scrollbar takes 12 px, GTK's
        // non-overlay one a few more.
        let caja_width = if full > width { full - 12 } else { width };
        let usable = (caja_width - 6) as f64;
        let columns = (usable / (155.0 * scale)).floor().max(1.0);
        let pitch = (usable / columns - scale).round() as i32;
        self.icon_columns.set(columns as i32);
        // GtkIconView also wants its margin on the right: the column
        // spacing makes up for what's missing (it isn't counted after the
        // last column), the cells being drawn shifted by half of it to stay
        // in the middle of their column. They have to fit beside a
        // scrollbar too, should one come.
        let scrollbar = sw.and_then(|sw| sw.vscrollbar()).map_or(0, |b| b.preferred_width().1);
        let narrowest = if full > width { width } else { width - scrollbar };
        let margin = IconViewExt::margin(iv);
        let spacing = (columns as i32 * pitch + 2 * margin - narrowest).max(0);
        let item_width = (pitch - 2 * iv.item_padding() - spacing).max(1);
        // Caja's own rounding puts its columns 1 px further right, except
        // at 24 px, where the top margin is set apart (see apply_zoom) and
        // the left one mustn't follow.
        let size = self.icon_size.get();
        let shift = if size == 24 { spacing / 2 - (margin - ((8.0 * scale).floor() as i32 - iv.item_padding())) } else { spacing / 2 + 1 };
        self.label.set_x_shift(shift);
        self.image.set_x_shift(shift);
        if iv.column_spacing() != spacing {
            iv.set_column_spacing(spacing);
        }
        if iv.item_width() != item_width {
            iv.set_item_width(item_width);
        }
    }

    /// Draws the selected items' names over the icon view, whole, as Caja
    /// does (their own cells leave them out), and the keyboard's focus.
    fn draw_whole_labels(&self, cr: &gtk::cairo::Context) {
        let iv = &self.icon_view;
        if iv.item_orientation() != gtk::Orientation::Vertical || !self.labels_shown.get() {
            return;
        }
        let Some(model) = iv.model() else { return };
        // The items in sight (where this draws), found without going
        // through all of them (as gtk_icon_view_get_visible_range does).
        // Also the rows above: a whole name hangs down over the rows below
        // its own, and when only those are drawn again (an item under the
        // pointer, a new thumbnail), the part over them must be too. A name
        // longer than the view is high doesn't happen.
        let (y1, y2) = cr.clip_extents().map_or((0.0, 0.0), |(_, y1, _, y2)| (y1, y2));
        let reach = iv.allocated_height() as f64;
        let range = crate::iconview::IconView::items_between(iv, y1 - reach, y2)
            .map(|(a, b)| (gtk::TreePath::from_indicesv(&[a]), gtk::TreePath::from_indicesv(&[b])))
            .or_else(|| iv.visible_range());
        let Some((mut path, last)) = range else { return };
        let editing = self.edit_hidden.borrow().clone();
        let cursor = iv.cursor().filter(|_| self.keyboard_focus.get() && iv.has_focus()).map(|(p, _)| p);
        let drop = self.drop_path.borrow().clone();
        loop {
            let index = path.indices().first().copied().unwrap_or(-1);
            let selected = crate::iconview::IconView::is_selected(iv, index) || drop.as_ref() == Some(&path);
            let focused = !selected && cursor.as_ref() == Some(&path);
            if selected || focused {
                if let Some(it) = model.iter(&path) {
                    let uri: String = model.gv(&it, COL_URI as i32);
                    if editing.as_deref() != Some(uri.as_str()) {
                        if let Some(rect) = cell_rect(iv, &path, &self.icon_text) {
                            let text = icon_label_text(&model, &it, &self.captions.borrow());
                            if selected {
                                self.label.draw_selected(iv.upcast_ref(), cr, &rect, &text);
                            } else {
                                self.label.draw_focus(iv.upcast_ref(), cr, &rect, &text);
                            }
                        }
                    }
                }
            }
            if path >= last {
                break;
            }
            path.next();
        }
    }

    pub fn set_show_hidden(&self, show: bool) {
        if self.show_hidden.get() == show {
            return;
        }
        self.show_hidden.set(show);
        self.refilter();
        self.emit(Event::Selection);
    }

    /// Refilter with the views detached, keeping the selection. Refiltering
    /// under an attached GtkTreeView intermittently trips
    /// gtk_tree_model_filter_real_unref_node's ref-count assertion.
    fn refilter(&self) {
        let sel = self.selected_uris();
        self.detach_model();
        self.filter.refilter();
        self.attach_model();
        self.select_uris(&sel);
    }

    pub fn set_folders_first(&self, v: bool) {
        self.folders_first.set(v);
        let (n, d) = self.sort_state();
        self.apply_sort(&n, d);
    }

    pub fn apply_click_policy(&self) {
        let single = config::with(|s| s.single_click);
        self.icon_view.set_activate_on_single_click(single);
        self.tree_view.set_activate_on_single_click(single);
    }

    /// With per-folder views on, switch to this folder's remembered view and
    /// sorting, or to the defaults from Preferences if it has none.
    pub fn apply_folder_settings(&self) {
        let s = config::get();
        let fv = folders::get(&self.current().uri());
        // The info panel: per folder, or the same everywhere.
        let info = if s.per_folder { fv.info_panel.unwrap_or(false) } else { s.info_panel };
        self.info_on.set(info);
        self.info.reset();
        self.update_info();
        if !s.per_folder {
            return;
        }
        let mode = ViewMode::from_str(fv.view.as_deref().unwrap_or(&s.default_view));
        // Moving through the columns stays in the columns view (as in
        // Finder), whatever the folders' own views are.
        let in_columns = self.keep_columns.get() && self.mode.get() == ViewMode::Columns;
        if mode != self.mode.get() && !in_columns {
            self.set_mode(mode);
        }
        let (by, desc) = fv.sort.unwrap_or((s.sort_by, s.sort_desc));
        if (by.clone(), desc) != self.sort_state() {
            self.apply_sort(&by, desc);
        }
    }

    /// Reset View to Defaults, as in Caja: forgets this folder's view,
    /// sorting and info panel and shows it as Preferences say new folders
    /// are shown.
    pub fn reset_view(&self) {
        folders::forget(&self.current().uri());
        let s = config::get();
        let mode = ViewMode::from_str(&s.default_view);
        if mode != self.mode.get() {
            self.set_mode(mode);
        }
        self.apply_sort(&s.sort_by, s.sort_desc);
        if s.per_folder {
            self.info_on.set(false);
            self.info.reset();
            self.update_info();
        }
    }

    pub fn sort_state(&self) -> (String, bool) {
        match self.store.sort_column_id() {
            Some((gtk::SortColumn::Index(i), t)) => (crate::details::sort_name(i).to_string(), t == gtk::SortType::Descending),
            _ => ("name".into(), false),
        }
    }

    pub fn apply_sort(&self, by: &str, desc: bool) {
        let id = crate::details::sort_id(by);
        let order = if desc { gtk::SortType::Descending } else { gtk::SortType::Ascending };
        // Force a resort even when unchanged (e.g. folders-first toggled).
        self.store.set_sort_column_id(gtk::SortColumn::Index((id + 1) % crate::details::sort_count()), order);
        self.store.set_sort_column_id(gtk::SortColumn::Index(id), order);
        for (cid, col) in &self.sort_cols {
            col.set_sort_indicator(*cid == id);
            if *cid == id {
                col.set_sort_order(order);
            }
        }
    }

    // ------------------------------------------------------------ search

    pub fn is_searching(&self) -> bool {
        self.search.borrow().is_some()
    }

    /// The text of the search whose results are shown.
    pub fn search_text(&self) -> Option<String> {
        self.search.borrow().as_ref().map(|q| q.text.clone())
    }

    /// Searches for `text` (from the search bar) in the open folder and
    /// everything below it, as Caja does: the results are a place of their
    /// own (Back returns to the folder), with the query editor above them.
    pub fn start_search(self: &Rc<Self>, text: &str) {
        let query = crate::search::Query::new(text, &self.current());
        self.back.borrow_mut().push(self.place());
        self.fwd.borrow_mut().clear();
        *self.search.borrow_mut() = Some(query);
        self.load();
    }

    /// The search's text as now typed in the search bar: the title follows
    /// it, the results only once searched again (Enter, Reload), as in Caja.
    pub fn set_search_text(&self, text: &str) {
        if let Some(q) = self.search.borrow_mut().as_mut() {
            q.text = text.to_string();
        }
        if self.is_searching() {
            self.tab_label.set_text(&self.title());
        }
    }

    /// Shows `q`'s results, found in a thread (see search::run) and added
    /// as they come. Hidden items are left out whatever's shown elsewhere,
    /// as in Caja.
    fn run_search(self: &Rc<Self>, gen: u64, q: &crate::search::Query) {
        use std::sync::atomic::Ordering;
        self.search_gen.store(gen, Ordering::SeqCst);
        self.query_editor.set_query(q, &self.current());
        self.query_editor.widget.show();
        self.clear_rows();
        self.loading.set(true);
        *self.free_space.borrow_mut() = None;
        self.apply_column_choice();
        self.tab_label.set_text(&self.title());
        self.tab_label.set_tooltip_text(Some(&self.title()));
        if self.columns_mode.get() {
            self.rebuild_columns();
        }
        self.emit(Event::Location);
        self.emit(Event::Loading(true));
        let params = q.params();
        let token = self.search_gen.clone();
        let (tx, rx) = async_channel::unbounded::<Vec<(String, String)>>();
        std::thread::spawn(move || {
            crate::search::run(&params, || token.load(Ordering::SeqCst) == gen, |found| tx.send_blocking(found).is_ok());
        });
        let w = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            // As a folder loads (see load): the first results at once, the
            // rest a second's worth at a time, as each addition makes the
            // icon view measure all its items again.
            let mut waiting = vec![];
            let mut shown = std::time::Instant::now();
            loop {
                let found = rx.recv().await;
                let done = found.is_err();
                if let Ok(found) = found {
                    waiting.extend(read_infos(found).await);
                }
                let Some(p) = w.upgrade() else { return };
                if p.gen.get() != gen {
                    return;
                }
                if !waiting.is_empty() && (done || p.store.iter_n_children(None) == 0 || shown.elapsed() >= Duration::from_secs(1)) {
                    p.add_rows(std::mem::take(&mut waiting), false);
                    shown = std::time::Instant::now();
                    // The count in the status bar follows.
                    p.emit(Event::Selection);
                }
                if done {
                    break;
                }
            }
            if let Some(p) = w.upgrade() {
                if p.gen.get() == gen {
                    p.loading.set(false);
                    p.count_dir_items(gen);
                    p.emit(Event::Loading(false));
                    p.emit(Event::Selection);
                }
            }
        });
    }

    /// Opens the folder a search result is in, with it selected (Caja's
    /// "Open parent location").
    pub fn open_parent_location(self: &Rc<Self>, uri: &str) {
        let f = gio::File::for_uri(uri);
        let Some(parent) = f.parent() else { return };
        self.navigate(&parent);
        *self.pending_select.borrow_mut() = vec![uri.to_string()];
    }

    // --------------------------------------------------------------- rename

    pub fn start_rename(&self, uri: &str) {
        let it = self.index.borrow().get(uri).cloned();
        let Some(it) = it else {
            *self.pending_rename.borrow_mut() = Some(uri.to_string());
            return;
        };
        let Some(fit) = self.filter.convert_child_iter_to_iter(&it) else { return };
        let path = self.filter.path(&fit).unwrap();
        self.select_filter_paths(std::slice::from_ref(&path));
        *self.editing.borrow_mut() = Some(uri.to_string());
        match self.mode.get() {
            ViewMode::List | ViewMode::Columns => {
                self.list_text.set_editable(true);
                self.tree_view.grab_focus();
                self.tree_view
                    .set_cursor_on_cell(&path, Some(&self.name_col), Some(&self.list_text), true);
            }
            _ => {
                self.icon_view.grab_focus();
                // GtkIconView lays out new items in an idle callback: the
                // editor goes over the item once it has its place.
                let w = self.me.borrow().clone();
                let uri = uri.to_string();
                glib::idle_add_local_full(glib::Priority::LOW, move || {
                    if let Some(p) = w.upgrade() {
                        p.open_rename_editor(&uri);
                    }
                    glib::ControlFlow::Break
                });
            }
        }
    }

    /// Renaming in the icon and compact views, like Nemo: a multi-line,
    /// wrapping field just as tall as its text, over the item's name. (The
    /// icon view's own editing can only use a one-line entry, which it
    /// stretches over the whole label area.) Enter or clicking elsewhere
    /// renames, Escape cancels.
    fn open_rename_editor(&self, uri: &str) {
        self.rename_tries.set(0);
        self.close_rename_editor(false);
        let Some(row) = self.index.borrow().get(uri).cloned().map(|it| self.row_info(self.store.upcast_ref(), &it)) else {
            self.editing.borrow_mut().take();
            return;
        };
        let view = gtk::TextView::new();
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        let compact = self.mode.get() == ViewMode::Compact;
        view.set_justification(if compact { gtk::Justification::Left } else { gtk::Justification::Center });
        view.set_accepts_tab(false);
        view.set_left_margin(2);
        view.set_right_margin(2);
        if let Some(b) = view.buffer() { b.set_text(&row.name) }
        let frame = gtk::Frame::new(None);
        frame.style_context().add_class("arkha-rename");
        frame.style_context().add_provider(&rename_css(), gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        view.style_context().add_provider(&rename_css(), gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        frame.add(&view);
        let ed = Rc::new(RenameEditor { frame: frame.clone(), view: view.clone(), uri: uri.to_string(), name: row.name.clone() });
        let this_editor = Rc::downgrade(&ed);
        *self.rename_editor.borrow_mut() = Some(ed);
        // The item's own name would show around the editor.
        *self.edit_hidden.borrow_mut() = Some(uri.to_string());
        self.icon_view.queue_draw();
        self.overlay.add_overlay(&frame);
        frame.show_all();
        view.grab_focus();
        // The name without its extension selected, as everywhere.
        if let Some(b) = view.buffer() {
            let (stem, _) = if row.is_dir { (row.name.as_str(), "") } else { util::split_ext(&row.name) };
            b.select_range(&b.iter_at_offset(stem.chars().count() as i32), &b.start_iter());
        }
        let w = self.me.borrow().clone();
        view.connect_key_press_event(move |_, ev| {
            use gdk::keys::constants as k;
            let Some(p) = w.upgrade() else { return glib::Propagation::Proceed };
            let key = ev.keyval();
            if key == k::Escape {
                p.close_rename_editor(false);
                glib::Propagation::Stop
            } else if (key == k::Return || key == k::KP_Enter) && !ev.state().contains(gdk::ModifierType::SHIFT_MASK) {
                p.close_rename_editor(true);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        let w = self.me.borrow().clone();
        view.connect_focus_out_event(move |_, _| {
            // Once the focus has moved: taken out of the window while GTK
            // is moving the focus away from it, GtkWindow goes on with the
            // field already gone (g_object_ref warnings).
            let (w, ed) = (w.clone(), this_editor.clone());
            glib::idle_add_local_once(move || {
                if let (Some(p), Some(ed)) = (w.upgrade(), ed.upgrade()) {
                    if p.rename_editor.borrow().as_ref().is_some_and(|cur| Rc::ptr_eq(cur, &ed)) {
                        p.close_rename_editor(true);
                    }
                }
            });
            glib::Propagation::Proceed
        });
        // As the text wraps onto more lines, the editor grows.
        if let Some(b) = view.buffer() {
            let ov = self.overlay.clone();
            b.connect_changed(move |_| ov.queue_resize());
        }
    }

    /// Ends renaming in the icon views; with `commit` the item gets the
    /// typed name.
    fn close_rename_editor(&self, commit: bool) {
        let Some(ed) = self.rename_editor.borrow_mut().take() else { return };
        let text = ed
            .view
            .buffer()
            .and_then(|b| b.text(&b.start_iter(), &b.end_iter(), false))
            .map(|t| t.replace(['\n', '\r'], ""))
            .unwrap_or_default();
        // The view gets the focus back, unless it went elsewhere (a click
        // on the location bar, say: the field then lost it).
        let window = ed.view.toplevel().and_downcast::<gtk::Window>();
        let focus_here = window.and_then(|w| gtk::prelude::GtkWindowExt::focused_widget(&w)).is_none_or(|f| &f == ed.view.upcast_ref::<gtk::Widget>());
        // (Gone already with the window, if that was closed.)
        if ed.frame.parent().as_ref() == Some(self.overlay.upcast_ref()) {
            self.overlay.remove(&ed.frame);
        }
        self.editing.borrow_mut().take();
        self.show_edited_name();
        if focus_here {
            self.icon_view.grab_focus();
        }
        if commit && !text.is_empty() && text != ed.name {
            self.emit(Event::Rename { uri: ed.uri.clone(), new_name: text });
        }
    }

    /// Where the rename editor goes: over the item's name, as wide as the
    /// name area (at least a bit wider than a short name) and as tall as
    /// its text.
    fn rename_rect(&self, ed: &RenameEditor) -> Option<gdk::Rectangle> {
        let it = self.index.borrow().get(&ed.uri).cloned()?;
        let fit = self.filter.convert_child_iter_to_iter(&it)?;
        let path = self.filter.path(&fit)?;
        let r = cell_rect(&self.icon_view, &path, &self.icon_text)?;
        let (x, y) = self.icon_view.translate_coordinates(&self.overlay, r.x(), r.y())?;
        let width = r.width().max(90);
        // The wrapped text's height, measured like the view draws it (the
        // view itself only knows once it has been laid out at this width),
        // plus the frame and margins.
        let text = ed.view.buffer().and_then(|b| b.text(&b.start_iter(), &b.end_iter(), false)).map(|t| t.to_string()).unwrap_or_default();
        let layout = ed.view.create_pango_layout(Some(if text.is_empty() { " " } else { &text }));
        layout.set_width((width - 8) * pango::SCALE);
        layout.set_wrap(pango::WrapMode::WordChar);
        let (_, text_height) = layout.pixel_size();
        let (_, frame_extra) = ed.frame.preferred_height();
        let (_, view_line) = ed.view.preferred_height();
        let height = text_height + (frame_extra - view_line).max(2);
        Some(gdk::Rectangle::new(x - (width - r.width()) / 2, y, width, height))
    }

    /// After renaming in the icon view: the item draws its name again, and
    /// is measured again (it was laid out without its name meanwhile).
    fn show_edited_name(&self) {
        let Some(uri) = self.edit_hidden.borrow_mut().take() else { return };
        let it = self.index.borrow().get(&uri).cloned();
        if let Some(it) = it {
            if let Some(path) = self.store.path(&it) {
                self.store.row_changed(&path, &it);
            }
        }
        self.icon_view.queue_draw();
    }

    pub fn rename_later(&self, uri: String) {
        if self.index.borrow().contains_key(&uri) {
            self.start_rename(&uri);
        } else {
            *self.pending_rename.borrow_mut() = Some(uri);
        }
    }

    // --------------------------------------------------------------- DnD helpers

    /// Folder under the pointer that can accept a drop: (filter path, uri).
    fn drop_target(&self, x: i32, y: i32) -> Option<(gtk::TreePath, String)> {
        let path = match self.mode.get() {
            ViewMode::List | ViewMode::Columns => self.tree_view.dest_row_at_pos(x, y).and_then(|(p, _)| p),
            _ => self.icon_hit(x, y),
        }?;
        let r = self.row_at_filter_path(&path)?;
        if !r.is_dir {
            return None;
        }
        // Can't drop a selected (dragged) folder onto itself.
        if DRAG_SRC_DIR.with(|d| d.borrow().is_some()) {
            let selected = match self.mode.get() {
                ViewMode::List | ViewMode::Columns => self.tree_view.selection().path_is_selected(&path),
                _ => self.icon_view.path_is_selected(&path),
            };
            if selected {
                return None;
            }
        }
        Some((path, r.uri))
    }

    fn highlight_drop(&self, path: Option<&gtk::TreePath>) {
        match self.mode.get() {
            ViewMode::List | ViewMode::Columns => self
                .tree_view
                .set_drag_dest_row(path, gtk::TreeViewDropPosition::IntoOrBefore),
            _ if self.icon_view.item_orientation() == gtk::Orientation::Vertical => {
                // As Caja: the folder looks selected (see draw_whole_labels).
                if self.drop_path.borrow().as_ref() != path {
                    *self.drop_path.borrow_mut() = path.cloned();
                    self.icon_view.queue_draw();
                }
            }
            _ => self
                .icon_view
                .set_drag_dest_item(path, gtk::IconViewDropPosition::DropInto),
        }
    }

    // --------------------------------------------------------------- status

    pub fn status_text(&self) -> String {
        let sel = self.selected();
        let total = self.store.iter_n_children(None) as usize;
        let visible = self.filter.iter_n_children(None) as usize;
        // Only shown without a filter, when the hidden ones are what's left out.
        let hidden = total - visible;
        let free = self
            .free_space
            .borrow()
            .map(|f| format!(", Free space: {}", util::format_size(f)))
            .unwrap_or_default();
        if self.loading.get() && sel.is_empty() {
            return format!("Loading… {visible} items");
        }
        match sel.len() {
            0 => {
                let mut s = format!("{visible} item{}", if visible == 1 { "" } else { "s" });
                if !self.show_hidden.get() && hidden > 0 && config::with(|s| s.status_hidden) {
                    s.push_str(&format!(" ({hidden} hidden)"));
                }
                s + &free
            }
            1 => {
                let r = &sel[0];
                if r.is_dir {
                    let n = self.index.borrow().get(&r.uri).map(|it| {
                        self.store.gv::<String>(it, COL_SIZE_TEXT as i32)
                    });
                    match n {
                        Some(t) if !t.is_empty() => format!("“{}” selected (containing {t})", r.name),
                        _ => format!("“{}” selected", r.name),
                    }
                } else {
                    format!("“{}” selected ({})", r.name, util::format_size(r.size))
                }
            }
            n => {
                let dirs = sel.iter().filter(|r| r.is_dir).count();
                let files = n - dirs;
                let bytes: u64 = sel.iter().filter(|r| !r.is_dir).map(|r| r.size).sum();
                match (dirs, files) {
                    (0, f) => format!("{f} items selected ({})", util::format_size(bytes)),
                    (d, 0) => format!("{d} folders selected"),
                    (d, f) => format!(
                        "{d} folder{} selected, {f} other item{} selected ({})",
                        if d == 1 { "" } else { "s" },
                        if f == 1 { "" } else { "s" },
                        util::format_size(bytes)
                    ),
                }
            }
        }
    }

    pub fn redraw(&self) {
        self.icon_view.queue_draw();
        self.tree_view.queue_draw();
    }

    /// Called before the pane is taken out of its window (while its views
    /// are still in it: see detach_model there), and nothing it was still
    /// to do is done after.
    pub fn shutdown(&self) {
        if self.closed.replace(true) {
            return;
        }
        // While everything is still there: an icon view not in the window
        // (in the list view) is disposed before it's destroyed, and its
        // destroy then lets go of the model with its cells already gone
        // (the selection-changed handlers ask for them). Also, GTK 3 lets a
        // scrolled window taken out of its window keep handling its
        // scrollbars' changes, for scrollbars it no longer has (warnings):
        // letting go of the models scrolls the views back to the top.
        *self.pending_scroll.borrow_mut() = None;
        self.detach_model();
        util::release_memory_soon();
        self.gen.set(self.gen.get() + 1);
        let taken = self.monitor.borrow_mut().take();
        if let Some(m) = taken {
            m.cancel();
        }
        if let Some(id) = self.thumb_listener.take() {
            icons::remove_thumbnail_listener(id);
        }
    }
}



/// A filter model on `child`. Not gtk::TreeModelFilter::new: in gtk-rs 0.18
/// it takes a reference on top of the one gtk_tree_model_filter_new hands
/// over (as for a floating widget, which a model isn't), so the filter,
/// its list and all its rows were never freed: every closed tab stayed in
/// memory. g_object_new hands over its reference properly.
fn new_filter(child: &impl IsA<gtk::TreeModel>) -> gtk::TreeModelFilter {
    glib::Object::builder().property("child-model", child.as_ref()).build()
}

/// Where `cell` of the item at `path` is, once the item has been laid out.
/// Before, GTK 3.24's gtk_icon_view_get_cell_rect looks the cell up in a
/// row the item isn't in yet and crashes (right after a new folder is
/// added, say). Without a cell it only reads the item's own area, which
/// until then is -1 wide (plus the padding on both sides).
fn cell_rect(iv: &gtk::IconView, path: &gtk::TreePath, cell: &impl IsA<gtk::CellRenderer>) -> Option<gdk::Rectangle> {
    let item = iv.cell_rect(path, None::<&gtk::CellRenderer>)?;
    if item.width() <= 2 * iv.item_padding() {
        return None;
    }
    iv.cell_rect(path, Some(cell))
}

/// When an item in the Trash was put there (0 elsewhere): its
/// trash::deletion-date is the local time without a zone, "2026-11-11T13:05:00".
fn trash_time(info: &gio::FileInfo) -> i64 {
    info.attribute_string("trash::deletion-date")
        .and_then(|d| glib::DateTime::from_iso8601(&d, Some(&glib::TimeZone::local())).ok())
        .map_or(0, |d| d.to_unix())
}

fn count_entries(path: &std::path::Path, hidden: bool) -> Option<u64> {
    let rd = std::fs::read_dir(path).ok()?;
    Some(
        rd.filter_map(|e| e.ok())
            .filter(|e| hidden || !util::name_hidden(&e.file_name().to_string_lossy()))
            .count() as u64,
    )
}

/// A thumbnail debug message, once per file (rows are drawn over and over).
fn debug_once(uri: &str, what: &str) {
    thread_local! {
        static TOLD: RefCell<std::collections::HashSet<String>> = RefCell::new(Default::default());
    }
    // Only kept while debug messages are shown (G_MESSAGES_DEBUG).
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = ON.get_or_init(|| std::env::var("G_MESSAGES_DEBUG").is_ok_and(|v| v.split([',', ' ']).any(|d| d == "all" || d == "arkha")));
    if !on {
        return;
    }
    if TOLD.with(|t| t.borrow_mut().insert(uri.to_string())) {
        glib::g_debug!("arkha", "thumbnail: {what} ({uri})");
    }
}

/// Asks for the thumbnail of a row being shown without one: made for what
/// is seen, the latest first, as Nemo does, not for a whole big folder in
/// the order it's listed.
fn request_row_thumbnail(m: &gtk::TreeModel, it: &gtk::TreeIter) {
    if m.gv::<bool>(it, COL_IS_DIR as i32) || !config::with(|s| s.thumbnails) {
        return;
    }
    let ctype = m.value(it, COL_CTYPE as i32);
    let ctype = value_str(&ctype);
    let uri: String = m.gv(it, COL_URI as i32);
    if !icons::can_thumbnail(ctype) {
        debug_once(&uri, &format!("no thumbnailer for {ctype}"));
        return;
    }
    if let Some(path) = gio::File::for_uri(&uri).path() {
        icons::request_thumbnail(&uri, &path, m.gv(it, COL_MTIME as i32), m.gv(it, COL_SIZE as i32), ctype);
    }
}

/// Tells icons.rs when `view` draws, so the thumbnails it shows get loaded.
fn mark_drawing(view: &gtk::Widget) {
    view.connect_draw(|_, _| {
        icons::set_drawing(true);
        glib::Propagation::Proceed
    });
    view.connect_local("draw", true, |_| {
        icons::set_drawing(false);
        Some(false.to_value())
    });
}

/// The details of items found, (folder URI, name) pairs, read all at once
/// (one after another, each would wait for the last).
async fn read_infos(found: Vec<(String, String)>) -> Vec<(gio::File, gio::FileInfo)> {
    let out = Rc::new(RefCell::new(Vec::with_capacity(found.len())));
    let left = Rc::new(Cell::new(found.len()));
    let (tx, rx) = async_channel::bounded::<()>(1);
    for (parent, name) in found {
        let parent = gio::File::for_uri(&parent);
        let (out, left, tx) = (out.clone(), left.clone(), tx.clone());
        parent.child(&name).query_info_async(ATTRS, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT, gio::Cancellable::NONE, move |res| {
            if let Ok(info) = res {
                out.borrow_mut().push((parent, info));
            }
            left.set(left.get() - 1);
            if left.get() == 0 {
                let _ = tx.try_send(());
            }
        });
    }
    if left.get() > 0 {
        let _ = rx.recv().await;
    }
    out.take()
}

/// What an icon view's name cell shows: its label text (see
/// icon_label_text), whole in the last row, as Caja shows it: there's
/// nothing below for it to cover.
fn icon_cell_text(m: &gtk::TreeModel, it: &gtk::TreeIter, captions: &[&str], columns: i32) -> String {
    let mut text = icon_label_text(m, it, captions);
    let cols = columns.max(1);
    let last_row_start = (m.iter_n_children(None) - 1).max(0) / cols * cols;
    if m.path(it).and_then(|p| p.indices().first().copied()).is_some_and(|i| i >= last_row_start) {
        text.insert(0, crate::iconlabel::WHOLE);
    }
    text
}

/// An icon view label's text: the name, then the captions shown at this
/// zoom level (see details::shown_captions), a line each.
fn icon_label_text(m: &gtk::TreeModel, it: &gtk::TreeIter, captions: &[&str]) -> String {
    let mut text = value_str(&m.value(it, COL_NAME as i32)).to_string();
    for id in captions {
        let line = crate::details::text(id, m, it);
        text.push(crate::iconlabel::CAPTION_SEP);
        // A folder's size is kept until its items are counted, so the
        // label doesn't grow.
        text.push_str(if line.is_empty() && *id == "size" { "…" } else { &line });
    }
    text
}

/// An item's thumbnail file, its pixel size, and whether it's shown with
/// a shadow, if it has one. The size is known from the file before the
/// picture is loaded. As in Caja, pictures with transparency (a logo, an
/// icon) have no shadow: it would show through around them.
fn icon_view_thumb(m: &gtk::TreeModel, it: &gtk::TreeIter) -> Option<(String, (i32, i32), bool)> {
    // Asked for every item at every layout, and most have none: not copied
    // out unless there is one.
    let v = m.value(it, COL_THUMB as i32);
    let thumb = Some(value_str(&v)).filter(|t| !t.is_empty())?;
    let head = icons::thumb_header(thumb)?;
    let shadow = !(head.alpha && m.value(it, COL_CTYPE as i32).get::<&str>().is_ok_and(|t| t.starts_with("image/")));
    Some((thumb.to_string(), (head.width, head.height), shadow))
}

/// The room an item's picture takes in the icon view, as in Caja: its
/// thumbnail with a shadow (longest side `thumb_size`) if it has one, else
/// its icon (`size`).
fn icon_view_dims(thumb: Option<&(String, (i32, i32), bool)>, size: i32, thumb_size: i32) -> (i32, i32) {
    match thumb {
        Some((_, (w, h), shadow)) => {
            let (w, h) = icons::fit(*w, *h, thumb_size);
            if *shadow { (w + 2 * icons::SHADOW, h + icons::SHADOW) } else { (w, h) }
        }
        None => (size, size),
    }
}

/// An item's picture in the icon view (see icon_view_dims).
fn icon_view_pixbuf(m: &gtk::TreeModel, it: &gtk::TreeIter, thumb: Option<(String, (i32, i32), bool)>, size: i32, thumb_size: i32) -> Option<Pixbuf> {
    let pb = match thumb {
        Some((thumb, _, true)) => icons::framed_thumb(&thumb, thumb_size).map(|p| dim_if_cut(m, it, p)),
        Some((thumb, _, false)) => icons::thumb_pixbuf(&thumb, thumb_size).map(|p| dim_if_cut(m, it, p)),
        None => icon_or_thumb(m, it, "", size),
    };
    // Hidden items' pictures at 55 %, as in Caja.
    if m.gv::<bool>(it, COL_HIDDEN as i32) { pb.map(|p| faded(&p, 0.55)) } else { pb }
}

/// `p` drawn at `alpha`.
fn faded(p: &Pixbuf, alpha: f64) -> Pixbuf {
    let (w, h) = (p.width(), p.height());
    match Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, w, h) {
        Some(d) => {
            d.fill(0);
            p.composite(&d, 0, 0, w, h, 0.0, 0.0, 1.0, 1.0, gdk_pixbuf::InterpType::Nearest, (alpha * 255.0) as i32);
            d
        }
        None => p.clone(),
    }
}

fn dim_if_cut(m: &gtk::TreeModel, it: &gtk::TreeIter, pb: Pixbuf) -> Pixbuf {
    if clipboard::any_cut() && clipboard::is_cut(&m.gv::<String>(it, COL_URI as i32)) {
        dim(&pb)
    } else {
        pb
    }
}

fn row_pixbuf(m: &gtk::TreeModel, it: &gtk::TreeIter, size: i32) -> Option<Pixbuf> {
    let v = m.value(it, COL_THUMB as i32);
    icon_or_thumb(m, it, value_str(&v), size)
}

/// An item's thumbnail (`thumb`, its file if it has one) at `size`, else
/// its icon, the thumbnail asked for if it may get one.
fn icon_or_thumb(m: &gtk::TreeModel, it: &gtk::TreeIter, thumb: &str, size: i32) -> Option<Pixbuf> {
    let pb = if size < 32 {
        if !m.gv::<bool>(it, COL_IS_DIR as i32) {
            debug_once(&m.gv::<String>(it, COL_URI as i32), &format!("no thumbnails at {size} px icons"));
        }
        None
    } else if !thumb.is_empty() {
        icons::thumb_pixbuf(thumb, size)
    } else {
        if icons::drawing() {
            request_row_thumbnail(m, it);
        }
        None
    };
    let pb = pb.or_else(|| {
        let icon: Option<gio::Icon> = m.gv(it, COL_ICON as i32);
        icons::icon_pixbuf(icon.as_ref(), size)
    });
    pb.map(|p| dim_if_cut(m, it, p))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    #[test]
    fn rows_of_a_list_and_its_filter() {
        // Models need no display; GTK just has to count as set up.
        unsafe { gtk::set_initialized() };
        let store = gtk::ListStore::new(&[String::static_type()]);
        for n in ["a", ".b", "c", ".d", "e"] {
            store.insert_with_values(None, &[(0, &n)]);
        }
        let names = |m: &gtk::TreeModel| m.rows().iter().map(|it| m.gv::<String>(it, 0)).collect::<Vec<_>>();
        assert_eq!(names(store.upcast_ref()), ["a", ".b", "c", ".d", "e"]);
        let filter = new_filter(&store);
        filter.set_visible_func(|m, it| !m.gv::<String>(it, 0).starts_with('.'));
        assert_eq!(names(filter.upcast_ref()), ["a", "c", "e"]);
        store.clear();
        assert!(filter.rows().is_empty());
    }

    #[test]
    fn numbers_sort_by_value() {
        let mut names = vec!["file10.txt", "file2.txt", "file1.txt", "file20.txt", "v1.10", "v1.9"];
        names.sort_by_key(|n| collate_key(n));
        assert_eq!(names, ["file1.txt", "file2.txt", "file10.txt", "file20.txt", "v1.9", "v1.10"]);
    }

    #[test]
    fn names_of_modes_and_types() {
        for m in [ViewMode::Icon, ViewMode::List, ViewMode::Compact, ViewMode::Columns] {
            assert_eq!(ViewMode::from_str(m.as_str()), m);
        }
        assert_eq!(ViewMode::from_str("bogus"), ViewMode::Icon);
        assert_eq!(type_description("inode/directory", true), "Folder");
        assert!(!type_description("text/plain", false).is_empty());
    }

    #[test]
    fn entry_counts() {
        let t = TestDir::new("count");
        for f in ["a", ".b", "c~", "d/e"] {
            t.file(f, "");
        }
        assert_eq!(count_entries(&t.0, false), Some(2));
        assert_eq!(count_entries(&t.0, true), Some(4));
        assert_eq!(count_entries(&t.0.join("missing"), true), None);
    }
}
