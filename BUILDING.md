# Building Voyager

Building Voyager needs Rust (1.85+, Debian 13’s), the GTK 3 development files and gettext (for the translations; without it Voyager is built in English only):

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
./uninstall.sh --purge           # ...and delete the settings in ~/.config/voyager
```

Uninstalling also gives folders back to the application `--default` replaced.

## Packaging Voyager for a distribution

Always build a distribution's package from a release's git tag (`v1.0.0`): a checkout of the tag, or GitHub's tarball of it, built with `cargo build --release --locked` so that the crates are exactly the ones in `Cargo.lock`.

**Don't repackage the prebuilt files from the release page** (the `voyager-<version>-x86_64.tar.gz` tarball, the .deb or the .rpm). A binary can't be checked against the sources it claims to come from, so whoever managed to replace it, on GitHub or on the way to you, would have their code shipped to all of your users: a supply chain attack. Built from the tag, everything in your package can be read and checked in the sources. The prebuilt files are only meant for people installing Voyager on their own computer.

## Arch Linux package

`packaging/arch/PKGBUILD` builds a pacman package from the committed state of this repository:

```sh
cd packaging/arch
makepkg -si
```

The AUR packages are in `packaging/aur/`, each folder with its `PKGBUILD` and `.SRCINFO` as uploaded to the AUR:

- `voyager` builds the latest release from its sources (GitHub's tarball of the tag `v<version>`);
- `voyager-git` builds the newest commit on `master`;
- `voyager-bin` installs the release's prebuilt program (`voyager-<version>-x86_64.tar.gz` on the release page: `voyager`, `voyager.desktop`, `voyager.svg`, the AppStream metainfo, `locale/`, `LICENSE` and `README.md` in a folder `voyager-<version>-x86_64/`).

Any of the three can be installed instead of the others. After changing a `PKGBUILD`, write its `.SRCINFO` again with `makepkg --printsrcinfo > .SRCINFO`.

## Debian and Ubuntu package

`packaging/deb/build.sh` builds a .deb on Debian 13 or newer, or Ubuntu 26.04 or newer, with the distribution's own Rust (needs `cargo rustc libgtk-3-dev gettext pkg-config build-essential dpkg-dev`); its library dependencies are worked out by `dpkg-shlibdeps`. From another machine, `packaging/deb/build-on.sh HOST` copies this checkout with its crates (`cargo vendor`, so the build needs no internet) to `HOST` over SSH with rsync, builds there and fetches the package into `packaging/deb/`:

```sh
packaging/deb/build-on.sh debian    # an ssh host name
sudo apt install ./packaging/deb/voyager_*.deb
```

## Fedora package

`packaging/rpm/voyager.spec` builds an RPM on Fedora (45 tested) with Fedora's own Rust (needs `cargo rust gcc gtk3-devel gettext rpm-build`); `packaging/rpm/build.sh` runs `rpmbuild` in the sources, which works out the library dependencies. As for the .deb, `packaging/rpm/build-on.sh HOST` builds it on another machine and fetches it into `packaging/rpm/` (both use `packaging/remote-build.sh`):

```sh
packaging/rpm/build-on.sh fedora    # an ssh host name
sudo dnf install ./packaging/rpm/voyager-*.rpm
```

## Versions

`Cargo.toml` holds the version, for every package and the About window: `1.0.0-dev` for the builds towards 1.0.0. The packages add the number of commits and the commit, in each format's own way of marking a build before a release, so that they update in order and to the release: `1.0.0dev.r142.g6a7d287` (Arch), `1.0.0~dev.r142.g6a7d287` (Debian and RPM; `packaging/version.sh` writes them). About shows `1.0.0-dev (r142, 6a7d287)`.

A release: set the version in `Cargo.toml` (`1.0.0`), add the release to `data/io.github.HydrAPI.Voyager.metainfo.xml` (`<releases><release version="1.0.0" date="2026-12-01"/></releases>`, newest first; software centres show it, `appstreamcli validate` checks the file), commit, and tag the commit `v1.0.0`; packages built from it are just `1.0.0`. Pushing the tag starts the release workflow (`.github/workflows/release.yml`): it checks that the tag matches `Cargo.toml`, builds the prebuilt program (`packaging/tarball/build.sh`), the .deb (in Debian 13), the .rpm (in Fedora) and the Arch Linux package (in Arch Linux, with `packaging/arch/PKGBUILD`), and puts them with their SHA-256 sums on a draft release, to write its notes and publish. For the AUR, set `pkgver=1.0.0` (and `pkgrel=1`) in `packaging/aur/voyager` and `voyager-bin` once the release and its files are on GitHub, then run `updpkgsums` and `makepkg --printsrcinfo > .SRCINFO` in each. Then the next version with `-dev` (`1.1.0-dev`). Builds after a release without that are numbered after it (`1.0.0.r150…`, `1.0.0+r150…`, `1.0.0^r150…`).

## Prebuilt program

`packaging/tarball/build.sh` builds `target/tarball/voyager-<version>-<arch>.tar.gz`, the program as the release page has it for `voyager-bin`: it runs from its folder, with its translations. It's for installing Voyager on your own computer, not for building a distribution's package from (see [Packaging Voyager for a distribution](#packaging-voyager-for-a-distribution)).

## Optional dependencies

Video thumbnails (`ffmpegthumbnailer` or `ffmpeg`), the Media Info tab (`mediainfo`), phones (`gvfs-mtp`), cameras (`gvfs-gphoto2`), network shares (`gvfs-smb`) and archives (`engrampa`, `file-roller` or `xarchiver`) are optional dependencies.

Trash, network locations and mounting use GVfs, which every MATE install already has. Mounting drives also needs udisks2 (`gvfs` pulls it in on most distributions); Android phones need the GVfs MTP backend (`gvfs-mtp` on Arch, Debian and Fedora).
