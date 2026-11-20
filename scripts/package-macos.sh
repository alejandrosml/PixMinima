#!/bin/sh
# Build, sign and optionally notarize the application and its disk image.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$project_dir"
: "${MACOSX_DEPLOYMENT_TARGET:=27.0}"
export MACOSX_DEPLOYMENT_TARGET
build_dir=${CARGO_TARGET_DIR:-"$project_dir/target"}
destination=${PIXMINIMA_DIST:-"$project_dir/dist"}
mkdir -p "$destination"
if { [ -n "${NOTARY_PROFILE:-}" ] || [ -n "${NOTARY_KEY_FILE:-}" ]; } && [ -z "${CODESIGN_IDENTITY:-}" ]; then
    echo "Set a Developer ID Application signing identity for notarization." >&2
    exit 1
fi
staging=$(mktemp -d "${TMPDIR:-/tmp}/pixminima-package.XXXXXXXX")
trap 'rm -rf -- "$staging"' EXIT
app="$staging/PixMinima.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Helpers" "$contents/Resources"
cargo build --release --locked --package pixminima --package pixminima-desktop --features pixminima/metal
version=$(plutil -extract CFBundleShortVersionString raw packaging/macos/Info.plist)
if [ "$("$build_dir/release/pixminima" --version)" != "PixMinima $version" ]; then
    echo "The Cargo version and the Mac bundle version must agree." >&2
    exit 1
fi
cp "$build_dir/release/pixminima-desktop" "$contents/MacOS/PixMinima"
cp "$build_dir/release/pixminima" "$contents/Helpers/pixminima"
cp packaging/macos/Info.plist "$contents/Info.plist"
cp LICENSE "$contents/Resources/LICENSE"
cp docs/THIRD-PARTY.txt "$contents/Resources/"
python3 scripts/make-icon.py "$staging/PixMinima.iconset"
iconutil -c icns -o "$contents/Resources/PixMinima.icns" "$staging/PixMinima.iconset"
plutil -lint "$contents/Info.plist"
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp --sign "$CODESIGN_IDENTITY" "$contents/Helpers/pixminima"
    codesign --force --options runtime --timestamp --sign "$CODESIGN_IDENTITY" "$app"
else
    codesign --force --sign - "$contents/Helpers/pixminima"
    codesign --force --sign - "$app"
fi
codesign --verify --deep --strict "$app"
arch=$(uname -m)
name="PixMinima-$version-macOS-$arch.dmg"
disk="$staging/$name"
if [ -e "$destination/$name" ] || [ -e "$destination/PixMinima.app" ]; then
    echo "Choose a fresh PIXMINIMA_DIST directory." >&2
    exit 1
fi
notarize() {
    archive=$1
    if [ -n "${NOTARY_PROFILE:-}" ]; then
        xcrun notarytool submit "$archive" --keychain-profile "$NOTARY_PROFILE" --wait --output-format json > "$staging/notary.json"
    else
        : "${NOTARY_KEY_ID:?Set the key identifier}" "${NOTARY_ISSUER:?Set the team API key issuer}"
        xcrun notarytool submit "$archive" --key "$NOTARY_KEY_FILE" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER" --wait --output-format json > "$staging/notary.json"
    fi
    status=$(plutil -extract status raw "$staging/notary.json")
    if [ "$status" != Accepted ]; then
        cat "$staging/notary.json" >&2
        exit 1
    fi
    echo "Apple accepted $(basename "$archive")."
}
if [ -n "${NOTARY_PROFILE:-}" ] || [ -n "${NOTARY_KEY_FILE:-}" ]; then
    ditto -c -k --keepParent "$app" "$staging/PixMinima.zip"
    notarize "$staging/PixMinima.zip"
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
fi
mkdir "$staging/disk"
ditto "$app" "$staging/disk/PixMinima.app"
ln -s /Applications "$staging/disk/Applications"
hdiutil create -quiet -volname PixMinima -srcfolder "$staging/disk" -format UDZO "$disk"
if [ -n "${CODESIGN_IDENTITY:-}" ]; then codesign --timestamp --sign "$CODESIGN_IDENTITY" "$disk"; fi
if [ -n "${NOTARY_PROFILE:-}" ] || [ -n "${NOTARY_KEY_FILE:-}" ]; then
    notarize "$disk"
    xcrun stapler staple "$disk"
    xcrun stapler validate "$disk"
fi
ditto "$app" "$destination/PixMinima.app"
ditto "$disk" "$destination/$name"
(cd "$destination" && shasum -a 256 "$name" > "$name.sha256")
echo "$destination/$name"
