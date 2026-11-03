# Arkha

A graphical file manager for Linux, modelled on Caja (MATE). Written in Rust with GTK 3, so it uses your MATE / GTK theme, icons and bookmarks.

## Features

- **Caja layout:** menubar, toolbar (Back / Forward / Up / Reload / Home / File System / Filter), breadcrumb path bar that switches to a text location bar (Ctrl+L) with path completion, side pane with Computer, Devices and Bookmarks sections (reorder them by dragging their headings), tabs, status bar with selection info and free space.
- **Split view (F3):** a second pane side by side, each with its own tabs; the toolbar and menus act on the pane you last clicked, and Edit → Copy/Move to Other Pane transfers the selection across.
- **Tabs like a browser:** reopen closed tabs (Ctrl+Shift+T), drag tabs between windows or out into a new window, optionally always show the tab bar.
- **Customizable keyboard shortcuts** for every action (Preferences → Keyboard).
- **Four views:** Icons, List (sortable Name / Size / Type / Modified columns), Compact and Columns (like macOS Finder: the folders on the way in columns side by side, the selected folder's contents or file's preview on the right; ← / → move between them, and every column has context menus, drag and drop and live updates); zoom with Ctrl+plus/minus or Ctrl+scroll, through zoom levels you can set per view.
- **Per-folder views** (on by default): remember the view and sorting of each folder, so a folder opens the way you left it.
- Optional alternating row colors in the list view.
- **Customizable context menu:** hide and reorder its entries (drag them, separators included), and add your own entries that run a shell command on the selected files (with `%f`, `%F`, `%n`, `%N`, `%d` placeholders), optionally limited to files, folders, a folder's empty space, file types (videos, audio, images, text, documents, archives) and/or name patterns like `*.jpg`.
- **Thumbnails:** built in for images, and for any other file type through custom thumbnailer commands, set up in Preferences → Display. Videos work out of the box with ffmpegthumbnailer or ffmpeg if either is installed; thumbnailers whose program is missing are skipped. Stored in the shared `~/.cache/thumbnails`, so Caja's thumbnails are reused and vice versa.
- **File operations:** these run on a background thread with a progress window and cancel button.
  - Copy, cut and paste, using the same clipboard format as Caja and Nautilus (you can copy in one and paste in the other)
  - Drag and drop: Shift moves, Ctrl copies, Alt asks; a plain drag moves on the same filesystem and copies across filesystems
  - Duplicate, Make Link, Copy To…, Move To…
  - Move to Trash, Delete permanently, Restore from Trash, Empty Trash
  - Conflict dialog (Skip / Keep Both / Replace or Merge, with "apply to all")
- Inline rename (F2) that pre-selects the name without its extension.
- New folder, new empty document, and new documents from your `~/Templates` folder.
- Live updates when files change on disk.
- **Opening files:** Open / Open With (the apps for the file type, or any other app) / Run or Display for executables, Open in Terminal (F4), and Extract Here / Compress via Engrampa or File Roller.
- **Properties dialog:** type, size (counted in the background), dates, permissions you can edit, the default app for the file type, and a Media Info tab with what [MediaInfo](https://mediaarea.net/MediaInfo) reports (codecs, resolution, duration…) when `mediainfo` is installed; it can be limited to videos, audio, images or other files.
- **Selection and navigation:** type the start of a name to jump to it, search (Ctrl+F: a Search field in the location bar, as in Caja; typing filters the folder, Enter searches it and all its subfolders, with a Location column for the results), select by pattern (Ctrl+S, e.g. `*.jpg`), invert selection, back-button history on right-click, and mouse back/forward buttons.
- Devices: flash drives, memory cards, discs, phones (MTP) and network mounts. Click to mount, unmount / eject / safely remove with the button next to them or the context menu. Newly inserted media are mounted by Arkha itself (no desktop environment needed); this can be turned off, and the eject button can be set to only unmount. Arkha switches off the desktop's own automounting (GNOME, Cinnamon, MATE, Xfce, KDE, and PCManFM / PCManFM-Qt on LXDE / LXQt) so only its setting counts; `uninstall.sh` switches it back on. Separate automounters (udiskie, devmon, ldm, udisks-glue) can't be switched off from outside: Arkha says so once if it finds one running.
- **Info panel:** beside the files (View → Info Panel, F11, or its toolbar button), turned on per folder. It shows the selected item with a few details: a folder's cover picture (`folder.jpg`, `cover.jpg`, … — the names are a setting, matched ignoring case), an image itself, or a file's thumbnail (videos and the like); for several items their number and total size. Folder sizes are left out on network shares (SMB, NFS, SSHFS…), where adding them up would ask the server about every file. Custom icons set in Caja are shown too.
- Bookmarks are shared with Caja and the GTK file chooser (`~/.config/gtk-3.0/bookmarks`). Includes a bookmark editor (Ctrl+B).
- Preferences: single-click mode, default view and sorting, per-folder views, zoom levels, hidden files, file sizes in IEC units (KiB, MiB), thumbnails, confirmation before trashing, the number of hidden items in the status bar (off by default), and which side pane entries to show and their row height.

Settings live in `~/.config/arkha/arkha.conf`; per-folder views in `~/.config/arkha/folders.conf`; custom context menu actions in `~/.config/arkha/actions.conf`; custom thumbnailers in `~/.config/arkha/thumbnailers.conf`.

## Building

Needs Rust (1.70+) and the GTK 3 development files:

```sh
# Debian / Ubuntu / Mint
sudo apt install build-essential libgtk-3-dev
# Fedora
sudo dnf install gtk3-devel
# Arch
sudo pacman -S gtk3
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

Trash, network locations and mounting use GVfs, which every MATE install already has. Mounting drives also needs udisks2 (`gvfs` pulls it in on most distributions); Android phones need the GVfs MTP backend (`gvfs-mtp` on Arch, Debian and Fedora).

## Code layout

| File | What it does |
|---|---|
| `src/main.rs` | GtkApplication setup, command-line paths |
| `src/window.rs` | Main window, menus, toolbar, path bar, tabs, all commands |
| `src/columns.rs` | The side columns of the columns view (folders above, selected folder's contents): clicks, context menus, drag and drop, live updates |
| `src/info.rs` | The info panel: folder covers, image previews and thumbnails, details |
| `src/sidebar.rs` | Side pane: places, devices (mount, eject, automount), bookmarks, section order |
| `src/scroll.rs` | Sideways wheel scrolling and middle-click autoscroll in every scrolled area |
| `src/pane.rs` | One tab: folder loading, monitoring, icon/list views, selection, DnD, inline rename |
| `src/iconlabel.rs` | The icon view's cells drawn like Caja's: wrapped names cut to three lines, captions, tinted selected icons, boxed selected names |
| `src/ops.rs` | Copy / move / link / trash / delete engine on a worker thread, progress + conflict dialogs |
| `src/clipboard.rs` | Caja/Nautilus-compatible file clipboard |
| `src/props.rs` | Properties dialog |
| `src/icons.rs` | Icon cache and background thumbnailer |
| `src/thumbnailers.rs` | Custom thumbnailer commands |
| `src/config.rs` | Settings file, zoom levels |
| `src/actions.rs` | Context menu entries (hiding, order), custom actions |
| `src/folders.rs` | View and sorting remembered per folder |
| `src/util.rs` | Formatting, naming, bookmarks, helpers |

## Not done yet (ideas)

- Undo for file operations
