#!/bin/sh
# Install Ngwa for the current user: the app, its icon, and a menu entry.
set -e
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" \
         "$HOME/.local/share/icons/hicolor/256x256/apps"
install -m 755 "$here/ngwa" "$HOME/.local/bin/ngwa"
install -m 644 "$here/ngwa.png" "$HOME/.local/share/icons/hicolor/256x256/apps/ngwa.png"
install -m 644 "$here/ngwa.desktop" "$HOME/.local/share/applications/ngwa.desktop"
echo "Installed. Open Ngwa from your app menu, or run: ngwa"
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "Note: add ~/.local/bin to your PATH to run ngwa from a terminal." ;;
esac
