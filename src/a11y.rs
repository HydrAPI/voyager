//! A fix for GTK 3's accessibility.
//!
//! Asked for its state once its widget is gone, the accessible object of a
//! window, button, toggle button (the path bar's, rebuilt on every change
//! of folder), label, entry, expander, check menu item or icon view item
//! gives nothing, where it should say it's defunct. The accessibility
//! bridge then fails two assertions for each such object, every time a
//! client (a screen reader) reads the tree. Their classes' ref_state_set is
//! wrapped here to give a defunct state instead; everything else is left to
//! GTK.

use gtk::atk::ffi as atk;

type RefStateSet = unsafe extern "C" fn(*mut atk::AtkObject) -> *mut atk::AtkStateSet;

/// GTK's own functions of the wrapped classes, in the order
/// fix_defunct_states wraps them.
static ORIGINALS: [std::sync::OnceLock<RefStateSet>; 8] = [const { std::sync::OnceLock::new() }; 8];

unsafe extern "C" {
    fn gtk_window_accessible_get_type() -> glib::ffi::GType;
    fn gtk_button_accessible_get_type() -> glib::ffi::GType;
    fn gtk_toggle_button_accessible_get_type() -> glib::ffi::GType;
    fn gtk_label_accessible_get_type() -> glib::ffi::GType;
    fn gtk_entry_accessible_get_type() -> glib::ffi::GType;
    fn gtk_expander_accessible_get_type() -> glib::ffi::GType;
    fn gtk_check_menu_item_accessible_get_type() -> glib::ffi::GType;
}

/// One for each class (a class's function may call its parent's, so each
/// has to know whose original to call).
unsafe extern "C" fn ref_state_set<const N: usize>(obj: *mut atk::AtkObject) -> *mut atk::AtkStateSet {
    // SAFETY: called by ATK with one of its objects, through class N's
    // slot, whose original function was kept before it was replaced.
    unsafe {
        let set = ORIGINALS[N].get().map_or(std::ptr::null_mut(), |f| f(obj));
        if !set.is_null() {
            return set;
        }
        let set = atk::atk_state_set_new();
        atk::atk_state_set_add_state(set, atk::ATK_STATE_DEFUNCT);
        set
    }
}

const WRAPPERS: [RefStateSet; 8] = [
    ref_state_set::<0>, ref_state_set::<1>, ref_state_set::<2>, ref_state_set::<3>,
    ref_state_set::<4>, ref_state_set::<5>, ref_state_set::<6>, ref_state_set::<7>,
];

/// Wraps the classes' ref_state_set (once, at start: before any of their
/// subclasses is set up, which then copy the wrapped function). Parents
/// come before their subclasses.
pub fn fix_defunct_states() {
    let mut types = Vec::new();
    // SAFETY: GTK's own type functions (public API); they register the types.
    unsafe {
        types.extend([
            gtk_window_accessible_get_type(),
            gtk_button_accessible_get_type(),
            gtk_toggle_button_accessible_get_type(),
            gtk_label_accessible_get_type(),
            gtk_entry_accessible_get_type(),
            gtk_expander_accessible_get_type(),
            gtk_check_menu_item_accessible_get_type(),
        ]);
    }
    // The icon view's item class is private: registered when the first item
    // is made, so one is.
    let store = gtk::ListStore::new(&[glib::types::Type::STRING]);
    store.insert_with_values(None, &[(0, &"")]);
    let view = gtk::IconView::with_model(&store);
    view.set_text_column(0);
    use gtk::prelude::*;
    // SAFETY: plain GObject and ATK calls on objects that live through them.
    unsafe {
        if let Some(acc) = view.accessible() {
            let child = atk::atk_object_ref_accessible_child(glib::translate::ToGlibPtr::to_glib_none(&acc).0, 0);
            if !child.is_null() {
                glib::gobject_ffi::g_object_unref(child as *mut _);
            }
        }
        let item = glib::gobject_ffi::g_type_from_name(c"GtkIconViewItemAccessible".as_ptr());
        if item != 0 {
            types.push(item);
        }
    }
    for (n, ty) in types.into_iter().enumerate() {
        if ORIGINALS[n].get().is_some() {
            continue;
        }
        // SAFETY: the class is kept referenced for good (g_type_class_ref),
        // so the wrapped function stays in place; it's an AtkObject class.
        unsafe {
            let class = glib::gobject_ffi::g_type_class_ref(ty) as *mut atk::AtkObjectClass;
            if let Some(f) = (*class).ref_state_set {
                if ORIGINALS[n].set(f).is_ok() {
                    (*class).ref_state_set = Some(WRAPPERS[n]);
                }
            }
        }
    }
}
