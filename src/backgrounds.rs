//! Backgrounds and emblems, as Caja's "Backgrounds and Emblems" window:
//! colours and patterns dragged onto a folder's view (its background, kept
//! as GVfs metadata on the folder, as Caja keeps it) or the side pane, and
//! emblems dragged onto items. The drags use Caja's own types, so its
//! window's colours, patterns and emblems can be dropped on Arkha too, and
//! the other way round. Your own colours, patterns and emblems can be
//! added: emblems where Caja puts them (~/.icons/hicolor/48x48/emblems),
//! so both show them.

use gdk_pixbuf::Pixbuf;
use gio::prelude::*;
use gtk::prelude::*;
use std::cell::RefCell;

use crate::{config, icons, util};

pub const COLOR_TARGET: &str = "application/x-color";
pub const PATTERN_TARGET: &str = "property/bgimage";
pub const EMBLEM_TARGET: &str = "property/keyword";
pub const RESET_TARGET: &str = "x-special/mate-reset-background";
/// The emblem keyword that takes all of an item's emblems off (Caja's).
pub const ERASE: &str = "erase";

/// What was dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum Drop {
    /// "#rrggbb".
    Color(String),
    /// A picture's URI.
    Pattern(String),
    /// An emblem keyword ("important"), or ERASE.
    Emblem(String),
    Reset,
}

/// What views and the side pane take besides files.
pub fn targets() -> Vec<gtk::TargetEntry> {
    [COLOR_TARGET, PATTERN_TARGET, EMBLEM_TARGET, RESET_TARGET].iter().map(|t| gtk::TargetEntry::new(t, gtk::TargetFlags::empty(), 0)).collect()
}

pub fn is_target(name: &str) -> bool {
    [COLOR_TARGET, PATTERN_TARGET, EMBLEM_TARGET, RESET_TARGET].contains(&name)
}

/// A drop's contents: a colour as GTK sends it (red, green, blue, alpha:
/// 16 bits each, this machine's byte order), the rest as text.
pub fn parse(target: &str, data: &[u8]) -> Option<Drop> {
    match target {
        COLOR_TARGET => {
            let c: Vec<u16> = data.as_chunks::<2>().0.iter().map(|b| u16::from_ne_bytes(*b)).collect();
            (c.len() >= 3).then(|| format!("#{:02x}{:02x}{:02x}", c[0] >> 8, c[1] >> 8, c[2] >> 8)).map(Drop::Color)
        }
        PATTERN_TARGET => {
            let t = String::from_utf8_lossy(data).trim().trim_end_matches('\0').to_string();
            let uri = t.lines().next().unwrap_or("").trim().to_string();
            (!uri.is_empty()).then(|| if uri.contains("://") { uri } else { gio::File::for_path(&uri).uri().to_string() }).map(Drop::Pattern)
        }
        EMBLEM_TARGET => {
            let k = String::from_utf8_lossy(data).trim().trim_end_matches('\0').to_string();
            (!k.is_empty()).then_some(Drop::Emblem(k))
        }
        RESET_TARGET => Some(Drop::Reset),
        _ => None,
    }
}

/// A drop's contents as GTK sends them, with their type.
pub fn encode(d: &Drop) -> (&'static str, Vec<u8>) {
    match d {
        Drop::Color(hex) => {
            let rgba = gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::WHITE);
            let c = [rgba.red(), rgba.green(), rgba.blue(), 1.0].map(|v| (v * 65535.0).round() as u16);
            (COLOR_TARGET, c.iter().flat_map(|v| v.to_ne_bytes()).collect())
        }
        Drop::Pattern(uri) => (PATTERN_TARGET, uri.as_bytes().to_vec()),
        Drop::Emblem(k) => (EMBLEM_TARGET, k.as_bytes().to_vec()),
        Drop::Reset => (RESET_TARGET, b"reset".to_vec()),
    }
}

/// CSS giving `selector` a background colour and/or picture (tiled), or
/// none ("" for both). Selected rows keep the theme's.
pub fn css(selector: &str, color: &str, image: &str) -> String {
    if color.is_empty() && image.is_empty() {
        return String::new();
    }
    let mut rules = String::new();
    if let Some(c) = gdk::RGBA::parse(color).ok().filter(|_| !color.is_empty()) {
        rules.push_str(&format!("background-color: {c};"));
    }
    if !image.is_empty() {
        rules.push_str(&format!("background-image: url(\"{}\"); background-repeat: repeat;", image.replace('"', "%22")));
    }
    format!("{selector} {{ {rules} }}")
}

/// The colours offered (as Caja's), then your own.
const COLORS: &[&str] = &[
    "#ffffff", "#f6f5f4", "#deddda", "#c0bfbc", "#9a9996", "#5e5c64", "#3d3846", "#000000", "#f9f06b", "#f8e45c", "#ffbe6f", "#ffa348", "#f66151", "#e01b24",
    "#dc8add", "#c061cb", "#99c1f1", "#62a0ea", "#3584e4", "#1c71d8", "#8ff0a4", "#57e389", "#33d17a", "#26a269", "#cdab8f", "#b5835a", "#986a44", "#63452c",
];

/// Where your own patterns are kept.
fn patterns_dir() -> std::path::PathBuf {
    glib::user_data_dir().join("arkha").join("patterns")
}

/// Patterns: Caja's, if it's installed, then your own.
fn patterns() -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    for dir in [std::path::PathBuf::from("/usr/share/caja/patterns"), patterns_dir()] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            // Caja's "reset" picture is its Reset item, not a pattern.
            let mut v: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.file_stem().is_none_or(|s| s != "reset")).collect();
            v.sort();
            out.extend(v);
        }
    }
    out
}

/// Where your own emblems go, as Caja installs them.
fn emblems_dir() -> std::path::PathBuf {
    glib::home_dir().join(".icons").join("hicolor").join("48x48").join("emblems")
}

/// Whether `keyword` can name a new emblem: letters, numbers and spaces,
/// as Caja allows.
pub fn keyword_ok(keyword: &str) -> bool {
    !keyword.trim().is_empty() && keyword.chars().all(|c| c.is_alphanumeric() || c == ' ') && keyword.trim().to_lowercase() != ERASE
}

/// Installs a picture as your own emblem `keyword` (as Caja does: a 48 px
/// PNG and an .icon file naming it), for every program using the theme.
fn add_emblem(picture: &std::path::Path, keyword: &str) -> Result<(), String> {
    let pb = icons::load_at_scale(picture, 48).map_err(|e| e.message().to_string())?;
    let dir = emblems_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = keyword.trim().to_lowercase().replace(' ', "-");
    pb.savev(dir.join(format!("emblem-{name}.png")), "png", &[]).map_err(|e| e.message().to_string())?;
    let _ = std::fs::write(dir.join(format!("emblem-{name}.icon")), format!("[Icon Data]\nDisplayName={}\n", keyword.trim()));
    // The theme lists it once it's looked at again.
    let _ = std::fs::File::open(glib::home_dir().join(".icons").join("hicolor")).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    if let Some(t) = gtk::IconTheme::default() {
        t.rescan_if_needed();
    }
    Ok(())
}

/// A swatch to drag: a picture and its name.
fn swatch(picture: Option<Pixbuf>, name: &str, drop: Drop, remove: Option<Box<dyn Fn()>>) -> gtk::EventBox {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let img = gtk::Image::new();
    if let Some(p) = &picture {
        img.set_from_pixbuf(Some(p));
    }
    b.pack_start(&img, false, false, 0);
    let l = gtk::Label::new(Some(name));
    l.set_ellipsize(pango::EllipsizeMode::End);
    l.set_max_width_chars(12);
    b.pack_start(&l, false, false, 0);
    let ev = gtk::EventBox::new();
    ev.add(&b);
    ev.set_tooltip_text(Some(&format!("{name}: drag it onto {}", if matches!(drop, Drop::Emblem(_)) { "an item" } else { "a folder's background or the side pane" })));
    let (kind, _) = encode(&drop);
    ev.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &[gtk::TargetEntry::new(kind, gtk::TargetFlags::empty(), 0)], gdk::DragAction::COPY);
    if let Some(p) = &picture {
        ev.drag_source_set_icon_pixbuf(p);
    }
    ev.connect_drag_data_get(move |_, _, sel, _, _| {
        let (kind, data) = encode(&drop);
        sel.set(&gdk::Atom::intern(kind), 8, &data);
    });
    // Your own: a right click removes it.
    if let Some(remove) = remove {
        let remove = std::rc::Rc::new(remove);
        ev.connect_button_press_event(move |ev, e| {
            if e.button() != 3 {
                return glib::Propagation::Proceed;
            }
            let menu = gtk::Menu::new();
            let item = gtk::MenuItem::with_mnemonic("_Remove");
            let remove = remove.clone();
            item.connect_activate(move |_| remove());
            menu.append(&item);
            menu.show_all();
            menu.set_attach_widget(Some(ev));
            menu.popup_at_pointer(Some(e));
            glib::Propagation::Stop
        });
    }
    ev
}

/// A colour's swatch, framed (white shows on white).
fn color_picture(hex: &str) -> Option<Pixbuf> {
    let c = gdk::RGBA::parse(hex).ok()?;
    let v = |x: f64| (x * 255.0).round() as u32;
    let frame = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 56, 40)?;
    frame.fill(0x9a9996ff);
    let inside = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 54, 38)?;
    inside.fill((v(c.red()) << 24) | (v(c.green()) << 16) | (v(c.blue()) << 8) | 0xff);
    inside.copy_area(0, 0, 54, 38, &frame, 1, 1);
    Some(frame)
}

thread_local! {
    static WINDOW: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

/// Edit → Backgrounds and Emblems: Patterns, Colors and Emblems to drag.
pub fn show(parent: &gtk::Window) {
    if let Some(w) = WINDOW.with(|w| w.borrow().clone()) {
        w.present();
        return;
    }
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title("Backgrounds and Emblems");
    win.set_transient_for(Some(parent));
    win.set_type_hint(gdk::WindowTypeHint::Dialog);
    win.set_default_size(560, 440);
    win.connect_destroy(|_| {
        WINDOW.with(|w| w.borrow_mut().take());
        FLOWS.with(|f| f.borrow_mut().clear());
    });
    let nb = gtk::Notebook::new();
    nb.set_border_width(6);
    let pages: [(&str, &str, &str); 3] = [("Patterns", "Add a New _Pattern…", "Drag a pattern onto a folder's background, or the side pane. Reset takes the background off."), ("Colors", "Add a New _Color…", "Drag a colour onto a folder's background, or the side pane. Reset takes the background off."), ("Emblems", "Add a New _Emblem…", "Drag an emblem onto an item (onto one of the selected ones: onto all of them). Erase takes them all off.")];
    let flows: Vec<gtk::FlowBox> = pages
        .iter()
        .map(|(title, add, about)| {
            let flow = gtk::FlowBox::new();
            flow.set_selection_mode(gtk::SelectionMode::None);
            flow.set_valign(gtk::Align::Start);
            flow.set_max_children_per_line(8);
            flow.set_row_spacing(10);
            flow.set_column_spacing(10);
            flow.set_border_width(12);
            let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
            sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            sw.set_vexpand(true);
            sw.add(&flow);
            let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
            let hint = gtk::Label::new(Some(about));
            hint.set_xalign(0.0);
            hint.set_line_wrap(true);
            hint.style_context().add_class("dim-label");
            hint.set_margin_start(12);
            hint.set_margin_top(8);
            page.pack_start(&hint, false, false, 0);
            page.pack_start(&sw, true, true, 0);
            let add_btn = gtk::Button::with_mnemonic(add);
            add_btn.set_halign(gtk::Align::Start);
            add_btn.set_margin_start(12);
            add_btn.set_margin_bottom(8);
            page.pack_start(&add_btn, false, false, 0);
            nb.append_page(&page, Some(&gtk::Label::new(Some(title))));
            let w = win.clone();
            let which = *title;
            add_btn.connect_clicked(move |_| add_new(&w, which));
            flow
        })
        .collect();
    win.add(&nb);
    WINDOW.with(|w| *w.borrow_mut() = Some(win.clone()));
    // The lists, filled again whenever one changes.
    FLOWS.with(|f| *f.borrow_mut() = flows.clone());
    fill();
    win.show_all();
}

thread_local! {
    static FLOWS: RefCell<Vec<gtk::FlowBox>> = const { RefCell::new(vec![]) };
}

/// Fills the window's three lists.
fn fill() {
    let flows = FLOWS.with(|f| f.borrow().clone());
    let [patterns_flow, colors_flow, emblems_flow] = flows.as_slice() else { return };
    for f in [patterns_flow, colors_flow, emblems_flow] {
        for c in f.children() {
            f.remove(&c);
        }
    }
    let reset = || swatch(icons::named_pixbuf("edit-clear", 40), "Reset", Drop::Reset, None);
    patterns_flow.add(&reset());
    let mine = patterns_dir();
    for p in patterns() {
        let name = p.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default();
        let remove: Option<Box<dyn Fn()>> = p.starts_with(&mine).then(|| {
            let p = p.clone();
            Box::new(move || {
                let _ = std::fs::remove_file(&p);
                fill();
            }) as Box<dyn Fn()>
        });
        patterns_flow.add(&swatch(icons::load_at_scale(&p, 56).ok(), &name, Drop::Pattern(gio::File::for_path(&p).uri().to_string()), remove));
    }
    colors_flow.add(&reset());
    let own = config::with(|s| s.own_colors.clone());
    for c in COLORS.iter().map(|c| c.to_string()).chain(own.iter().cloned()) {
        let remove: Option<Box<dyn Fn()>> = own.contains(&c).then(|| {
            let c = c.clone();
            Box::new(move || {
                config::update(|s| s.own_colors.retain(|x| *x != c));
                fill();
            }) as Box<dyn Fn()>
        });
        colors_flow.add(&swatch(color_picture(&c), &c, Drop::Color(c.clone()), remove));
    }
    emblems_flow.add(&swatch(icons::named_pixbuf("edit-clear", 40), "Erase", Drop::Emblem(ERASE.into()), None));
    let mine = emblems_dir();
    for (icon, title) in icons::available_emblems() {
        let keyword = icon.strip_prefix("emblem-").unwrap_or(&icon).to_string();
        let file = mine.join(format!("{icon}.png"));
        let remove: Option<Box<dyn Fn()>> = file.exists().then(|| {
            let icon = icon.clone();
            Box::new(move || {
                let _ = std::fs::remove_file(emblems_dir().join(format!("{icon}.png")));
                let _ = std::fs::remove_file(emblems_dir().join(format!("{icon}.icon")));
                if let Some(t) = gtk::IconTheme::default() {
                    t.rescan_if_needed();
                }
                fill();
            }) as Box<dyn Fn()>
        });
        emblems_flow.add(&swatch(icons::named_pixbuf(&icon, 40), &title, Drop::Emblem(keyword), remove));
    }
    for f in [patterns_flow, colors_flow, emblems_flow] {
        f.show_all();
    }
}

/// Add a New Pattern / Color / Emblem.
fn add_new(win: &gtk::Window, which: &str) {
    match which {
        "Colors" => {
            let d = gtk::ColorChooserDialog::new(Some("Add a New Color"), Some(win));
            if d.run() == gtk::ResponseType::Ok {
                let c = d.rgba();
                let v = |x: f64| (x * 255.0).round() as u8;
                let hex = format!("#{:02x}{:02x}{:02x}", v(c.red()), v(c.green()), v(c.blue()));
                config::update(|s| {
                    if !s.own_colors.contains(&hex) {
                        s.own_colors.push(hex);
                    }
                });
            }
            d.close();
        }
        "Patterns" => {
            let Some(picture) = choose_picture(win, "Add a New Pattern") else { return };
            let dir = patterns_dir();
            let name = picture.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "pattern.png".into());
            let target = dir.join(util::unique_name(&gio::File::for_path(&dir), &name, false));
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::copy(&picture, &target)) {
                util::error_dialog(Some(win), "Could not add the pattern", &e.to_string());
            }
        }
        _ => {
            let d = gtk::Dialog::with_buttons(Some("Add a New Emblem"), Some(win), gtk::DialogFlags::MODAL, &[("_Cancel", gtk::ResponseType::Cancel), ("_Add", gtk::ResponseType::Accept)]);
            let grid = gtk::Grid::new();
            grid.set_row_spacing(8);
            grid.set_column_spacing(12);
            grid.set_border_width(12);
            let name = gtk::Entry::new();
            name.set_activates_default(true);
            let picture = gtk::FileChooserButton::new("Choose a Picture", gtk::FileChooserAction::Open);
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images"));
            filter.add_pixbuf_formats();
            picture.add_filter(filter);
            for (r, (label, w)) in [("_Keyword:", name.upcast_ref::<gtk::Widget>()), ("_Picture:", picture.upcast_ref())].into_iter().enumerate() {
                let l = gtk::Label::with_mnemonic(label);
                l.set_mnemonic_widget(Some(w));
                l.set_xalign(1.0);
                grid.attach(&l, 0, r as i32, 1, 1);
                w.set_hexpand(true);
                grid.attach(w, 1, r as i32, 1, 1);
            }
            let hint = gtk::Label::new(Some("The keyword names the emblem: letters, numbers and spaces."));
            hint.style_context().add_class("dim-label");
            grid.attach(&hint, 0, 2, 2, 1);
            d.content_area().add(&grid);
            d.set_default_response(gtk::ResponseType::Accept);
            d.show_all();
            loop {
                if d.run() != gtk::ResponseType::Accept {
                    break;
                }
                let keyword = name.text().to_string();
                let taken = icons::available_emblems().iter().any(|(i, _)| *i == format!("emblem-{}", keyword.trim().to_lowercase().replace(' ', "-")));
                let problem = match picture.filename() {
                    _ if !keyword_ok(&keyword) => Some("The keyword can only have letters, numbers and spaces.".to_string()),
                    _ if taken => Some("There is already an emblem with that keyword.".to_string()),
                    None => Some("Choose a picture for the emblem.".to_string()),
                    Some(p) => add_emblem(&p, &keyword).err(),
                };
                match problem {
                    Some(why) => util::error_dialog(Some(d.upcast_ref()), "Could not add the emblem", &why),
                    None => break,
                }
            }
            d.close();
        }
    }
    fill();
}

fn choose_picture(win: &gtk::Window, title: &str) -> Option<std::path::PathBuf> {
    let c = gtk::FileChooserDialog::with_buttons(Some(title), Some(win), gtk::FileChooserAction::Open, &[("_Cancel", gtk::ResponseType::Cancel), ("_Add", gtk::ResponseType::Accept)]);
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images"));
    filter.add_pixbuf_formats();
    c.add_filter(filter);
    let picked = (c.run() == gtk::ResponseType::Accept).then(|| c.filename()).flatten();
    c.close();
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_both_ways() {
        for d in [Drop::Color("#3584e4".into()), Drop::Pattern("file:///usr/share/caja/patterns/a%20b.png".into()), Drop::Emblem("important".into()), Drop::Emblem(ERASE.into()), Drop::Reset] {
            let (kind, data) = encode(&d);
            assert!(is_target(kind));
            assert_eq!(parse(kind, &data), Some(d.clone()));
        }
        // A pattern given as a path, as Caja's own window may.
        assert_eq!(parse(PATTERN_TARGET, b"/usr/share/x.png\0"), Some(Drop::Pattern("file:///usr/share/x.png".into())));
        assert_eq!(parse(COLOR_TARGET, &[1]), None);
        assert_eq!(parse("text/plain", b"x"), None);
    }

    #[test]
    fn background_css() {
        assert_eq!(css("iconview", "", ""), "");
        let c = css("iconview", "#ff0000", "");
        assert!(c.contains("background-color: rgb(255,0,0)"), "{c}");
        let i = css("iconview", "", "file:///a/b.png");
        assert!(i.contains("url(\"file:///a/b.png\")") && i.contains("repeat"), "{i}");
    }

    #[test]
    fn emblem_keywords() {
        assert!(keyword_ok("My Stuff 2"));
        assert!(!keyword_ok("a/b") && !keyword_ok("  ") && !keyword_ok("Erase"));
    }
}
