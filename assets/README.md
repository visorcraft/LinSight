<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# assets/

Master imagery for LinSight. These are the canonical sources — every
other reproduction of the logo across the repository (the FreeDesktop
hicolor tree under `packaging/icons/`, the GUI window-icon resources at
`apps/linsight-gui/resources/`, packaging recipes, the social card) is a
**derived copy** that must trace back to the files here.

| File | Size | Purpose |
| ---- | ---- | ------- |
| `LinSight.svg` | scalable | Source-of-truth vector (the app icon). Render to any size with `rsvg-convert -w <px>`. |
| `social-card.svg` | 1024×512 | Source-of-truth vector for the GitHub banner; embeds the icon plus the wordmark and tagline. |
| `LinSight.png` | 1024×1024 | Master raster — high-resolution PNG for docs, slide decks, the README hero, and any consumer that cannot read SVG. |
| `LinSight.ico` | 16/32/48/64/128/256 | Multi-resolution Windows-style icon, used for GitHub repo display and any tooling that prefers `.ico` (favicons, etc.). |
| `social-1024x512.png` | 1024×512 | GitHub social preview / OpenGraph card. Upload via **Settings → Social preview** on github.com. |

## Regenerating

Edit `LinSight.svg` and/or `social-card.svg`, then re-run the render
script — it rewrites every derived artifact in one pass:

```sh
./scripts/dev_render_icons.sh
```

It needs `rsvg-convert` (librsvg) and `magick` (ImageMagick 7), and it
produces:

- `packaging/icons/<size>x<size>/apps/com.visorcraft.LinSight.png`
  (16, 24, 32, 48, 64, 96, 128, 192, 256, 512) + the scalable SVG copy
- `apps/linsight-gui/resources/linsight-{32,64,128,256,512}.png`
  + `linsight.svg` (compiled into the GUI binary via the Qt resource
  bundle in `apps/linsight-gui/build.rs`)
- `assets/LinSight.png`, `assets/LinSight.ico`, `assets/social-1024x512.png`

## Where the per-distro icons live

The FreeDesktop hicolor icon tree at
`packaging/icons/<size>x<size>/apps/com.visorcraft.LinSight.png`
(plus the scalable SVG copy) is dictated by spec and referenced by every
packaging recipe (`packaging/arch/PKGBUILD.local`,
`packaging/arch-v3/PKGBUILD.local`, `packaging/debian/rules`,
`packaging/fedora/linsight.spec`, `packaging/opensuse/linsight.spec`,
`packaging/appimage/AppImageBuilder.yml`,
`packaging/flatpak/com.visorcraft.LinSight.yml`).
Do **not** move those files; if the master art changes, just re-run the
render script so the existing paths are rewritten in place and packaging
stays intact.
