#!/bin/sh
# Installs omnidl from GitHub releases on Linux (x86_64, aarch64).
#
#   curl -fsSL https://github.com/Hattinga/omnidl/releases/latest/download/install.sh | sh
#   ... | sh -s -- --desktop          also the desktop app, with menu entry and omnidl:// handler
#   ... | sudo sh                     for all users, into /usr/local/bin
#
# Options:
#   --desktop        install the desktop app as well (default: only omnidl-cli)
#   --version X.Y.Z  a specific release instead of the newest
#   --bin-dir DIR    where the programs go (default ~/.local/bin, as root /usr/local/bin)
#
# Every download is checked against its published SHA-256 before anything is
# installed. Running it again updates in place.
set -eu

REPO=Hattinga/omnidl
desktop=0
version=
bindir=

die() {
    echo "omnidl: $*" >&2
    exit 1
}

usage() {
    echo "omnidl-Installer für Linux"
    echo "  --desktop        auch die Desktop-App (mit Menüeintrag und omnidl://)"
    echo "  --version X.Y.Z  eine bestimmte Version statt der neuesten"
    echo "  --bin-dir DIR    Zielordner (Standard ~/.local/bin, als root /usr/local/bin)"
}

while [ $# -gt 0 ]; do
    case $1 in
        --desktop) desktop=1 ;;
        --version) [ $# -ge 2 ] || die "--version braucht eine Versionsnummer"; version=${2#v}; shift ;;
        --bin-dir) [ $# -ge 2 ] || die "--bin-dir braucht einen Ordner"; bindir=$2; shift ;;
        -h | --help) usage; exit 0 ;;
        *) die "unbekannte Option: $1 (--desktop, --version, --bin-dir)" ;;
    esac
    shift
done

[ "$(uname -s)" = Linux ] || die "dieses Skript ist für Linux. macOS und Windows: https://github.com/$REPO/releases"
case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "für $(uname -m) gibt es keine fertigen Programme; bauen mit: cargo install --git https://github.com/$REPO --no-default-features --features cli --bin omnidl-cli" ;;
esac

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q -O "$2" "$1"; }
else
    die "curl oder wget wird gebraucht"
fi
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    die "sha256sum wird gebraucht (Paket coreutils)"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

if [ -z "$version" ]; then
    fetch "https://api.github.com/repos/$REPO/releases/latest" "$tmp/release.json" || die "GitHub nicht erreichbar"
    version=$(sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' "$tmp/release.json" | head -n 1)
    [ -n "$version" ] || die "keine Version gefunden"
fi

name="omnidl-$version-linux-$arch"
# OMNIDL_INSTALL_BASE: another download folder, for testing the script.
url="${OMNIDL_INSTALL_BASE:-https://github.com/$REPO/releases/download/v$version}/$name.tar.gz"
echo "omnidl $version ($arch) wird geladen ..."
fetch "$url" "$tmp/$name.tar.gz" || die "Download fehlgeschlagen: $url"
fetch "$url.sha256" "$tmp/$name.tar.gz.sha256" || die "Prüfsumme fehlt: $url.sha256"
expected=$(cut -d ' ' -f 1 "$tmp/$name.tar.gz.sha256")
[ "$(sha256 "$tmp/$name.tar.gz")" = "$expected" ] || die "Prüfsumme stimmt nicht, nichts installiert"
tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
src="$tmp/$name"

if [ "$(id -u)" = 0 ]; then
    [ -n "$bindir" ] || bindir=/usr/local/bin
    share=/usr/local/share
else
    [ -n "$bindir" ] || bindir="$HOME/.local/bin"
    share="${XDG_DATA_HOME:-$HOME/.local/share}"
fi
mkdir -p "$bindir"

# Copy next to the target, then rename: works while the old version runs.
put() {
    cp "$1" "$2.new"
    chmod "$3" "$2.new"
    mv -f "$2.new" "$2"
}

put "$src/omnidl-cli" "$bindir/omnidl-cli" 755
echo "  $bindir/omnidl-cli"

if [ "$desktop" = 1 ]; then
    put "$src/omnidl" "$bindir/omnidl" 755
    mkdir -p "$share/applications" "$share/icons/hicolor/512x512/apps"
    put "$src/omnidl.png" "$share/icons/hicolor/512x512/apps/omnidl.png" 644
    sed "s|^Exec=omnidl|Exec=$bindir/omnidl|" "$src/omnidl.desktop" >"$tmp/omnidl.desktop"
    put "$tmp/omnidl.desktop" "$share/applications/omnidl.desktop" 644
    update-desktop-database "$share/applications" >/dev/null 2>&1 || true
    gtk-update-icon-cache -q "$share/icons/hicolor" >/dev/null 2>&1 || true
    echo "  $bindir/omnidl (im Anwendungsmenü)"
fi

case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "Hinweis: $bindir ist nicht im PATH. Zum Beispiel in ~/.profile ergänzen: export PATH=\"$bindir:\$PATH\"" ;;
esac
echo "Fertig. Los geht's mit: omnidl-cli --help"
