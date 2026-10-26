# App icon

The icon is direction b of the design canvas, "the yolk": an egg white with a yolk, and a
❯ in the yolk. The user chose it on 2026-10-26. Each colour is a role of the design
system (`src/theme/palette.ts`). The icon has no gradient, no gloss and no shadow.

## Files

| File                    | What it holds                                                           |
| ----------------------- | ----------------------------------------------------------------------- |
| `icon.png`              | The icon of the app: 1024 x 1024, RGB, no transparency, no corners.     |
| `icon.svg`              | The source of `icon.png`: all layers on one canvas.                     |
| `layer-background.svg`  | Layer 1 for Icon Composer: `surface_1` (#0d111a).                       |
| `layer-egg-white.svg`   | Layer 2: the egg white in `text_bright` (#f2eddd).                      |
| `layer-yolk.svg`        | Layer 3: the yolk in `accent_primary` (#ffc940), with the ❯ in #0d111a. |
| `appearance-clear.svg`  | A reference for the clear appearance of iOS 26. iOS draws it.           |
| `appearance-tinted.svg` | A reference for the tinted appearance of iOS 26. iOS adds the tint.     |

The yolk is a separate layer, so the Liquid Glass of iOS 26 can light it alone.

## Make the PNG again

Do this after a change to `icon.svg`. You need `rsvg-convert` (librsvg) and ImageMagick.

```sh
rsvg-convert -w 1024 -h 1024 -o icon-rgba.png icon.svg
magick icon-rgba.png -background "#0d111a" -alpha remove -alpha off -strip \
  -define png:color-type=2 icon.png
rm icon-rgba.png
```

`src/theme/icon.test.ts` checks the size, that the PNG has no alpha channel, and that
every SVG uses only colours of the design system.

## Icon Composer (on a Mac)

To make an `.icon` file with the iOS 26 appearances, open Icon Composer, put the three
layer files in three groups (background, egg white, yolk), and compare the clear and
tinted results with the two reference files. Then check the icon at 60 pt (home screen)
and at 29 pt (Settings). At 29 pt the ❯ is about 3 px. The user accepts this.
