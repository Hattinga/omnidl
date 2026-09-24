#!/bin/sh
# Builds the macOS release (Apple Silicon + Intel in one universal binary) into dist/:
#   omnidl-<version>-macos.dmg                   omnidl.app, to drag into Applications
#   omnidl-cli-<version>-macos-universal.tar.gz  terminal program and web interface
#   omnidl-macos-universal, omnidl-cli-macos-universal   plain programs for the in-app updater
#
# Runs on macOS with the Xcode command line tools and rustup:
#   packaging/macos/bundle.sh [--no-build]
# Signing is ad hoc (codesign -s -): enough for Apple Silicon to run it; not notarized.
set -eu

cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
targets="aarch64-apple-darwin x86_64-apple-darwin"
bundle_id=io.github.hattinga.omnidl
export MACOSX_DEPLOYMENT_TARGET=11.0

if [ "${1:-}" != --no-build ]; then
    for t in $targets; do
        rustup target add "$t" >/dev/null
        cargo build --release --locked --bins --target "$t"
    done
fi

out=target/universal
rm -rf "$out"
mkdir -p "$out" dist
for program in omnidl omnidl-cli; do
    lipo -create -output "$out/$program" \
        "target/aarch64-apple-darwin/release/$program" "target/x86_64-apple-darwin/release/$program"
    case $program in
        omnidl) id=$bundle_id ;;
        *) id=$bundle_id.cli ;;
    esac
    codesign --force --sign - --identifier "$id" "$out/$program"
done
lipo -info "$out/omnidl" "$out/omnidl-cli"
cp "$out/omnidl" dist/omnidl-macos-universal
cp "$out/omnidl-cli" dist/omnidl-cli-macos-universal

# omnidl.app
app="$out/omnidl.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' >"$app/Contents/PkgInfo"
cp "$out/omnidl" "$app/Contents/MacOS/omnidl"

# Icon: all sizes from the 1024 px master (with the margin macOS icons have).
iconset="$out/omnidl.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" packaging/macos/icon-1024.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" packaging/macos/icon-1024.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/omnidl.icns"

codesign --force --sign - "$app"
codesign --verify --strict --verbose=2 "$app"

# Disk image with a shortcut to Applications.
stage="$out/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
dmg="dist/omnidl-$version-macos.dmg"
rm -f "$dmg"
# hdiutil occasionally reports "Resource busy" on CI machines; try again.
for attempt in 1 2 3 4 5; do
    if hdiutil create -volname omnidl -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"; then
        break
    fi
    [ "$attempt" = 5 ] && exit 1
    sleep 5
done

tar -czf "dist/omnidl-cli-$version-macos-universal.tar.gz" -C "$out" omnidl-cli -C "$PWD" LICENSE

ls -l dist
