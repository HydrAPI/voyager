## Downloads

- **Debian 13 or newer, Ubuntu 26.04 or newer:** the `.deb`, installed with `sudo apt install ./arkha_*.deb`
- **Fedora:** the `.rpm`, installed with `sudo dnf install ./arkha-*.rpm`
- **Arch Linux:** `arkha`, `arkha-bin` or `arkha-git` from the AUR
- **Other distributions:** `arkha-<version>-x86_64.tar.gz`, the program to run from its folder (it needs GTK 3 and GVfs)

`SHA256SUMS` has the files' checksums.

## For packagers

Please build your distribution's package from this release's git tag (or GitHub's source tarball of it), with `cargo build --release --locked`. **Don't repackage the prebuilt files above:** a binary can't be checked against its sources, so anyone who replaced it could ship their code to all of your users (a supply chain attack). See [Packaging Arkha for a distribution](https://github.com/dekart811/arkha/blob/master/BUILDING.md#packaging-arkha-for-a-distribution).
