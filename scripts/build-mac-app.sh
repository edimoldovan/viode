#!/usr/bin/env bash
# Build the self-contained Viode.app and its .dmg. macOS only.
#
# The rule this serves (CLAUDE.md, "Release rules"): a Mac user
# downloads ONE file, drags Viode to Applications, and never installs
# anything else. So the bundle carries the entire engine: GStreamer
# with every plugin Viode uses (including soundtouch, which brew
# omits — built here from the matching gst-plugins-bad source),
# ffmpeg and ffprobe, whisper-cli, the speech and face models. The
# binary finds all of it by itself (adopt_bundle_engine in viode-cli).
#
# Engine completeness vs. iteration speed: the vidstab-enabled ffmpeg
# from the homebrew-ffmpeg tap builds from source (slow). CI runs on
# every push use brew's core ffmpeg and skip only that one piece;
# FULL_ENGINE=1 (used for the shipped .dmg) does the real swap. The
# acceptance for a shipped bundle is all 19 doctor checks green on a
# Mac with no Homebrew.
#
# Env:
#   VERSION       version label for the dmg (default: Cargo.toml)
#   SIGN_IDENTITY codesign identity (default: "-", ad-hoc)
#   FULL_ENGINE=1 require the vidstab ffmpeg (refuses core ffmpeg)
#   SKIP_MODELS=1 leave the models out (fast CI iteration)
set -euo pipefail
cd "$(dirname "$0")/.."

[[ "$(uname)" == "Darwin" ]] || { echo "macOS only" >&2; exit 1; }
BREW="$(brew --prefix)"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
APP=dist/Viode.app
C="$APP/Contents"

say() { printf '\n== %s\n' "$*"; }

# ---------------------------------------------------------------- build
say "release binary"
cargo build --release --locked -p viode-cli

say "bundle skeleton"
rm -rf dist && mkdir -p "$C"/{MacOS,Frameworks/gstreamer-1.0,Frameworks/lib,Resources/models,Helpers}
cp target/release/viode "$C/MacOS/viode"

cat > "$C/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.eduardmoldovan.viode</string>
  <key>CFBundleName</key><string>Viode</string>
  <key>CFBundleDisplayName</key><string>Viode</string>
  <key>CFBundleExecutable</key><string>viode</string>
  <key>CFBundleIconFile</key><string>viode</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array><dict>
    <key>CFBundleTypeName</key><string>Viode project</string>
    <key>CFBundleTypeRole</key><string>Editor</string>
    <key>LSItemContentTypes</key><array><string>com.eduardmoldovan.viode.project</string></array>
  </dict></array>
  <key>UTExportedTypeDeclarations</key>
  <array><dict>
    <key>UTTypeIdentifier</key><string>com.eduardmoldovan.viode.project</string>
    <key>UTTypeDescription</key><string>Viode project</string>
    <key>UTTypeConformsTo</key><array><string>public.data</string></array>
    <key>UTTypeTagSpecification</key>
    <dict><key>public.filename-extension</key><array><string>viode</string></array></dict>
  </dict></array>
</dict>
</plist>
PLIST

say "icon"
ICONSET=dist/viode.iconset
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" packaging/icons/viode.svg -o "$ICONSET/icon_${size}x${size}.png"
    rsvg-convert -w "$((size * 2))" -h "$((size * 2))" packaging/icons/viode.svg -o "$ICONSET/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$C/Resources/viode.icns"

# ------------------------------------------------------------- soundtouch
# Brew's gstreamer omits the soundtouch plugin; speed changes need it.
# Build exactly the one plugin from the source matching the installed
# GStreamer (the docs/macos-bootstrap.md recipe, ~1 minute).
GST_VERSION="$(gst-launch-1.0 --version | sed -n 's/.*version \([0-9.]*\).*/\1/p' | head -1)"
SOUNDTOUCH_DYLIB="cache/mac-engine/gst-plugins-bad-$GST_VERSION/build/ext/soundtouch/libgstsoundtouch.dylib"
if [[ ! -f "$SOUNDTOUCH_DYLIB" ]]; then
    say "building libgstsoundtouch for GStreamer $GST_VERSION"
    mkdir -p cache/mac-engine
    curl -sL "https://gstreamer.freedesktop.org/src/gst-plugins-bad/gst-plugins-bad-$GST_VERSION.tar.xz" \
        | tar -xJ -C cache/mac-engine
    (cd "cache/mac-engine/gst-plugins-bad-$GST_VERSION" &&
        meson setup build -Dauto_features=disabled -Dsoundtouch=enabled > /dev/null &&
        ninja -C build ext/soundtouch/libgstsoundtouch.dylib > /dev/null)
fi

# ---------------------------------------------------------------- ffmpeg
if [[ "${FULL_ENGINE:-}" == "1" ]]; then
    ffmpeg -hide_banner -filters 2> /dev/null | grep -q vidstabtransform || {
        echo "FULL_ENGINE=1 but ffmpeg lacks vidstab. Install the tap build:" >&2
        echo "  brew uninstall --ignore-dependencies ffmpeg" >&2
        echo "  brew tap homebrew-ffmpeg/ffmpeg" >&2
        echo "  brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvidstab" >&2
        exit 1
    }
fi

# ---------------------------------------------------------------- models
if [[ "${SKIP_MODELS:-}" != "1" ]]; then
    say "models"
    curl -sL -o "$C/Resources/models/ggml-base.en.bin" \
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
    curl -sL -o "$C/Resources/models/seeta_fd_frontal_v1.0.bin" \
        "https://github.com/atomashpolskiy/rustface/raw/master/model/seeta_fd_frontal_v1.0.bin"
fi

# ------------------------------------------------------------ relocation
say "collecting the engine"
# brew's plugin dir holds symlinks into the Cellar, some dangling
# (plugins whose backing formula is not installed). Resolve the live
# ones, skip the dead ones.
for plugin in "$BREW/lib/gstreamer-1.0/"*.dylib; do
    [[ -e "$plugin" ]] || continue
    cp -L "$plugin" "$C/Frameworks/gstreamer-1.0/"
done
cp "$SOUNDTOUCH_DYLIB" "$C/Frameworks/gstreamer-1.0/"
for helper in ffmpeg ffprobe; do
    cp "$(command -v $helper)" "$C/Helpers/$helper"
done
if command -v whisper-cli > /dev/null; then
    cp "$(command -v whisper-cli)" "$C/Helpers/whisper-cli"
fi
SCANNER="$BREW/libexec/gstreamer-1.0/gst-plugin-scanner"
[[ -f "$SCANNER" ]] && cp "$SCANNER" "$C/Helpers/gst-plugin-scanner"

# Breadth-first crawl: every Mach-O in the bundle pulls the brew
# libraries it references into Frameworks/lib until the set is closed.
deps_of() { otool -L "$1" | awk 'NR>1 {print $1}' | grep -E "^($BREW|/usr/local)" || true; }
say "crawling dylib dependencies"
while :; do
    added=0
    while IFS= read -r -d '' macho; do
        while IFS= read -r dep; do
            [[ -z "$dep" ]] && continue
            leaf="$(basename "$dep")"
            if [[ ! -e "$C/Frameworks/lib/$leaf" ]]; then
                cp "$(readlink -f "$dep")" "$C/Frameworks/lib/$leaf"
                chmod u+w "$C/Frameworks/lib/$leaf"
                added=1
            fi
        done < <(deps_of "$macho")
    done < <(find "$C" -type f \( -perm -111 -o -name '*.dylib' \) -print0)
    [[ "$added" == "0" ]] && break
done

say "rewriting install names"
rewrite() {
    local macho="$1"
    chmod u+w "$macho"
    if [[ "$macho" == *.dylib ]]; then
        install_name_tool -id "@rpath/$(basename "$macho")" "$macho" 2> /dev/null
    fi
    while IFS= read -r dep; do
        [[ -z "$dep" ]] && continue
        install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$macho" 2> /dev/null
    done < <(deps_of "$macho")
    # Helpers and the main binary sit one level below Contents; the
    # same relative rpath serves both. Libraries and plugins resolve
    # through the loading process but get a loader-relative rpath too,
    # so the scanner (its own process) can load plugins alone.
    install_name_tool -add_rpath "@executable_path/../Frameworks/lib" "$macho" 2> /dev/null || true
    install_name_tool -add_rpath "@loader_path/../lib" "$macho" 2> /dev/null || true
    install_name_tool -add_rpath "@loader_path" "$macho" 2> /dev/null || true
    codesign --force -s "$SIGN_IDENTITY" "$macho" 2> /dev/null
}
while IFS= read -r -d '' macho; do
    # Plain `grep && rewrite` would return 1 on every non-Mach-O file
    # and set -e would kill the whole build on the first plist.
    if file "$macho" | grep -q Mach-O; then
        rewrite "$macho"
    fi
done < <(find "$C" -type f -print0)

say "self-containment check"
leaks=0
while IFS= read -r -d '' macho; do
    file "$macho" | grep -q Mach-O || continue
    if otool -L "$macho" | awk 'NR>1 {print $1}' | grep -E "^($BREW|/usr/local)" > /dev/null; then
        echo "LEAK in $macho:" >&2
        otool -L "$macho" | grep -E "$BREW|/usr/local" >&2
        leaks=1
    fi
done < <(find "$C" -type f -print0)
[[ "$leaks" == "0" ]] || { echo "bundle still references Homebrew — not self-contained" >&2; exit 1; }

say "signing the bundle"
codesign --force -s "$SIGN_IDENTITY" "$APP"

say "smoke: the bundled binary runs its doctor"
"$C/MacOS/viode" --version
# The bundle sets GST_PLUGIN_SYSTEM_PATH to itself, so this doctor run
# exercises the BUNDLED plugins even on a machine that has brew.
"$C/MacOS/viode" doctor

say "dmg"
STAGE=dist/dmg
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname Viode -srcfolder "$STAGE" -format UDZO "dist/Viode-$VERSION.dmg"
rm -rf "$STAGE" "$ICONSET"
echo
echo "built dist/Viode.app and dist/Viode-$VERSION.dmg"
