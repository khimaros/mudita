#!/usr/bin/env bash
# render the icon (icons/*.svg) to the sizes the web app and the linux app
# use. the results are checked in, so this runs only when the drawing
# changes, and inkscape is needed only then.
set -euo pipefail
cd "$(dirname "$0")/.."

render() {
  inkscape "icons/$1.svg" --export-type=png --export-width="$2" \
    --export-filename="$3" > /dev/null
  echo "wrote $3"
}

render mudita 32 web/favicon.png
render mudita 192 web/icons/Icon-192.png
render mudita 512 web/icons/Icon-512.png
render mudita-maskable 192 web/icons/Icon-maskable-192.png
render mudita-maskable 512 web/icons/Icon-maskable-512.png
render mudita 256 linux/runner/resources/mudita.png
