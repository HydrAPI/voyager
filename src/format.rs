//! Formatting a drive (a memory card, a USB stick, an external or other
//! disk), from This PC's menu. Only drives the system isn't on: never a
//! disk or partition that / or another system folder is mounted from, nor
//! another partition of such a disk. The work is UDisks's (its Block.Format,
//! which asks for the administrator's password through polkit).

use std::collections::HashSet;
use std::rc::Rc;

use gio::prelude::*;
use gtk::prelude::*;

use crate::i18n::{fill, tr};
use crate::util;

/// Folders the running system lives on.
const SYSTEM_FOLDERS: &[&str] = &["/", "/boot", "/boot/efi", "/efi", "/home", "/usr", "/var", "/etc", "/opt", "/srv", "/nix", "/root"];

/// The device names (sda, sda1, nvme0n1, dm-0…) behind `name`: itself, the
/// disk it's a partition of and, for a mapped device (encryption, LVM),
/// the devices under it.
fn related(name: &str, out: &mut HashSet<String>) {
    if !out.insert(name.to_string()) {
        return;
    }
    let sys = std::path::Path::new("/sys/class/block").join(name);
    if let Ok(real) = std::fs::canonicalize(&sys) {
        if let Some(parent) = real.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()) {
            if parent != "block" {
                related(&parent, out);
            }
        }
    }
    if let Ok(slaves) = std::fs::read_dir(sys.join("slaves")) {
        for s in slaves.flatten() {
            related(&s.file_name().to_string_lossy(), out);
        }
    }
}

/// The devices the system folders are mounted from, with all they're made of.
fn protected_devices() -> HashSet<String> {
    let mut out = HashSet::new();
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    for line in mounts.lines() {
        let mut f = line.split_whitespace();
        let (Some(src), Some(mp)) = (f.next(), f.next()) else { continue };
        if !SYSTEM_FOLDERS.contains(&mp) || !src.starts_with("/dev/") {
            continue;
        }
        if let Ok(real) = std::fs::canonicalize(src) {
            if let Some(n) = real.file_name() {
                related(&n.to_string_lossy(), &mut out);
            }
        }
    }
    out
}

/// The device name of a volume ("sdb1"), if it's a plain block device.
fn device_name(v: &gio::Volume) -> Option<String> {
    let dev = v.identifier("unix-device")?;
    let name = dev.strip_prefix("/dev/")?;
    const NOT: &[&str] = &["loop", "ram", "zram", "sr", "dm-", "md", "nbd"];
    if NOT.iter().any(|p| name.starts_with(p)) || name.contains('/') {
        return None;
    }
    Some(name.to_string())
}

/// Whether the volume may be offered for formatting: on a drive, not what
/// the system runs from, nor on the same disk as that.
pub fn can_format(v: &gio::Volume) -> bool {
    if v.drive().is_none() {
        return false;
    }
    let Some(name) = device_name(v) else { return false };
    let mut mine = HashSet::new();
    related(&name, &mut mine);
    protected_devices().is_disjoint(&mine)
}

const FILE_SYSTEMS: &[(&str, &str)] = &[
    ("ext4", "ext4 (Linux)"),
    ("exfat", "exFAT (Windows, macOS and Linux)"),
    ("ntfs", "NTFS (Windows)"),
    ("vfat", "FAT32 (old devices and cameras)"),
    ("btrfs", "Btrfs (Linux)"),
];

/// Asks what to format it as, and for the last yes, then formats it.
/// `done` runs when it has (or hasn't) worked.
pub fn show(parent: Option<&gtk::Window>, volume: gio::Volume, name: String, done: Rc<dyn Fn()>) {
    let Some(dev) = device_name(&volume) else { return };
    // Checked again here: never on the strength of a menu alone.
    if !can_format(&volume) {
        return;
    }
    let d = gtk::Dialog::with_buttons(
        Some(&fill(&tr("Format “{name}”"), &[("name", &name)])),
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[(tr("_Cancel").as_str(), gtk::ResponseType::Cancel), (tr("_Format").as_str(), gtk::ResponseType::Accept)],
    );
    d.set_default_response(gtk::ResponseType::Cancel);
    d.set_response_sensitive(gtk::ResponseType::Accept, false);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    grid.set_border_width(12);
    let warning = gtk::Label::new(None);
    warning.set_markup(&format!(
        "<b>{}</b>",
        glib::markup_escape_text(&fill(&tr("All files on “{name}” (/dev/{dev}) will be erased. This cannot be undone."), &[("name", &name), ("dev", &dev)]))
    ));
    warning.set_line_wrap(true);
    warning.set_max_width_chars(46);
    warning.set_xalign(0.0);
    grid.attach(&warning, 0, 0, 2, 1);
    let fs = gtk::ComboBoxText::new();
    for (id, label) in FILE_SYSTEMS {
        fs.append(Some(id), label);
    }
    fs.set_active(Some(1));
    let label = gtk::Entry::new();
    label.set_hexpand(true);
    label.set_text(&name);
    let l1 = gtk::Label::with_mnemonic(&tr("_File system:"));
    l1.set_mnemonic_widget(Some(&fs));
    l1.set_xalign(1.0);
    let l2 = gtk::Label::with_mnemonic(&tr("_Name:"));
    l2.set_mnemonic_widget(Some(&label));
    l2.set_xalign(1.0);
    grid.attach(&l1, 0, 1, 1, 1);
    grid.attach(&fs, 1, 1, 1, 1);
    grid.attach(&l2, 0, 2, 1, 1);
    grid.attach(&label, 1, 2, 1, 1);
    let erase = gtk::CheckButton::with_mnemonic(&tr("_Overwrite the old data (slow, but it can't be recovered)"));
    grid.attach(&erase, 1, 3, 1, 1);
    let sure = gtk::CheckButton::with_mnemonic(&tr("I _understand that everything on it will be lost"));
    grid.attach(&sure, 1, 4, 1, 1);
    d.content_area().add(&grid);
    {
        let d = d.clone();
        sure.connect_toggled(move |b| d.set_response_sensitive(gtk::ResponseType::Accept, b.is_active()));
    }
    d.show_all();
    let parent = parent.cloned();
    d.connect_response(move |d, r| {
        let chosen = fs.active_id().map(|s| s.to_string()).unwrap_or_else(|| "ext4".into());
        let (volume, name, dev, label, erase, done, parent) = (volume.clone(), name.clone(), dev.clone(), label.text().to_string(), erase.is_active(), done.clone(), parent.clone());
        d.close();
        if r != gtk::ResponseType::Accept {
            return;
        }
        glib::MainContext::default().spawn_local(run(parent, volume, name, dev, chosen, label, erase, done));
    });
}

#[allow(clippy::too_many_arguments)]
async fn run(parent: Option<gtk::Window>, volume: gio::Volume, name: String, dev: String, fs: String, label: String, erase: bool, done: Rc<dyn Fn()>) {
    let busy = gtk::MessageDialog::new(parent.as_ref(), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Info, gtk::ButtonsType::None, &fill(&tr("Formatting “{name}”…"), &[("name", &name)]));
    busy.set_secondary_text(Some(&tr("Please wait, and don't take the drive out.")));
    busy.show();
    let fail = |msg: String| {
        busy.close();
        util::error_dialog(parent.as_ref(), &fill(&tr("Could not format “{name}”"), &[("name", &name)]), &msg);
    };
    // Not mounted while it's formatted.
    if let Some(m) = volume.get_mount() {
        let op = gtk::MountOperation::new(parent.as_ref());
        if let Err(e) = m.unmount_with_operation_future(gio::MountUnmountFlags::NONE, Some(&op)).await {
            fail(e.message().to_string());
            return;
        }
    }
    let path = format!("/org/freedesktop/UDisks2/block_devices/{dev}");
    let proxy = match gio::DBusProxy::for_bus_future(gio::BusType::System, gio::DBusProxyFlags::NONE, None, "org.freedesktop.UDisks2", &path, "org.freedesktop.UDisks2.Block").await {
        Ok(p) => p,
        Err(e) => {
            fail(e.message().to_string());
            return;
        }
    };
    let opts = glib::VariantDict::new(None);
    opts.insert("label", label.as_str());
    if matches!(fs.as_str(), "ext4" | "btrfs") {
        opts.insert("take-ownership", true);
    }
    if erase {
        opts.insert("erase", "zero");
    }
    let params = glib::Variant::tuple_from_iter([fs.to_variant(), opts.end()]);
    match proxy.call_future("Format", Some(&params), gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION, 6 * 3600 * 1000).await {
        Ok(_) => {
            busy.close();
            let ok = gtk::MessageDialog::new(parent.as_ref(), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Info, gtk::ButtonsType::Close, &fill(&tr("“{name}” was formatted"), &[("name", &name)]));
            ok.connect_response(|d, _| d.close());
            ok.show();
        }
        Err(e) => fail(e.message().to_string()),
    }
    done();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_device_is_only_itself() {
        let mut out = HashSet::new();
        related("nonexistent-device", &mut out);
        assert_eq!(out.len(), 1);
    }
}
