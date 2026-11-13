//! Scrolling behaviour shared by every scrolled area in the program:
//! - the mouse wheel scrolls sideways when there's only a horizontal scroll
//!   bar (nothing to scroll up and down);
//! - a middle-click on empty space starts Firefox-style autoscrolling: a
//!   marker appears where you clicked and the view scrolls towards the
//!   pointer, faster the further away it is. A click (or, after dragging with
//!   the button held, releasing it), a key or the wheel stops it. In the
//!   list view it starts on files and folders too, if the list can scroll
//!   (otherwise, or with Shift, a middle-click opens them). It can be turned
//!   off in the preferences.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glib::translate::*;
use gtk::prelude::*;

/// Hooks into every scrolled window when it's realized, so the behaviour
/// applies everywhere, including GTK's own (the sidebar, dialogs).
pub fn init() {
    unsafe extern "C" fn on_realize(
        _hint: *mut glib::gobject_ffi::GSignalInvocationHint,
        n: u32,
        params: *const glib::gobject_ffi::GValue,
        _data: glib::ffi::gpointer,
    ) -> glib::ffi::gboolean {
        if let Some(sw) = emitter(n, params).and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok()) {
            install(&sw);
        }
        glib::ffi::GTRUE
    }
    // Before a container is unrealized (its handler may destroy windows
    // first, a GtkStack's before its children are): see
    // let_go_of_indicators.
    unsafe extern "C" fn on_unrealize(
        _hint: *mut glib::gobject_ffi::GSignalInvocationHint,
        n: u32,
        params: *const glib::gobject_ffi::GValue,
        _data: glib::ffi::gpointer,
    ) -> glib::ffi::gboolean {
        // Any container: a GtkStack destroys windows of its own first
        // without saying it has one.
        if let Some(w) = emitter(n, params).filter(|w| w.is::<gtk::Container>()) {
            let_go_of_indicators(&w);
        }
        glib::ffi::GTRUE
    }
    unsafe {
        let ty = gtk::Widget::static_type().into_glib();
        let id = glib::gobject_ffi::g_signal_lookup(c"realize".as_ptr(), ty);
        glib::gobject_ffi::g_signal_add_emission_hook(id, 0, Some(on_realize), std::ptr::null_mut(), None);
        let id = glib::gobject_ffi::g_signal_lookup(c"unrealize".as_ptr(), ty);
        glib::gobject_ffi::g_signal_add_emission_hook(id, 0, Some(on_unrealize), std::ptr::null_mut(), None);
    }
}

/// The widget a signal was emitted on, from an emission hook's parameters
/// (`n` of them; the first is the instance).
///
/// # Safety
/// `params` must point to `n` valid GValues, as GLib passes them.
unsafe fn emitter(n: u32, params: *const glib::gobject_ffi::GValue) -> Option<gtk::Widget> {
    if n == 0 {
        return None;
    }
    let v = &*(params as *const glib::Value);
    v.get::<Option<gtk::Widget>>().ok().flatten()
}

/// The wheel's movement up (negative) or down in steps, one per notch; a
/// touchpad's or smooth wheel's in fractions.
pub fn vertical_steps(ev: &gdk::EventScroll) -> f64 {
    match ev.direction() {
        gdk::ScrollDirection::Up => -1.0,
        gdk::ScrollDirection::Down => 1.0,
        gdk::ScrollDirection::Smooth => ev.delta().1,
        _ => 0.0,
    }
}

/// Marks a scrolled window whose overlay scrolling was switched off for
/// being unrealized, to switch it on again once it's realized.
const WAS_OVERLAY: &str = "arkha-was-overlay";

/// GTK 3 bug: a scrolled window unrealized while it has overlay scrollbars
/// (a tab taken out of its window, a view whose window goes, a page of a
/// stack) leaves their handlers on its adjustments, for scrollbars it has
/// forgotten: any scrolling or resizing of it until it's realized again
/// warns (gtk_widget_get_mapped, gtk_widget_set_opacity,
/// gdk_window_is_visible). With overlay scrolling switched off first, it
/// lets go of them properly. That has to happen while the windows are still
/// there, so for `w` and every scrolled window inside it, before `w` is
/// unrealized (see init); switched back on in `install`.
fn let_go_of_indicators(w: &gtk::Widget) {
    if !w.is_realized() || w.window().is_none_or(|win| win.is_destroyed()) {
        return;
    }
    if let Some(sw) = w.downcast_ref::<gtk::ScrolledWindow>() {
        if sw.is_overlay_scrolling() {
            // SAFETY: only ever set (and read, in install) as a bool.
            unsafe { sw.set_data(WAS_OVERLAY, true) };
            sw.set_overlay_scrolling(false);
        }
    }
    if let Some(c) = w.downcast_ref::<gtk::Container>() {
        c.forall(let_go_of_indicators);
    }
}

fn install(sw: &gtk::ScrolledWindow) {
    unsafe {
        if sw.data::<bool>("arkha-scroll").is_some() {
            return;
        }
        sw.set_data("arkha-scroll", true);
    }
    // Overlay scrolling back on (see let_go_of_indicators).
    sw.connect_realize(|sw| {
        // SAFETY: only ever set as a bool.
        if unsafe { sw.steal_data::<bool>(WAS_OVERLAY) }.is_some() {
            sw.set_overlay_scrolling(true);
        }
    });
    sw.connect_scroll_event(|sw, ev| {
        let mods = gtk::accelerator_get_default_mod_mask();
        if !(ev.state() & mods).is_empty() || can_scroll(&sw.vadjustment(), sw.vscrollbar_policy()) {
            return glib::Propagation::Proceed;
        }
        let h = sw.hadjustment();
        if !can_scroll(&h, sw.hscrollbar_policy()) {
            return glib::Propagation::Proceed;
        }
        let dy = match ev.direction() {
            gdk::ScrollDirection::Up => -1.0,
            gdk::ScrollDirection::Down => 1.0,
            gdk::ScrollDirection::Smooth => match ev.delta() {
                (dx, dy) if dy != 0.0 && dx == 0.0 => dy,
                _ => return glib::Propagation::Proceed,
            },
            _ => return glib::Propagation::Proceed,
        };
        // The same step GTK uses for the wheel.
        let step = h.page_size().powf(2.0 / 3.0);
        h.set_value((h.value() + dy * step).clamp(h.lower(), h.upper() - h.page_size()));
        glib::Propagation::Stop
    });
    // Middle-clicks that nothing inside handled (entries paste, buttons and
    // rows act on their own) land here.
    let middle = |w: &gtk::Widget, ev: &gdk::EventButton| {
        if ev.button() == 2 && ev.event_type() == gdk::EventType::ButtonPress && autoscroll(w, ev) {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    };
    sw.connect_button_press_event(move |sw, ev| middle(sw.upcast_ref(), ev));
    // Stop autoscrolling if the scrolled area goes away (its tab closed, say).
    sw.connect_unmap(|sw| {
        if with_active(|a| &a.sw == sw).unwrap_or(false) {
            stop();
        }
    });
    // Lists and icon grids keep middle-clicks to themselves, so they get the
    // handler too (the file views run their own first: there, a middle-click
    // on a folder opens it in a new tab).
    match sw.child() {
        Some(c) if c.is::<gtk::TreeView>() => {
            c.connect_button_press_event(move |c, ev| {
                let tv = c.downcast_ref::<gtk::TreeView>().unwrap();
                if ev.window() != tv.bin_window() {
                    return glib::Propagation::Proceed;
                }
                middle(c, ev)
            });
        }
        Some(c) if c.is::<gtk::IconView>() => {
            c.connect_button_press_event(move |c, ev| middle(c, ev));
        }
        _ => {}
    }
}

fn can_scroll(adj: &gtk::Adjustment, policy: gtk::PolicyType) -> bool {
    !matches!(policy, gtk::PolicyType::Never | gtk::PolicyType::External)
        && adj.upper() - adj.lower() > adj.page_size() + 0.5
}

struct Auto {
    sw: gtk::ScrolledWindow,
    marker: gtk::Window,
    seat: gdk::Seat,
    origin: (f64, f64),
    pos: Cell<(f64, f64)>,
    dragged: Cell<bool>,
    horizontal: bool,
    vertical: bool,
    cursor: RefCell<&'static str>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
    last_frame: Cell<i64>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Rc<Auto>>> = const { RefCell::new(None) };
}

const MARKER: i32 = 28;

/// Starts autoscrolling the scrolled window around `w` from a middle-button
/// press. Returns false (and does nothing) if there's nothing to scroll.
pub fn autoscroll(w: &gtk::Widget, ev: &gdk::EventButton) -> bool {
    stop();
    if !crate::config::with(|s| s.autoscroll) {
        return false;
    }
    let Some(sw) = w
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|a| a.downcast::<gtk::ScrolledWindow>().ok())
        .or_else(|| w.clone().downcast::<gtk::ScrolledWindow>().ok())
    else {
        return false;
    };
    let horizontal = can_scroll(&sw.hadjustment(), sw.hscrollbar_policy());
    let vertical = can_scroll(&sw.vadjustment(), sw.vscrollbar_policy());
    if !horizontal && !vertical {
        return false;
    }
    let Some(seat) = ev.device().and_then(|d| d.seat()) else { return false };
    let origin = ev.root();

    let marker = gtk::Window::new(gtk::WindowType::Popup);
    // Its own group, so a modal dialog's grab doesn't swallow its events.
    gtk::WindowGroup::new().add_window(&marker);
    if let Some(top) = sw.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()) {
        marker.set_transient_for(Some(&top));
    }
    let screen = gtk::prelude::WidgetExt::screen(&marker);
    if let Some(v) = screen.as_ref().filter(|s| s.is_composited()).and_then(|s| s.rgba_visual()) {
        marker.set_visual(Some(&v));
    }
    marker.set_app_paintable(true);
    marker.set_default_size(MARKER, MARKER);
    marker.move_(origin.0 as i32 - MARKER / 2, origin.1 as i32 - MARKER / 2);
    marker.add_events(
        gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::KEY_PRESS_MASK
            | gdk::EventMask::SCROLL_MASK,
    );
    marker.connect_draw(move |m, cr| {
        draw_marker(m, cr, horizontal, vertical);
        glib::Propagation::Stop
    });
    marker.show();

    let a = Rc::new(Auto {
        sw: sw.clone(),
        marker: marker.clone(),
        seat: seat.clone(),
        origin,
        pos: Cell::new(origin),
        dragged: Cell::new(false),
        horizontal,
        vertical,
        cursor: RefCell::new(""),
        tick: RefCell::new(None),
        last_frame: Cell::new(0),
    });
    a.update_cursor();
    // No cursor of its own: the marker window's cursor is shown everywhere
    // while grabbed, so changing it changes the pointer.
    let Some(win) = marker.window() else {
        unsafe { marker.destroy() };
        return false;
    };

    marker.connect_motion_notify_event(|_, ev| {
        with_active(|a| {
            let p = ev.root();
            a.pos.set(p);
            // Moving past the dead zone with the button held means dragging:
            // letting go then stops, as in Firefox.
            if ev.state().contains(gdk::ModifierType::BUTTON2_MASK)
                && ((p.0 - a.origin.0).abs() > 12.0 || (p.1 - a.origin.1).abs() > 12.0)
            {
                a.dragged.set(true);
            }
            a.update_cursor();
        });
        glib::Propagation::Stop
    });
    marker.connect_button_press_event(|_, _| {
        stop();
        glib::Propagation::Stop
    });
    marker.connect_button_release_event(|_, ev| {
        if ev.button() == 2 && with_active(|a| a.dragged.get()).unwrap_or(false) {
            stop();
        }
        glib::Propagation::Stop
    });
    marker.connect_key_press_event(|_, _| {
        stop();
        glib::Propagation::Stop
    });
    marker.connect_scroll_event(|_, _| {
        stop();
        glib::Propagation::Stop
    });
    marker.connect_grab_broken_event(|_, _| {
        stop();
        glib::Propagation::Stop
    });
    let id = sw.add_tick_callback(|_, clock| {
        if with_active(|a| a.step(clock.frame_time())).is_none() {
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    *a.tick.borrow_mut() = Some(id);
    let session = Rc::downgrade(&a);
    ACTIVE.with(|c| *c.borrow_mut() = Some(a));

    let ev = ev.clone();
    let grab = move || {
        let caps = gdk::SeatCapabilities::ALL_POINTING | gdk::SeatCapabilities::KEYBOARD;
        seat.grab(&win, caps, false, None, Some(&ev), None) == gdk::GrabStatus::Success
    };
    if !grab() {
        // Something else (the window manager, say) can still hold the
        // pointer for a moment after the click: retry briefly, then give up.
        let tries = Cell::new(0);
        glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
            let current = ACTIVE.with(|c| c.borrow().as_ref().map(Rc::downgrade));
            if !current.is_some_and(|c| c.ptr_eq(&session)) || grab() {
                return glib::ControlFlow::Break;
            }
            tries.set(tries.get() + 1);
            if tries.get() >= 10 {
                stop();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
    true
}

fn with_active<R>(f: impl FnOnce(&Auto) -> R) -> Option<R> {
    let a = ACTIVE.with(|c| c.borrow().clone())?;
    Some(f(&a))
}

fn stop() {
    let Some(a) = ACTIVE.with(|c| c.borrow_mut().take()) else { return };
    let taken = a.tick.borrow_mut().take();
    if let Some(id) = taken {
        id.remove();
    }
    a.seat.ungrab();
    unsafe { a.marker.destroy() };
}

/// Firefox's speed curve: nothing within 12px of the marker, then growing
/// faster than linearly with the distance (px per 20ms).
fn accelerate(cur: f64, start: f64) -> f64 {
    let v = (cur - start) / 12.0;
    if v > 1.0 {
        v * v.sqrt() - 1.0
    } else if v < -1.0 {
        v * (-v).sqrt() + 1.0
    } else {
        0.0
    }
}

impl Auto {
    fn step(&self, now: i64) {
        let last = self.last_frame.replace(now);
        if last == 0 {
            return;
        }
        let k = (now - last) as f64 / 20_000.0;
        let (x, y) = self.pos.get();
        let scroll = |adj: gtk::Adjustment, d: f64| {
            if d != 0.0 {
                adj.set_value((adj.value() + d * k).clamp(adj.lower(), adj.upper() - adj.page_size()));
            }
        };
        if self.horizontal {
            scroll(self.sw.hadjustment(), accelerate(x, self.origin.0));
        }
        if self.vertical {
            scroll(self.sw.vadjustment(), accelerate(y, self.origin.1));
        }
    }

    /// Points the cursor the way the view scrolls (a 4-way cursor while in
    /// the dead zone).
    fn update_cursor(&self) {
        let (x, y) = self.pos.get();
        let dx = if self.horizontal { accelerate(x, self.origin.0) } else { 0.0 };
        let dy = if self.vertical { accelerate(y, self.origin.1) } else { 0.0 };
        // Only diagonal when clearly off both axes.
        let h = if dx.abs() > dy.abs() / 2.5 { dx.signum() as i32 * (dx != 0.0) as i32 } else { 0 };
        let v = if dy.abs() > dx.abs() / 2.5 { dy.signum() as i32 * (dy != 0.0) as i32 } else { 0 };
        let name = match (h, v) {
            (0, -1) => "n-resize",
            (0, 1) => "s-resize",
            (-1, 0) => "w-resize",
            (1, 0) => "e-resize",
            (-1, -1) => "nw-resize",
            (1, -1) => "ne-resize",
            (-1, 1) => "sw-resize",
            (1, 1) => "se-resize",
            _ => "all-scroll",
        };
        if *self.cursor.borrow() == name {
            return;
        }
        *self.cursor.borrow_mut() = name;
        if let Some(win) = self.marker.window() {
            let cursor = gdk::Cursor::from_name(&win.display(), name)
                .or_else(|| gdk::Cursor::from_name(&win.display(), "default"));
            win.set_cursor(cursor.as_ref());
        }
    }
}

/// A round marker with arrows for the directions that can scroll.
fn draw_marker(m: &gtk::Window, cr: &gtk::cairo::Context, horizontal: bool, vertical: bool) {
    let r = MARKER as f64 / 2.0;
    let rgba = gtk::prelude::WidgetExt::screen(m).map(|s| s.is_composited()).unwrap_or(false);
    cr.set_operator(gtk::cairo::Operator::Source);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    let _ = cr.paint();
    cr.set_operator(gtk::cairo::Operator::Over);
    // Light disc with a dark outline (like Firefox's), readable on any theme.
    if rgba {
        cr.arc(r, r, r - 1.0, 0.0, std::f64::consts::TAU);
    } else {
        cr.rectangle(0.0, 0.0, 2.0 * r, 2.0 * r);
    }
    cr.set_source_rgba(0.98, 0.98, 0.98, 0.92);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(0.3, 0.3, 0.3, 0.9);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    cr.set_source_rgba(0.2, 0.2, 0.2, 1.0);
    cr.arc(r, r, 2.0, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    // (direction x, direction y) of each arrow.
    let mut arrows = Vec::new();
    if vertical {
        arrows.extend([(0.0, -1.0), (0.0, 1.0)]);
    }
    if horizontal {
        arrows.extend([(-1.0, 0.0), (1.0, 0.0)]);
    }
    for (ax, ay) in arrows {
        // Tip near the edge, base 4px further in.
        let (tx, ty) = (r + ax * (r - 4.0), r + ay * (r - 4.0));
        let (bx, by) = (r + ax * (r - 9.0), r + ay * (r - 9.0));
        cr.move_to(tx, ty);
        cr.line_to(bx + ay * 4.0, by + ax * 4.0);
        cr.line_to(bx - ay * 4.0, by - ax * 4.0);
        cr.close_path();
        let _ = cr.fill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoscroll_speed() {
        // Nothing within 12px of the marker.
        for d in [0.0, 5.0, -5.0, 12.0, -12.0] {
            assert_eq!(accelerate(100.0 + d, 100.0), 0.0);
        }
        // Then it starts from nothing, without a jump at the edge …
        let edge = accelerate(112.5, 100.0);
        assert!(edge > 0.0 && edge < 0.1);
        // … the same speed either way …
        assert_eq!(accelerate(160.0, 100.0), -accelerate(40.0, 100.0));
        // … and it grows faster than the distance.
        let s = |d: f64| accelerate(d, 0.0);
        assert!((s(48.0) - 7.0).abs() < 1e-9);
        assert!(s(96.0) > 2.0 * s(48.0));
        let mut last = 0.0;
        for d in 13..400 {
            let v = s(d as f64);
            assert!(v > last);
            last = v;
        }
    }
}
