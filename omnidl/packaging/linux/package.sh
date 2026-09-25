#!/bin/sh
# Packs the Linux release for this machine's processor into dist/:
#   omnidl-<version>-linux-<arch>.tar.gz    both programs, desktop entry, icon, README
#   omnidl_<version>_<debarch>.deb          desktop app
#   omnidl-cli_<version>_<debarch>.deb      terminal program + web service (systemd)
#   omnidl-linux-<arch>, omnidl-cli-linux-<arch>   plain programs for the in-app updater
#
# Build first (the terminal program without graphics libraries):
#   cargo build --release --locked --bin omnidl
#   cargo build --release --locked --no-default-features --features cli --bin omnidl-cli
# The .deb files need cargo-deb (cargo install cargo-deb); without it they are skipped.
#   packaging/linux/package.sh
set -eu

cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
case $(uname -m) in
    x86_64 | amd64) arch=x86_64 debarch=amd64 ;;
    aarch64 | arm64) arch=aarch64 debarch=arm64 ;;
    *) echo "Unbekannter Prozessor: $(uname -m)" >&2; exit 1 ;;
esac
bin=${CARGO_TARGET_DIR:-target}/release
for program in omnidl omnidl-cli; do
    [ -x "$bin/$program" ] || { echo "$bin/$program fehlt, erst bauen" >&2; exit 1; }
done

mkdir -p dist
cp "$bin/omnidl" "dist/omnidl-linux-$arch"
cp "$bin/omnidl-cli" "dist/omnidl-cli-linux-$arch"

name="omnidl-$version-linux-$arch"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
cp "$bin/omnidl" "$bin/omnidl-cli" packaging/linux/omnidl.desktop packaging/linux/README.txt LICENSE "$stage/$name/"
cp assets/icon-512.png "$stage/$name/omnidl.png"
chmod 755 "$stage/$name/omnidl" "$stage/$name/omnidl-cli"
chmod 644 "$stage/$name/omnidl.desktop" "$stage/$name/README.txt" "$stage/$name/LICENSE" "$stage/$name/omnidl.png"
tar -C "$stage" --owner=0 --group=0 -czf "dist/$name.tar.gz" "$name"

if cargo deb --version >/dev/null 2>&1; then
    cargo deb --no-build --no-strip --output "dist/omnidl_${version}_$debarch.deb"
    cargo deb --no-build --no-strip --variant cli --output "dist/omnidl-cli_${version}_$debarch.deb"
else
    echo "cargo-deb fehlt, keine .deb-Pakete" >&2
fi

ls -l dist
