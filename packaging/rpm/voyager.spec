# voyager.spec: Voyager's RPM, for Fedora (45 tested). Built by build.sh in a
# copy of the sources (rpmbuild --build-in-place, no source tarball), the
# crates in vendor/ when build-on.sh made it; voyager_version comes from
# there too. The library dependencies are found by rpmbuild.

# Cargo strips the program (see Cargo.toml): nothing to split off.
%global debug_package %{nil}
# No %changelog to take the files' dates from (the git history is it).
%global source_date_epoch_from_changelog 0

Name:           voyager
Version:        %{voyager_version}
Release:        1%{?dist}
Summary:        Fast graphical file manager
License:        GPL-3.0-or-later

BuildRequires:  cargo >= 1.85
BuildRequires:  rust >= 1.85
BuildRequires:  gcc
BuildRequires:  gtk3-devel
BuildRequires:  gettext

# Trash, network places, metadata (notes, emblems).
Recommends:     gvfs
# Thumbnailers run in a sandbox (without it, as they are).
Recommends:     bubblewrap
Suggests:       gvfs-smb
Suggests:       gvfs-mtp
Suggests:       gvfs-gphoto2
Suggests:       ffmpegthumbnailer
Suggests:       mediainfo
Suggests:       file-roller

%description
Voyager is a file manager for GTK desktops: icon, list, compact and columns
views, tabs and split view, an info panel, bulk renaming, checksums,
checked copies, custom actions and thumbnailers, and undo for file
operations. It uses the desktop's GTK theme, icons and bookmarks, and
shares notes, emblems and custom icons with Caja.

%build
cargo build --release --locked %{?voyager_offline}

%install
install -Dm755 target/release/voyager %{buildroot}%{_bindir}/voyager
install -Dm644 voyager.desktop %{buildroot}%{_datadir}/applications/voyager.desktop
install -Dm644 data/voyager.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/voyager.svg
install -Dm644 data/io.github.HydrAPI.Voyager.metainfo.xml %{buildroot}%{_metainfodir}/io.github.HydrAPI.Voyager.metainfo.xml
# The translations build.rs made from po/*.po.
for mo in target/release/locale/*/LC_MESSAGES/voyager.mo; do
    [ -e "$mo" ] && install -Dm644 "$mo" "%{buildroot}%{_datadir}/locale/${mo#target/release/locale/}"
done
%find_lang %{name}

%files -f %{name}.lang
%license LICENSE
%doc README.md
%{_bindir}/voyager
%{_datadir}/applications/voyager.desktop
%{_datadir}/icons/hicolor/scalable/apps/voyager.svg
%{_metainfodir}/io.github.HydrAPI.Voyager.metainfo.xml
