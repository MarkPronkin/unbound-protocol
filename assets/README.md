# Unbound Protocol branding

The **Dot Portal** identity is adapted from
[The Unbound Launcher](https://github.com/MarkPronkin/the-unbound-launcher/tree/main/branding),
by MarkPronkin and contributors. Both projects share the dotted open U and its
rising right arm. The Protocol lockup reads **UNBOUND** with **PROTOCOL** below;
the Launcher's small “THE” line is removed.

- `unbound-logo.png`: transparent black horizontal logo for light backgrounds.
- `unbound-logo-dark.svg`: self-contained white version for dark backgrounds.
  It embeds the same PNG and applies a white SVG color filter, preserving its
  exact alpha and geometry.
- `unbound-mark.png`: the original transparent black Dot Portal symbol, copied
  unchanged from the Launcher identity for standalone use.

The README selects the horizontal logo automatically for the viewer's theme.
Original branding and this adaptation are covered by the project's MIT license.
The Unbound Launcher branding is the design reference, not a vendor logo.

## Method and source

The wordmark was adapted with the built-in `image_gen` tool using the
[Launcher horizontal logo](https://github.com/MarkPronkin/the-unbound-launcher/blob/main/branding/logo.png)
as the edit target and its
[app icon](https://github.com/MarkPronkin/the-unbound-launcher/blob/main/branding/app-icon.png)
as the symbol reference. The standalone mark comes from
[the original symbol](https://github.com/MarkPronkin/the-unbound-launcher/blob/main/branding/symbol.png).
The dark SVG follows the same self-contained filter approach as the Launcher.

### Edit prompt

```text
Use case: text-localization.
Asset type: primary horizontal logo for Unbound Protocol.
Input image 1: EDIT TARGET, the existing transparent black horizontal logo for The Unbound Launcher.
Input image 2: REFERENCE ONLY, the current Launcher app icon showing the exact approved Dot Portal symbol in white on black. Its background and square tile are NOT part of the requested output.
Primary request: Adapt the first logo into a sibling logo for UNBOUND PROTOCOL. Preserve its dotted open U, equal circular dots on a regular square grid, two-dot-thick strokes, chamfered lower bowl and taller right arm ending in two single dots. Match the current symbol from image 2 exactly. Preserve the recognizable compact, bold, squared geometric UNBOUND wordmark and its relationship to the icon.
Change only the lettering needed for the sibling project: remove the small word THE entirely; replace LAUNCHER with PROTOCOL, arranged below UNBOUND in smaller, widely spaced uppercase type.
Text verbatim: "UNBOUND" on the main line, "PROTOCOL" below. No other words.
Composition: one balanced horizontal logo, icon at left and wordmark at right, comfortable clear margins, about 3:1 proportions.
Style: pure solid black artwork on a genuinely transparent background. Clean circular dots and sharp typography, professional finished logo. Every space outside the artwork and inside the U remains transparent.
Avoid: redesigning the symbol, extra dots, new motifs, infinity marks, colors, gray texture, gradients, shadows, outlines, 3D effects, mockups, icon tiles, background strips, concept boards or duplicate logos.
```
