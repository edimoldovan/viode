#!/usr/bin/env bash
# Dev-install the Linux desktop integration into ~/.local/share so the
# launcher shows Viode without a package: icons, the .desktop entry, and
# the project-file MIME type. The packages (AUR/.deb/.rpm) install the
# same files under /usr/share instead. Requires `viode` on PATH (the
# .desktop entry launches it by name). Undo with --uninstall.
set -euo pipefail
cd "$(dirname "$0")/.."

APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor"
MIME="$HOME/.local/share/mime"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$APPS/viode.desktop" "$MIME/packages/viode.xml"
    find "$ICONS" -name 'viode.png' -delete 2> /dev/null || true
    rm -f "$ICONS/scalable/apps/viode.svg"
else
    command -v viode > /dev/null || {
        echo "viode is not on PATH — the launcher entry would not start" >&2
        exit 1
    }
    ./scripts/gen-icons.sh
    mkdir -p "$APPS" "$MIME/packages"
    cp -r packaging/icons/hicolor/. "$ICONS/"
    # The launcher runs with the session PATH, not your shell's — a dev
    # build of viode usually is not on it. Bake the absolute path into
    # the installed entry; the packages install to /usr/bin and keep the
    # plain Exec=viode.
    BIN="$(command -v viode)"
    sed "s|^Exec=viode|Exec=$BIN|; s|^TryExec=viode|TryExec=$BIN|" \
        packaging/linux/viode.desktop > "$APPS/viode.desktop"
    cp packaging/linux/viode-mime.xml "$MIME/packages/viode.xml"
fi

command -v update-mime-database > /dev/null && update-mime-database "$MIME" > /dev/null
command -v update-desktop-database > /dev/null && update-desktop-database "$APPS" > /dev/null
echo "done"
