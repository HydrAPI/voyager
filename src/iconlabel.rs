//! The icon view's cells, drawn like Caja's: names wrapped below the icon
//! and cut to a few lines with "…", captions (size, date) under them at
//! large zoom levels, selected items with a tinted icon and their whole
//! name in a rounded box over what's below (see `IconLabel::draw_selected`).
//!
//! The label's sizes are remembered: GtkIconView measures every item again
//! whenever any row of its model changes, which with thousands of items
//! would mean laying out every name with Pango each time.
//!
//! With a wrap width of -1 (the compact view, names beside the icons) the
//! label is a plain GtkCellRendererText, only with the sizes remembered.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// Separates the name from the captions in a label's text.
pub const CAPTION_SEP: char = '\u{1f}';
/// Starts the text of a label shown whole, not cut to the line limit (as
/// Caja does for the last row, where it covers nothing below).
pub const WHOLE: char = '\u{1e}';

glib::wrapper! {
    pub struct IconLabel(ObjectSubclass<imp::IconLabel>)
        @extends gtk::CellRendererText, gtk::CellRenderer;
}

impl IconLabel {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Draws names this many pixels right of where GtkIconView puts them
    /// (where Caja has them, see Pane::fit_icon_columns).
    pub fn set_x_shift(&self, px: i32) {
        self.imp().x_shift.set(px);
    }

    /// Sets the text to show. It's set for every item at every layout:
    /// below icons the label measures and draws it itself, so it's only
    /// kept here (setting the property costs more than the rest); beside
    /// them GtkCellRendererText does, from the property.
    pub fn set_label_text(&self, text: &str) {
        let imp = self.imp();
        {
            let mut t = imp.text.borrow_mut();
            if *t != text {
                t.clear();
                t.push_str(text);
            }
        }
        if !imp.below() {
            self.set_property("text", text);
        }
    }

    /// How names are shown below icons: at most `max_lines` lines (0: all),
    /// in the view's font made `font_delta` points larger (or smaller).
    pub fn set_style(&self, max_lines: i32, font_delta: i32) {
        let imp = self.imp();
        if imp.style.get() != (max_lines, font_delta) {
            imp.style.set((max_lines, font_delta));
            imp.props.set(None);
        }
    }

    /// Draws a selected item's label as Caja does, over what's below: the
    /// whole name (not cut to a few lines) in a rounded box of the selection
    /// color. `area` is the label cell's area in `widget`, `text` what it
    /// shows.
    pub fn draw_selected(&self, widget: &gtk::Widget, cr: &gtk::cairo::Context, area: &gdk::Rectangle, text: &str) {
        self.draw_over(widget, cr, area, text, true);
    }

    /// Draws the focus line around a label (the keyboard's item, as Caja).
    pub fn draw_focus(&self, widget: &gtk::Widget, cr: &gtk::cairo::Context, area: &gdk::Rectangle, text: &str) {
        self.draw_over(widget, cr, area, text, false);
    }

    /// The box around a label's text (where it's drawn selected, and where
    /// clicking hits it): (x, y, width, height) in `widget`. `whole`: the
    /// name not cut to the line limit.
    pub fn text_box(&self, widget: &gtk::Widget, area: &gdk::Rectangle, text: &str, whole: bool) -> Option<(f64, f64, f64, f64)> {
        let imp = self.imp();
        let p = imp.props();
        if p.wrap_width < 0 || text.is_empty() {
            return None;
        }
        let layouts = imp.drawn_layouts(widget, text, whole);
        let (x, y) = self.text_origin(area);
        // Caja's box: the lines as laid out, 4 px more on the sides and 1
        // above and below.
        let (mut left, mut right) = (f64::MAX, f64::MIN);
        for l in layouts.iter() {
            let (_, logical) = l.pixel_extents();
            left = left.min(x + logical.x() as f64);
            right = right.max(x + (logical.x() + logical.width()) as f64);
        }
        let height = lines_height(&layouts);
        Some((left - BOX_PAD_X, y - BOX_PAD_Y, right - left + 2.0 * BOX_PAD_X, height + BOX_PAD_Y + BOX_PAD_BOTTOM))
    }

    /// Where a label's lines start in its cell's `area`.
    fn text_origin(&self, area: &gdk::Rectangle) -> (f64, f64) {
        let imp = self.imp();
        let p = imp.props();
        ((area.x() + imp.x_shift.get()) as f64 + (area.width() - p.wrap_width) as f64 / 2.0, area.y() as f64 + p.ypad as f64)
    }

    /// The item is where a drag would drop: drawn as selected, as in Caja
    /// (set for each item before it's drawn).
    pub fn set_drop_target(&self, on: bool) {
        self.imp().drop_target.set(on);
    }

    fn draw_over(&self, widget: &gtk::Widget, cr: &gtk::cairo::Context, area: &gdk::Rectangle, text: &str, selected: bool) {
        let Some((bx, by, bw, bh)) = self.text_box(widget, area, text, selected) else { return };
        let ctx = widget.style_context();
        let bg = ctx.lookup_color("theme_selected_bg_color").unwrap_or_else(|| gdk::RGBA::new(0.08, 0.33, 0.62, 1.0));
        let layouts = self.imp().drawn_layouts(widget, text, selected);
        let (x, y) = self.text_origin(area);
        ctx.save();
        if selected {
            cr.save().ok();
            rounded_rect(cr, bx, by, bw, bh, BOX_RADIUS);
            cr.set_source_rgba(bg.red(), bg.green(), bg.blue(), bg.alpha());
            let _ = cr.fill();
            cr.restore().ok();
            ctx.set_state(ctx.state() | gtk::StateFlags::SELECTED);
            draw_lines(&ctx, cr, x, y, &layouts);
        } else {
            // The view's own focus box is switched off (outline-width: 0
            // with the "caja-like" class), not this one.
            ctx.remove_class("caja-like");
            gtk::render_focus(&ctx, cr, bx, by, bw, bh);
        }
        ctx.restore();
    }
}

/// Draws a label's lines from (`x`, `y`) in the style's text color: the
/// name, then captions as Caja does ("dim-label").
fn draw_lines(ctx: &gtk::StyleContext, cr: &gtk::cairo::Context, x: f64, mut y: f64, layouts: &[pango::Layout]) {
    for (i, l) in layouts.iter().enumerate() {
        if i == 1 {
            ctx.save();
            ctx.add_class("dim-label");
            y += CAPTION_GAP;
        }
        gtk::render_layout(ctx, cr, x, y, l);
        y += l.pixel_size().1 as f64;
    }
    if layouts.len() > 1 {
        ctx.restore();
    }
}

impl Default for IconLabel {
    fn default() -> Self {
        Self::new()
    }
}

/// The selection box around a name: how far it reaches past the text
/// (Caja's TEXT_BACK_PADDING_X/Y, and at the bottom its LABEL_OFFSET too),
/// and its corners' radius.
const BOX_PAD_X: f64 = 4.0;
const BOX_PAD_Y: f64 = 1.0;
const BOX_PAD_BOTTOM: f64 = 2.0;
const BOX_RADIUS: f64 = 3.0;
/// Caption lines are drawn this much below the name: Caja counts the
/// name's padding into its height when drawing, not when measuring (the
/// rows and the selection box leave it out).
const CAPTION_GAP: f64 = 2.0;

/// `name` with a zero-width space after each "_", "-" and "." (unless a
/// digit follows): lines may break there, as Caja lets them.
fn breakable(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        let digit_next = chars.peek().is_some_and(|n| n.is_ascii_digit());
        if c == '_' || c == '-' || (c == '.' && !digit_next) {
            out.push('\u{200b}');
        }
    }
    out
}

/// The height of a label's lines: the name, then its captions.
fn lines_height(layouts: &[pango::Layout]) -> f64 {
    layouts.iter().map(|l| l.pixel_size().1).sum::<i32>() as f64
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * PI / 2.0);
    cr.close_path();
}

glib::wrapper! {
    /// The icon view's picture cell: selected icons tinted with the
    /// selection color, as Caja does (see `set_tint`).
    pub struct IconImage(ObjectSubclass<imp::IconImage>)
        @extends gtk::CellRendererPixbuf, gtk::CellRenderer;
}

impl IconImage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_tint(&self, on: bool) {
        self.imp().tint.set(on);
    }

    /// Draws pictures this many pixels right of where GtkIconView puts them
    /// (see IconLabel::set_x_shift).
    pub fn set_x_shift(&self, px: i32) {
        self.imp().x_shift.set(px);
    }

    pub fn x_shift(&self) -> i32 {
        self.imp().x_shift.get()
    }

    /// See IconLabel::set_drop_target.
    pub fn set_drop_target(&self, on: bool) {
        self.imp().drop_target.set(on);
    }

    /// Sets the picture. The same one again does nothing: it's set for
    /// every item at every layout (None, when not drawing), and each set is
    /// a property change with its notification.
    pub fn set_picture(&self, picture: Option<gdk_pixbuf::Pixbuf>) {
        let imp = self.imp();
        if *imp.picture.borrow() == picture {
            return;
        }
        self.show(picture.as_ref());
        imp.picture.replace(picture);
    }

    /// Has GtkCellRendererPixbuf draw `picture`: from its cairo surface
    /// if it's small (see surface_of), else as it is.
    fn show(&self, picture: Option<&gdk_pixbuf::Pixbuf>) {
        match picture.and_then(surface_of) {
            Some(s) => self.set_property("surface", s),
            None => self.set_property("pixbuf", picture),
        }
    }
}

impl Default for IconImage {
    fn default() -> Self {
        Self::new()
    }
}

/// `pixbuf` with each color multiplied by `color`'s, like Caja's selected
/// icons (eel_create_colorized_pixbuf).
/// Pictures up to this many pixels keep the cairo surface they're drawn
/// from (see surface_of).
const KEPT_SURFACE_PIXELS: i32 = 128 * 128;

/// The cairo surface a small picture is drawn from, made once and kept
/// with it: GtkCellRendererPixbuf would make it again each time it draws
/// one, every item in sight at every step of scrolling. Bigger pictures
/// (thumbnails at large sizes) aren't given one, to keep the memory.
fn surface_of(p: &gdk_pixbuf::Pixbuf) -> Option<gtk::cairo::Surface> {
    if p.width() * p.height() > KEPT_SURFACE_PIXELS {
        return None;
    }
    thread_local! { static KEY: glib::Quark = glib::Quark::from_str("arkha-surface"); }
    let key = KEY.with(|k| *k);
    // SAFETY: only ever set here, with this type.
    if let Some(s) = unsafe { p.qdata::<gtk::cairo::Surface>(key) } {
        return Some(unsafe { s.as_ref() }.clone());
    }
    use gdk::prelude::GdkPixbufExt;
    let s = p.create_surface(1, None::<&gdk::Window>)?;
    unsafe { p.set_qdata(key, s.clone()) };
    Some(s)
}

fn colorized(pixbuf: &gdk_pixbuf::Pixbuf, color: &gdk::RGBA) -> Option<gdk_pixbuf::Pixbuf> {
    let out = pixbuf.add_alpha(false, 0, 0, 0).ok()?;
    let (w, h, stride) = (out.width() as usize, out.height() as usize, out.rowstride() as usize);
    let mul = [color.red(), color.green(), color.blue()].map(|v| (v.clamp(0.0, 1.0) * 255.0) as u32);
    // The copy is ours alone.
    let pixels = unsafe { out.pixels() };
    for y in 0..h {
        for x in 0..w {
            let i = y * stride + x * 4;
            for (c, m) in mul.iter().enumerate() {
                pixels[i + c] = ((pixels[i + c] as u32 * m) >> 8) as u8;
            }
        }
    }
    Some(out)
}

mod imp {
    use super::*;

    /// Measured sizes: (kind of request, the width/height it's for, hash
    /// of everything the size depends on) → (minimum, natural).
    type Sizes = HashMap<(u8, i32, u64), (i32, i32)>;

    /// The properties other than the text, read once: views ask for sizes
    /// thousands of times in a row, and reading a property takes a while.
    #[derive(Clone, Copy)]
    pub struct Props {
        hash: u64,
        pub wrap_width: i32,
        ellipsized: bool,
        xpad: i32,
        pub ypad: i32,
    }

    pub struct IconLabel {
        sizes: RefCell<Sizes>,
        /// The layouts of labels drawn lately (see drawn_layouts).
        drawn: RefCell<HashMap<u64, Rc<[pango::Layout]>>>,
        /// The text last set with set_label_text.
        pub text: RefCell<String>,
        pub props: Cell<Option<Props>>,
        /// (max lines, font size change in points): see set_style.
        pub style: Cell<(i32, i32)>,
        pub x_shift: Cell<i32>,
        pub drop_target: Cell<bool>,
    }

    impl Default for IconLabel {
        fn default() -> Self {
            IconLabel {
                sizes: RefCell::default(),
                drawn: RefCell::default(),
                text: RefCell::default(),
                props: Cell::new(None),
                style: Cell::new((0, 0)),
                x_shift: Cell::new(0),
                drop_target: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconLabel {
        const NAME: &'static str = "ArkhaIconLabel";
        type Type = super::IconLabel;
        type ParentType = gtk::CellRendererText;
    }

    impl ObjectImpl for IconLabel {
        fn constructed(&self) {
            self.parent_constructed();
            // The text is set for every item; anything else is read again.
            // (Told apart by the property itself: its name would be checked
            // for UTF-8 at every item.)
            let text = self.obj().find_property("text").map(|p| p.as_ptr());
            self.obj().connect_notify_local(None, move |obj, pspec| {
                if Some(pspec.as_ptr()) != text {
                    obj.imp().props.set(None);
                }
            });
        }
    }
    impl CellRendererTextImpl for IconLabel {}

    impl IconLabel {
        pub fn props(&self) -> Props {
            if let Some(p) = self.props.get() {
                return p;
            }
            let obj = self.obj();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for p in ["wrap-width", "width-chars", "max-width-chars"] {
                obj.property::<i32>(p).hash(&mut h);
            }
            let ellipsize = obj.property::<pango::EllipsizeMode>("ellipsize");
            obj.property::<pango::WrapMode>("wrap-mode").into_glib().hash(&mut h);
            ellipsize.into_glib().hash(&mut h);
            obj.padding().hash(&mut h);
            self.style.get().hash(&mut h);
            let p = Props {
                hash: h.finish(),
                wrap_width: obj.property::<i32>("wrap-width"),
                ellipsized: ellipsize != pango::EllipsizeMode::None,
                xpad: obj.padding().0,
                ypad: obj.padding().1,
            };
            self.props.set(Some(p));
            p
        }

        /// Names below icons (else the compact view's beside them).
        pub(super) fn below(&self) -> bool {
            let p = self.props();
            p.wrap_width > -1 && !p.ellipsized
        }

        fn text(&self) -> String {
            self.text.borrow().clone()
        }

        /// A hash of what the text's size depends on.
        fn key(&self, widget: &gtk::Widget) -> u64 {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.props().hash.hash(&mut h);
            self.text.borrow().hash(&mut h);
            // Hashed where the context keeps it: asked for every item's
            // every measurement, a copy each time costs more than the rest.
            let ctx = widget.pango_context();
            // SAFETY: the context is alive; the description stays its own.
            let font = unsafe {
                let d = pango::ffi::pango_context_get_font_description(glib::translate::ToGlibPtr::to_glib_none(&ctx).0);
                if d.is_null() { 0 } else { pango::ffi::pango_font_description_hash(d) }
            };
            font.hash(&mut h);
            widget.scale_factor().hash(&mut h);
            h.finish()
        }

        fn cached(&self, widget: &gtk::Widget, kind: u8, for_size: i32, measure: impl FnOnce() -> (i32, i32)) -> (i32, i32) {
            let k = (kind, for_size, self.key(widget));
            if let Some(s) = self.sizes.borrow().get(&k) {
                return *s;
            }
            let s = measure();
            let mut sizes = self.sizes.borrow_mut();
            if sizes.len() > 50_000 {
                sizes.clear();
            }
            sizes.insert(k, s);
            s
        }

        /// The layouts of a label below its icon: the name, then a line for
        /// each caption. `whole`: the name not cut to the line limit.
        pub fn layouts(&self, widget: &gtk::Widget, text: &str, whole: bool) -> Vec<pango::Layout> {
            let p = self.props();
            let (max_lines, font_delta) = self.style.get();
            let mut font = widget.pango_context().font_description().unwrap_or_default();
            if font_delta != 0 {
                if font.is_size_absolute() {
                    font.set_absolute_size((font.size() + font_delta * pango::SCALE * 4 / 3) as f64);
                } else {
                    font.set_size(font.size() + font_delta * pango::SCALE);
                }
            }
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_insert_hyphens(false));
            let make = |t: &str| {
                let l = widget.create_pango_layout(Some(t));
                l.set_font_description(Some(&font));
                l.set_width(p.wrap_width * pango::SCALE);
                l.set_alignment(pango::Alignment::Center);
                l.set_attributes(Some(&attrs.copy().unwrap_or_default()));
                l
            };
            let (text, whole) = match text.strip_prefix(WHOLE) {
                Some(t) => (t, true),
                None => (text, whole),
            };
            let mut parts = text.split(CAPTION_SEP);
            let name = make(&breakable(parts.next().unwrap_or("")));
            name.set_wrap(pango::WrapMode::WordChar);
            // As Caja: always ellipsized, with no height limit when whole
            // (Pango places the lines a little differently otherwise).
            name.set_ellipsize(pango::EllipsizeMode::End);
            name.set_height(if whole || max_lines <= 0 { i32::MIN } else { -max_lines });
            let mut out = vec![name];
            for caption in parts {
                let l = make(caption);
                l.set_ellipsize(pango::EllipsizeMode::End);
                out.push(l);
            }
            out
        }

        /// `layouts`, kept for the labels drawn lately: every label in
        /// sight is drawn again at each step of scrolling, and laying out
        /// its text costs more than drawing it.
        pub fn drawn_layouts(&self, widget: &gtk::Widget, text: &str, whole: bool) -> Rc<[pango::Layout]> {
            let ctx = widget.pango_context();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (self.props().hash, self.style.get(), text, whole, widget.scale_factor()).hash(&mut h);
            // The context (its font, resolution and options apply) and
            // its font, as `key` reads it.
            glib::translate::ToGlibPtr::<*mut pango::ffi::PangoContext>::to_glib_none(&ctx).0.hash(&mut h);
            // SAFETY: as in `key`.
            unsafe {
                let d = pango::ffi::pango_context_get_font_description(glib::translate::ToGlibPtr::to_glib_none(&ctx).0);
                if !d.is_null() {
                    pango::ffi::pango_font_description_hash(d).hash(&mut h);
                }
            }
            let k = h.finish();
            if let Some(l) = self.drawn.borrow().get(&k) {
                return l.clone();
            }
            let l: Rc<[pango::Layout]> = self.layouts(widget, text, whole).into();
            let mut drawn = self.drawn.borrow_mut();
            // More than a few windows full: start again.
            if drawn.len() >= 600 {
                drawn.clear();
            }
            drawn.insert(k, l.clone());
            l
        }

        fn label_height(&self, widget: &gtk::Widget) -> i32 {
            let text = self.text();
            if text.is_empty() {
                return 0;
            }
            lines_height(&self.layouts(widget, &text, false)).ceil() as i32 + 2 * self.props().ypad
        }
    }

    use glib::translate::IntoGlib;

    impl CellRendererImpl for IconLabel {
        fn preferred_width<P: IsA<gtk::Widget>>(&self, widget: &P) -> (i32, i32) {
            // Labels below icons are laid out and centered in the wrap width.
            let p = self.props();
            if self.below() {
                return (p.wrap_width + 2 * p.xpad, p.wrap_width + 2 * p.xpad);
            }
            self.cached(widget.upcast_ref(), 0, -1, || self.parent_preferred_width(widget))
        }

        fn preferred_height<P: IsA<gtk::Widget>>(&self, widget: &P) -> (i32, i32) {
            let (min_width, _) = self.preferred_width(widget);
            self.preferred_height_for_width(widget, min_width)
        }

        fn preferred_height_for_width<P: IsA<gtk::Widget>>(&self, widget: &P, width: i32) -> (i32, i32) {
            if self.below() {
                // As high at any width: drawn in the wrap width.
                let w = widget.upcast_ref();
                return self.cached(w, 2, -1, || {
                    let h = self.label_height(w);
                    (h, h)
                });
            }
            self.cached(widget.upcast_ref(), 2, width, || self.parent_preferred_height_for_width(widget, width))
        }

        fn preferred_width_for_height<P: IsA<gtk::Widget>>(&self, widget: &P, height: i32) -> (i32, i32) {
            if self.below() {
                return self.preferred_width(widget);
            }
            self.cached(widget.upcast_ref(), 3, height, || self.parent_preferred_width_for_height(widget, height))
        }

        fn render<P: IsA<gtk::Widget>>(
            &self,
            cr: &gtk::cairo::Context,
            widget: &P,
            background_area: &gdk::Rectangle,
            cell_area: &gdk::Rectangle,
            flags: gtk::CellRendererState,
        ) {
            if !self.below() {
                return self.parent_render(cr, widget, background_area, cell_area, flags);
            }
            // Selected ones are drawn whole over what's below, after the
            // view (IconLabel::draw_selected).
            let text = self.text();
            if flags.contains(gtk::CellRendererState::SELECTED) || self.drop_target.get() || text.is_empty() {
                return;
            }
            let widget = widget.upcast_ref();
            let p = self.props();
            let shift = self.x_shift.get();
            let x = (cell_area.x() + shift) as f64 + (cell_area.width() - p.wrap_width) as f64 / 2.0;
            let y = cell_area.y() as f64 + p.ypad as f64;
            // GtkIconView clips each item to where it puts it: drawn
            // `shift` to the right, the clip goes along, or a name as wide
            // as the item would lose its right end.
            let shifted_clip = shift != 0 && cr.save().is_ok();
            if shifted_clip {
                if let Ok((x1, y1, x2, y2)) = cr.clip_extents() {
                    cr.reset_clip();
                    cr.rectangle(x1 + shift as f64, y1, x2 - x1, y2 - y1);
                    cr.clip();
                }
            }
            draw_lines(&widget.style_context(), cr, x, y, &self.drawn_layouts(widget, &text, false));
            if shifted_clip {
                let _ = cr.restore();
            }
        }
    }

    #[derive(Default)]
    pub struct IconImage {
        /// The picture set (see set_picture).
        pub picture: RefCell<Option<gdk_pixbuf::Pixbuf>>,
        pub tint: Cell<bool>,
        pub x_shift: Cell<i32>,
        pub drop_target: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconImage {
        const NAME: &'static str = "ArkhaIconImage";
        type Type = super::IconImage;
        type ParentType = gtk::CellRendererPixbuf;
    }

    impl ObjectImpl for IconImage {}
    impl CellRendererPixbufImpl for IconImage {}

    impl IconImage {
        /// The fixed size, if set: what GtkCellRenderer makes the size in
        /// the end anyway, without GtkCellRendererPixbuf first working one
        /// out through its style context (for every item at every layout).
        fn fixed(&self) -> Option<(i32, i32)> {
            let (w, h) = self.obj().fixed_size();
            (w >= 0 && h >= 0).then_some((w, h))
        }
    }

    impl CellRendererImpl for IconImage {
        fn preferred_width<P: IsA<gtk::Widget>>(&self, widget: &P) -> (i32, i32) {
            match self.fixed() {
                Some((w, _)) => (w, w),
                None => self.parent_preferred_width(widget),
            }
        }

        fn preferred_height<P: IsA<gtk::Widget>>(&self, widget: &P) -> (i32, i32) {
            match self.fixed() {
                Some((_, h)) => (h, h),
                None => self.parent_preferred_height(widget),
            }
        }

        fn preferred_height_for_width<P: IsA<gtk::Widget>>(&self, widget: &P, width: i32) -> (i32, i32) {
            match self.fixed() {
                Some((_, h)) => (h, h),
                None => self.parent_preferred_height_for_width(widget, width),
            }
        }

        fn preferred_width_for_height<P: IsA<gtk::Widget>>(&self, widget: &P, height: i32) -> (i32, i32) {
            match self.fixed() {
                Some((w, _)) => (w, w),
                None => self.parent_preferred_width_for_height(widget, height),
            }
        }

        fn render<P: IsA<gtk::Widget>>(
            &self,
            cr: &gtk::cairo::Context,
            widget: &P,
            background_area: &gdk::Rectangle,
            cell_area: &gdk::Rectangle,
            flags: gtk::CellRendererState,
        ) {
            let obj = self.obj();
            let shifted;
            let cell_area = if self.x_shift.get() != 0 {
                shifted = gdk::Rectangle::new(cell_area.x() + self.x_shift.get(), cell_area.y(), cell_area.width(), cell_area.height());
                &shifted
            } else {
                cell_area
            };
            let tinted = (self.tint.get() && (flags.contains(gtk::CellRendererState::SELECTED) || self.drop_target.get()))
                .then(|| {
                    let color = widget.style_context().lookup_color("theme_selected_bg_color")?;
                    colorized(self.picture.borrow().as_ref()?, &color)
                })
                .flatten();
            match tinted {
                Some(t) => {
                    obj.set_property("pixbuf", &t);
                    self.parent_render(cr, widget, background_area, cell_area, flags);
                    obj.show(self.picture.borrow().as_ref());
                }
                None => self.parent_render(cr, widget, background_area, cell_area, flags),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_break_after_separators() {
        let z = '\u{200b}';
        assert_eq!(breakable("my_long-file.name.txt"), format!("my_{z}long-{z}file.{z}name.{z}txt"));
        // Not inside a number ("1.5"), nor where nothing follows the dot.
        assert_eq!(breakable("v1.5.tar"), format!("v1.5.{z}tar"));
        assert_eq!(breakable("plain"), "plain");
        assert_eq!(breakable(""), "");
    }

    #[test]
    fn random_names_keep_their_characters() {
        crate::testutil::cases(3000, |r| {
            let name = r.text(20);
            assert_eq!(breakable(&name).replace('\u{200b}', ""), name.replace('\u{200b}', ""));
            let _ = crate::pane::collate_key(&name);
        });
    }
}
