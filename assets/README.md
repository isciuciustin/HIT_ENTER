# Brand assets

`logo.svg` is the source of truth for the app icon: a return arrow (the key you
hit) in the accent green on the app's ink background.

Regenerate the platform icon set after editing it:

```bash
rsvg-convert -w 1024 -h 1024 assets/logo.svg -o /tmp/icon-source.png
npx @tauri-apps/cli@2 icon /tmp/icon-source.png -o src-tauri/icons
rm -rf src-tauri/icons/android src-tauri/icons/ios   # mobile is out of scope
```
