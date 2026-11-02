#!/usr/bin/env bash
# Cross-compiles Melodia for 64-bit Windows and packages it in dist/:
#   melodia.exe                                 the app
#   WebView2Loader.dll                          WebView2 loader («Play in this app»)
#   Melodia-<version>-setup.exe                 installer (NSIS)
#   melodia-<version>-windows-x64-portable.zip  portable version: melodia-<version>-portable.exe
#                                               and WebView2Loader.dll
#   SHA256SUMS.txt                              SHA-256 hashes of the installer and the zip
# With FAST=1 (the `build-dev` service), it builds with the `fast` profile (no LTO, incremental) and
# makes only the .exe and the DLL. Use it to try changes, not to distribute. The `build-release`
# service makes all the files.
set -euo pipefail

TARGET=x86_64-pc-windows-gnu
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)"
DIST=dist
PROFILE=release
[[ -n "${FAST:-}" ]] && PROFILE=fast
BUILD="target/$TARGET/$PROFILE"

if [[ -n "${SPOTIFY_CLIENT_ID:-}" ]]; then
    echo "The Spotify Client ID will be embedded in the executable."
fi

echo "Building Melodia $VERSION for $TARGET ($PROFILE)…"
cargo build --profile "$PROFILE" --locked --target "$TARGET"

mkdir -p "$DIST"
cp "$BUILD/melodia.exe" "$DIST/"
# The app imports WebView2Loader.dll (redistributable, from the WebView2 SDK of Microsoft, which
# the webview2-com-sys crate includes). It must be next to the app.
LOADER="$(find "$BUILD/build" -path '*webview2-com-sys-*/out/x64/WebView2Loader.dll' | head -n1)"
if [[ -z "$LOADER" ]]; then
    echo "error: WebView2Loader.dll not found in the build output" >&2
    exit 1
fi
cp "$LOADER" "$DIST/"

imports() { x86_64-w64-mingw32-objdump -p "$1" | sed -n 's/^\s*DLL Name: //p' | sort -fu; }
echo "DLLs imported by melodia.exe: $(imports "$DIST/melodia.exe" | tr '\n' ' ')"
# Only DLLs that Windows includes, and the WebView2 loader that comes with the app.
if imports "$DIST/melodia.exe" | grep -qiE '^(libgcc|libstdc|libwinpthread)'; then
    echo "error: melodia.exe depends on MinGW runtime DLLs" >&2
    exit 1
fi

if [[ "$PROFILE" == fast ]]; then
    echo
    echo "Done (FAST: no installer or zip; any in $DIST/ are from an earlier build):"
    ls -lh "$DIST/melodia.exe" "$DIST/WebView2Loader.dll"
    exit 0
fi

echo "Creating the installer…"
makensis -V2 \
    -DVERSION="$VERSION" \
    -DDIST="$PWD/$DIST" \
    -DICON="$PWD/assets/icon.ico" \
    -DOUTFILE="$PWD/$DIST/Melodia-$VERSION-setup.exe" \
    installer/windows/installer.nsi

# The portable version: the .exe, with the version in its name, and the DLL.
ZIP="melodia-$VERSION-windows-x64-portable.zip"
ZIP_PATH="$PWD/$DIST/$ZIP"
PORTABLE="$(mktemp -d)"
cp "$DIST/melodia.exe" "$PORTABLE/melodia-$VERSION-portable.exe"
cp "$DIST/WebView2Loader.dll" "$PORTABLE/"
rm -f "$ZIP_PATH"
(cd "$PORTABLE" && zip -q -6 "$ZIP_PATH" ./*)
rm -rf "$PORTABLE"

# The hashes of the files to publish. Upload SHA256SUMS.txt with them: users compare it with
# `Get-FileHash <file> -Algorithm SHA256` in PowerShell.
(cd "$DIST" && sha256sum "Melodia-$VERSION-setup.exe" "$ZIP" >SHA256SUMS.txt)

echo
echo "Done:"
ls -lh "$DIST"/*.exe "$DIST"/*.dll "$DIST"/*.zip "$DIST/SHA256SUMS.txt"
cat "$DIST/SHA256SUMS.txt"
