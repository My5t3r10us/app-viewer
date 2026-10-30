# AppIcon — macOS icon kit

| File | Use it for |
|---|---|
| `AppIcon.icns` | Any Mac app bundle (`Contents/Resources/AppIcon.icns` + `CFBundleIconFile`), Electron (`build/icon.icns` for electron-builder, `packagerConfig.icon` for Forge), Tauri (`bundle.icon`), Xojo, Qt… |
| `AppIcon.appiconset/` | Xcode: replace the folder inside `Assets.xcassets/` (or drag the PNGs into the AppIcon slots). |
| `AppIcon.iconset/` | Rebuild the .icns on a Mac: `iconutil -c icns AppIcon.iconset` |
| `png/icon_1024.png` | Tauri: `npx tauri icon png/icon_1024.png` generates every platform's icons. Also App Store / website. |
| `IconComposer-layers/` | macOS 26+ Liquid Glass: open Icon Composer (ships with Xcode 26+), drop `background` as the background fill/layer and `glyph.svg` (or `glyph3d.png`) as a foreground layer, then save `AppIcon.icon` and add it to the Xcode project. Check the preview: layer scale may need a small tweak. |

Notes
- The PNG/ICNS artwork follows the classic macOS grid (824 px body on a 1024 canvas, transparent margin with a soft shadow). macOS 26+ accepts it and applies its own material; because the silhouette is a proper squircle, it won't be put in the grey "icon jail".
- If you only ship one thing for a native Xcode app targeting macOS 26+, prefer the Icon Composer `.icon`; Xcode derives older-OS images from it.
