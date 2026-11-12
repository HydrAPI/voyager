# Arkha

A fast graphical file manager for Linux. It is modelled on Caja (MATE) and written in Rust with GTK 3, so it uses your MATE / GTK theme, icons and bookmarks.

## Features

- **Caja layout:** menubar, toolbar (Back / Forward / Up / Reload / Home / File System / Search), breadcrumb path bar (a middle-click opens a folder in a new tab) that switches to a text location bar (Ctrl+L) with path completion, side pane with Computer, Devices and Bookmarks sections (reorder them by dragging their headings), tabs, status bar with selection info and free space. The toolbar, location bar, side pane and status bar can each be hidden (View menu); a hidden location bar still shows while you type a location or search.
- **The bar beside the path:** file operations, split view (and Swap Panes), view, zoom and info panel buttons; drag them to rearrange the bar once View → Lock Toolbar is off (it's on to begin with, so a touchpad can't move them by accident), and hide the ones you don't use (Preferences → Display).
- **Split view (F3):** a second pane side by side, each with its own tabs; the toolbar and menus act on the pane you last clicked (or, optionally, the one under the mouse pointer), F6 switches to the other pane, View → Swap Panes (Ctrl+U, or its toolbar button) swaps the two sides with all their tabs, View → Same Location as Other Pane goes where the other one is, and Edit → Copy/Move to Other Pane transfers the selection across.
- **Tabs like a browser:** reopen closed tabs (Ctrl+Shift+T), move them with Ctrl+Shift+Page Up/Down or drag them, also between windows or out into a new window, optionally always show the tab bar. File → Close All Windows, Go → Clear History, View → Stop (for places that are slow to load).
- **Customizable keyboard shortcuts** for every action (Preferences → Keyboard).
- **Four views:** Icons, List (Name, then any of Size, Type, Modified, Accessed, Created, Owner, Group, Permissions, Octal Permissions, MIME Type, Extension, Location and Size on Disk, sortable by each; Trashed On and Original Location show in the Trash, Location in search results), Compact and Columns (like macOS Finder: the folders on the way in columns side by side, the selected folder's contents or file's preview on the right; ← / → move between them, and every column has context menus, drag and drop and live updates); zoom with Ctrl+plus/minus or Ctrl+scroll, through zoom levels you can set per view.
- **List columns, sort orders and captions as in Caja:** choose and order the list's columns with View → Visible Columns, Preferences → List Columns or a right click on the column headers, or drag a header to another place; View → Arrange Items sorts by any of them too. In the icon view, up to three of them show under the names (Preferences → Display → Icon Captions), more as you zoom in.
- **Per-folder views** (on by default): remember the view and sorting of each folder, so a folder opens the way you left it; View → Reset View to Defaults forgets a folder's.
- Optional alternating row colors in the list view and the bulk rename list.
- **Scrolling:** the mouse wheel scrolls sideways where there's only a horizontal scrollbar, and a middle-click on empty space starts autoscrolling (can be turned off).
- **Customizable context menu:** hide and reorder its entries (drag them, separators included), and add your own entries that run a shell command on the selected files (with `%f`, `%F`, `%n`, `%N`, `%d` placeholders), optionally limited to files, folders, a folder's empty space, file types (videos, audio, images, text, documents, archives) and/or name patterns like `*.jpg`.
- **Thumbnails:** built in for images, and through the thumbnailers installed on the system for everything they handle, as in Caja (`/usr/share/thumbnailers`: PDF, PostScript, DjVu and comics with Evince or Atril, office documents with libgsf, audio cover art with ffmpegthumbnailer, and whatever is installed later). Custom thumbnailer commands, set up in Preferences → Previews, come first for the file types they cover; videos work out of the box with ffmpegthumbnailer or ffmpeg if either is installed. Thumbnailers whose program is missing are skipped. Stored in the shared `~/.cache/thumbnails`, so Caja's thumbnails are reused and vice versa.
- **Previews, as Caja's Preview settings** (Preferences → Previews): thumbnails, a text file's first lines in its icon (from 64 px icons on) and the number of items in folders, each shown always, for local files only (the default: not for GVfs places such as smb:// or phones) or never; images bigger than a set size (100 MB by default) get no thumbnail.
- **File operations:** these run on a background thread with a progress window and cancel button.
  - Copy, cut and paste, using the same clipboard format as Caja and Nautilus (you can copy in one and paste in the other)
  - Drag and drop: Shift moves, Ctrl copies, Alt asks; a plain drag moves on the same filesystem and copies across filesystems
  - Files dragged out of an archive manager (Engrampa, Xarchiver, File Roller before its GTK 4 versions) into a folder, onto a folder or onto the path bar are extracted there (X11 only: the archive managers' XDS protocol needs X). File Roller 43 and later can't drag files out at all.
  - Duplicate, Make Link, Copy To…, Move To…
  - Move to Trash, Delete permanently, Restore from Trash, Empty Trash (also from a bar above the trash folder)
  - Checked copies, for when a copy has to be right: Edit → Paste and Check (Shift+Ctrl+V, also in a folder's menu), the "Check the copies" box in Copy To and Move To, and Copy and Check Here / Move and Check Here in the menu of a drop made with Alt. Each copy is read back from the drive or share (not from memory) and its CRC32 compared with its original's; a move deletes an original only once its copy matched. A copy that doesn't match asks to copy it again or skip it, and at the end a notice says all matched, or a message lists those that didn't.
  - Undo (Edit → Undo, Ctrl+Z) and Redo (Edit → Redo, Ctrl+Shift+Z or Ctrl+Y) for the last 20 operations, as in Caja: copies, moves and links (copies and links go to the trash, moved items go back where they were; after a merge only what was added is taken back, and a copy that replaced a file is left alone), moving to the trash (the items are found in the trash and restored), restoring from the trash, renames (Rename All's as one step), new folders and documents (to the trash; one named at once is one step), and permission, owner and group changes in Properties. Deleting, emptying the trash and Apply to Enclosed Items can't be undone. Anything new done clears Redo.
  - Conflict dialog (Skip / Keep Both / Replace or Merge, with "apply to all", or a new name of your own for the copy)
- Inline rename (F2) that pre-selects the name without its extension.
- **Bulk rename** (Rename All…, for several items): Case, Insert, Remove, Replace, Enumerate and Regex tabs with a live preview, as MATE's caja-rename extension does it, optionally working before or after the extension.
- New folder, new empty document, and new documents from your `~/Templates` folder.
- Live updates when files change on disk.
- **Opening files:** Open / Open With (the apps for the file type, or any other app) / Run or Display for executables, Open in Terminal (F4), and Extract Here / Compress via Engrampa or File Roller.
- **Properties dialog:** type, size (counted in the background), dates, disk usage of a drive's top folder, permissions you can edit (setuid, setgid and sticky too, and for a folder also for everything inside it), the group (one of yours; the owner too when run as root, as in Caja), the SELinux security context, the default app for the file type, and a Media Info tab with what [MediaInfo](https://mediaarea.net/MediaInfo) reports (codecs, resolution, duration…) when `mediainfo` is installed; it can be limited to videos, audio, images or other files. As in Caja, a click on the icon picks a custom icon (or reverts to the usual one), the Emblems tab gives items the icon theme's emblems, and the Notes tab keeps a note with an item; they show on the icons (a note as the note emblem) and are kept as GVfs metadata, shared with Caja. For a single file, the Checksums tab works out its CRC32, MD5, SHA-1 or SHA-256 (also SHA-384, SHA-512 and BLAKE2b) when a checksum's Calculate button is pressed, one at a time, with a progress bar, the time left and Cancel (Calculate All works out all of them in one read of the file); Copy puts it on the clipboard. Preferences → Checksums picks which are offered and their order. The tabs can be dragged into another order, which is kept, as in Rename All.
- **Backgrounds and Emblems** (Edit menu), as Caja's: drag a pattern or colour onto a folder's background (kept with the folder, as Caja keeps it) or onto the side pane, Reset to take it off; drag an emblem onto an item (onto all the selected ones), Erase to take its emblems off. Add your own colours, patterns and emblems (a right click removes them); your own emblems go where Caja keeps them, so both show them. Caja's own window's colours, patterns and emblems can be dropped on Arkha too.
- **Selection and navigation:** type the start of a name to jump to it, search (Ctrl+F: a Search field in the location bar, as in Caja; typing filters the folder, Enter searches it and all its subfolders, with a Location column for the results), select by pattern (Ctrl+S, e.g. `*.jpg`), invert selection, back-button history on right-click, and mouse back/forward buttons (buttons 8 and 9; others, or none, with `mouse_back_button`, `mouse_forward_button` and `mouse_extra_buttons` in `arkha.conf`, as Caja keeps them in dconf).
- Devices: flash drives, memory cards, discs, phones (MTP) and network mounts. Click to mount, unmount / eject / safely remove with the button next to them or the context menu. Newly inserted media are mounted by Arkha itself (no desktop environment needed); this can be turned off, and the eject button can be set to only unmount. As in Caja, a notification says when a removed drive can be unplugged (and, while data is still being written to it, that it shouldn't be yet). On GNOME, Cinnamon, MATE, Xfce, KDE, LXDE and LXQt, that setting is the desktop's own, as Caja's is MATE's: changing it in Arkha changes it in the desktop's settings and the other way round, so there's one setting and nothing to undo when Arkha is removed. The desktop mounts media first; Arkha mounts any it left. Separate automounters (udiskie, devmon, ldm, udisks-glue) can't be switched off from outside: Arkha says so once if it finds one running.
- **Inserted media:** for what a medium Arkha mounts holds (an audio CD, a video DVD, a camera's photos, a music player, software…) it can start a program, open the folder, do nothing, or ask, as in Caja: a dialog to choose, optionally for good (Preferences → Media, with "Never prompt or start programs" and "Browse media when inserted"). On GNOME, Cinnamon and MATE these are the desktop's own settings, so Caja and Arkha share them. Media the desktop mounts itself are left to it.
- **"Show in folder" for other programs:** Arkha provides the `org.freedesktop.FileManager1` D-Bus service, as Caja does: web browsers, download lists and the like open a folder with the file selected, or show its properties. While Arkha runs it does this unless another file manager that does it got there first; Preferences → Behavior can also let programs start Arkha for it (a D-Bus activation file in your own `~/.local/share/dbus-1/services`, which goes before the system's).
- **Info panel:** beside the files (View → Info Panel, F11, or its toolbar button), turned on per folder. It shows the selected item with a few details: a folder's cover picture (`folder.jpg`, `cover.jpg`, … — the names are a setting, matched ignoring case), an image itself, or a file's thumbnail (videos and the like); for several items their number and total size. Folder sizes are left out on network shares (SMB, NFS, SSHFS…), where adding them up would ask the server about every file. Custom icons set in Caja are shown too.
- Bookmarks are shared with Caja and the GTK file chooser (`~/.config/gtk-3.0/bookmarks`). Includes a bookmark editor (Ctrl+B).
- Preferences: single-click mode, default view and sorting, per-folder views, list columns, icon captions, zoom levels, icons in the list view, hidden files, file sizes in IEC units (KiB, MiB), the date format (informal, your language's, ISO), previews (thumbnails and their size limit, text in icons, folder item counts), confirmation before trashing, where new tabs go (after the current tab in the order opened, right after the current tab, or at the end), text on the toolbar buttons, an always-text location bar, the number of hidden items in the status bar (off by default), and which side pane entries to show and their row height.
- **In English and German:** the system's language by default, or the one chosen in Preferences → Display (from the next start); GTK's own dialogs follow it. More languages come as `.po` files (see Translations below).
- **Light on resources:** opens folders with less CPU than Caja and Nemo (big ones too: 20,000 files in about 1.8 s of CPU against Caja's 2.2 s), scrolls with less CPU than Caja (much less in big folders), uses the least memory of the three, and gives memory back when big tabs are closed. The window doesn't freeze when GVfs's metadata service is slow to answer (every image GTK decodes asks it, through the glycin loader's temporary file).

Settings live in `~/.config/arkha/arkha.conf`; per-folder views in `~/.config/arkha/folders.conf`; custom context menu actions in `~/.config/arkha/actions.conf`; custom thumbnailers in `~/.config/arkha/thumbnailers.conf`.

## Building

Needs Rust (1.82+), the GTK 3 development files and gettext (for the translations; without it Arkha is built in English only):

```sh
# Debian / Ubuntu / Mint
sudo apt install build-essential libgtk-3-dev gettext
# Fedora
sudo dnf install gtk3-devel gettext
# Arch
sudo pacman -S gtk3 gettext
```

Then:

```sh
cargo run --release              # try it
./install.sh                     # install to ~/.local/bin + menu entry
./install.sh --default           # ...and use it as the default folder handler
./uninstall.sh                   # remove it again (keeps your settings)
./uninstall.sh --purge           # ...and delete the settings in ~/.config/arkha
```

Uninstalling also gives folders back to the application `--default` replaced.

### Arch Linux package

`packaging/arch/PKGBUILD` builds a pacman package from the committed state of this repository:

```sh
cd packaging/arch
makepkg -si
```

Video thumbnails (`ffmpegthumbnailer` or `ffmpeg`), the Media Info tab (`mediainfo`), phones (`gvfs-mtp`), cameras (`gvfs-gphoto2`), network shares (`gvfs-smb`) and archives (`engrampa`, `file-roller` or `xarchiver`) are optional dependencies.

Trash, network locations and mounting use GVfs, which every MATE install already has. Mounting drives also needs udisks2 (`gvfs` pulls it in on most distributions); Android phones need the GVfs MTP backend (`gvfs-mtp` on Arch, Debian and Fedora).

## Testing

```sh
cargo test --release                                  # unit tests (no display needed)
ARKHA_RANDOM_CASES=50 cargo test --release random     # the randomized tests, 50 times as many inputs
cargo test --release -- --ignored stress              # file operation stress tests (ARKHA_STRESS_FILES=n)
tools/soak.py --backend x11 --minutes 30              # the whole program, driven at random
tools/soak.py --backend wayland --minutes 30 --cycle  # one round repeated: does memory keep growing?
tools/soak.py --backend x11 --minutes 10 --heaptrack   # memory recorded by heaptrack
tools/gui-tests/run-all.sh                            # the GUI tests, X11 and Wayland (cargo build --release first)
tools/gui-tests/undo.sh wayland                       # one of them
tools/bench/compare.sh                                # speed and memory against Caja and Nemo
tools/bench/speed.sh OLD-BINARY target/release/arkha  # opening folders, two builds compared
```

`tools/soak.py` runs Arkha in a bubblewrap sandbox (the system read-only, `/home` and `/mnt` hidden, its own D-Bus session, a headless X server, and for Wayland Weston inside it) and drives it through its D-Bus actions, keyboard and mouse input (clicks, drags, the wheel, keys, typeahead, inline renames, search; xdotool) and its dialogs through accessibility (AT-SPI), while files keep changing underneath. The sandbox has no applications, so opening a file never starts another program. It reports crashes (with backtraces, as Arkha runs under gdb), GLib/GTK warnings, slow or missing responses, and memory over time, in `target/soak/<run>/`. `--fatal-criticals` stops at the first critical warning for its backtrace; `--name` keeps runs of the same seed apart (`target/soak/<backend>-seed<N>-<name>`). Its once-a-minute status line includes the sandbox home's free space: GTK and GVfs warnings about writing files there mean the 1 GiB home has filled up. For test coverage, give it a binary built with `RUSTFLAGS="-C instrument-coverage"` (`--binary`) and `LLVM_PROFILE_FILE=/tmp/soak-run/arkha-%p-%m.profraw`: the profiles land in the run's folder. Needs `bubblewrap`, `xorg-server-xvfb`, `weston`, `gdb`, `xdotool`, `at-spi2-core` and `python-gobject`.

The GUI tests in `tools/gui-tests/` check features with real keyboard and mouse input and read the result back from the files, from Arkha's log, or through accessibility (window titles, tabs and dialogs, the same on X11 and Wayland). Each runs in a private D-Bus session with its own runtime folder, home and settings, in a headless X server with xmonad, or Weston's kiosk shell in it for Wayland, and fails on any GLib/GTK warning in Arkha's log. It prints a PASS or FAIL line per check; screenshots and logs are in `target/gui-tests/<test>-<backend>/`. `lib.sh` holds what they share (writing another test is a few lines of `gt_click`, `gt_key`, `gt_act` and `gt_check`), `fixtures.sh` makes what they need in `target/gui-fixtures/` (a `/usr/share` without applications, so nothing else is ever started; an xmonad build; for the benchmarks 20,000 files and sample files of 30 or so types). `tools/lockbar-test.sh` checks Lock Toolbar with real drags.

| Test | What it checks |
|---|---|
| `navbuttons` | Mouse back and forward buttons over the views as set in Preferences (default, swapped, off); right and middle clicks |
| `tabs` | Where new tabs go |
| `undo` | Undo of a copy and a move; Ctrl+Z in the location bar undoes nothing |
| `selection` | After a paste exactly the copies are selected (50 and 1,500 files) |
| `rename` | Renaming in place: Enter, Escape, a click elsewhere, a reload meanwhile |
| `fm1` | "Show in folder" for other programs (org.freedesktop.FileManager1) |
| `metadata-stuck` | Arkha keeps answering while GVfs's metadata service doesn't |
| `prefs` | Every Preferences page; adding and removing a custom action |
| `backgrounds` | Backgrounds and Emblems opens and closes |
| `focuscol` | No accessibility assertion after the columns view hides the cursor's column |
| `swap` | Swapping the panes of split view: the menu, Ctrl+U and the toolbar button |
| `columns` | The list columns: default, one added from a header's menu (and kept), the Trash's own; sorting by permissions |
| `undo-redo` | Undo and Redo of a move to the trash, a new folder named at once, renames |
| `check-copies` | Checked copies started with Paste and Check, Copy To's box and an Alt drop |
| `bookmarks` | The bookmark editor shows paths, the bookmarks file keeps the URLs |
| `properties` | An emblem ticked and a note typed in Properties, kept in GVfs metadata under Caja's keys |
| `checksums` | The Checksums tab: only the chosen ones, in order; SHA-256 matches sha256sum, the other buttons wait meanwhile; Cancel; Calculate All; Copy |
| `language` | German chosen in the settings: the status bar (with its plural) and side pane in German; the system's language: English again |
| `tab-order` | Properties' and Rename All's tabs dragged into another order, which is kept |
| `previews` | Thumbnails always (with a size limit) or never; folders' item counts always or never |

`tools/bench/compare.sh` measures what the audits show: opening `/usr/lib` and 20,000 files (when the main thread stays idle, its CPU, all threads'), scrolling, memory and thumbnails, against Caja and Nemo where installed; `speed.sh` compares two builds of Arkha opening folders in the icon view, the list view and the list view with every column. `sample.sh` profiles without perf (gdb samples the main thread's stack), summarized by `sample-report.py`.

## Translations

Arkha's texts are translated with gettext. Each language is a file in `po/` (`po/de.po` is German); the build turns them into catalogs (`build.rs`, into `target/release/locale/`), and Preferences offers each by the name in its `X-Language-Name` header.

- **A new language:** `msginit -i po/arkha.pot -o po/<code>.po -l <code>` (e.g. `fr`), add `"X-Language-Name: <the language's name in itself>\n"` to its header, translate it (by hand or with Poedit, Lokalize, …), check it with `msgfmt --check --statistics -o /dev/null po/<code>.po`, and build.
- **After changing texts in the code:** `tools/update-po.sh` collects them again into `po/arkha.pot` and updates every `.po` file (new texts untranslated, changed ones marked fuzzy).
- **In the code:** a text people see is written `tr("…")`, one with a number in it `trn("{n} file", "{n} files", n)`, a text in a table `n_("…")` (translated where it's shown); values go in named places filled by `fill()` (`fill(&tr("Copying “{name}”"), &[("name", &name)])`), so a translation can order the words its own way. A comment starting `TRANSLATORS:` right above it is shown to translators.
- Keep each `{name}` and the `_` of a menu shortcut (`_File`) in a translation; `msgfmt --check` checks the places. The GUI tests always run in English.

## Code layout

| File | What it does |
|---|---|
| `src/main.rs` | GtkApplication setup, command-line paths |
| `src/i18n.rs` | Translations: `tr()`, `trn()`, `n_()`, `fill()`, the language chosen in Preferences |
| `build.rs`, `po/` | The translations (`po/<language>.po`, template `po/arkha.pot`), built into catalogs; `tools/update-po.sh` refreshes them |
| `src/window.rs` | Main window, toolbar, path bar, tabs, all commands |
| `src/window/menus.rs` | The menu bar and the context menus |
| `src/window/prefs.rs` | Preferences, and the editors for menus, places, bookmarks, shortcuts, thumbnailers and custom actions |
| `src/columns.rs` | The side columns of the columns view (folders above, selected folder's contents): clicks, context menus, drag and drop, live updates |
| `src/info.rs` | The info panel: folder covers, image previews and thumbnails, details |
| `src/sidebar.rs` | Side pane: places, devices (mount, eject, automount), bookmarks, section order |
| `src/a11y.rs` | A workaround for GTK 3 accessible objects that report no state once their widget is gone |
| `src/scroll.rs` | Sideways wheel scrolling and middle-click autoscroll in every scrolled area, and a workaround for a GTK 3 overlay-scrollbar bug |
| `src/pane.rs` | One tab: folder loading, monitoring, icon/list views, selection, DnD, inline rename |
| `src/pane/signals.rs` | What clicks, keys, drags and edits do in a tab's views |
| `src/pane/build.rs` | The parts of a tab made on their own: the sorted model and its filter, the list columns, the Trash bar |
| `src/search.rs` | Searching a folder and its subfolders by name |
| `src/bulkrename.rs` | Rename All…: renaming several items at once |
| `src/keys.rs` | Keyboard shortcuts and their editor |
| `src/details.rs` | What can be shown about a file besides its name: the list columns, sort orders and icon captions |
| `src/iconlabel.rs` | The icon view's cells drawn like Caja's: wrapped names cut to three lines, captions, tinted selected icons, boxed selected names |
| `src/iconview.rs` | The icon view: sized in big folders without measuring every item for each size request, and drawing only the rows in sight |
| `src/ops.rs` | Copy / move / link / trash / delete engine on a worker thread, progress + conflict dialogs |
| `src/clipboard.rs` | Caja/Nautilus-compatible file clipboard |
| `src/backgrounds.rs` | Backgrounds and Emblems: folder and side pane backgrounds, emblems by dragging, your own ones |
| `src/metadata.rs` | GVfs metadata (emblems, notes, custom icons, backgrounds) read and written by a thread of its own |
| `src/props.rs` | Properties dialog: basic info and custom icon, emblems, permissions with owner and group, Open With, notes, media info, checksums |
| `src/checksums.rs` | Checksums of a file (MD5, SHA family, CRC32, BLAKE2b): for the Checksums tab and checked copies |
| `src/icons.rs` | Icon cache, background thumbnailer, thumbnail caches |
| `src/imagesize.rs` | An image's width and height from its file header, so big pictures can wait their turn for thumbnails |
| `src/thumbnailers.rs` | Custom thumbnailer commands, and the system's (`.thumbnailer` files) |
| `src/config.rs` | Settings file, zoom levels |
| `src/actions.rs` | Context menu entries (hiding, order), custom actions |
| `src/folders.rs` | View and sorting remembered per folder |
| `src/util.rs` | Formatting, naming, bookmarks, notifications, helpers |
| `src/testutil.rs` | Test helpers: scratch folders, random inputs |

