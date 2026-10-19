# Braid logo pack · 2026-10-19

## Approved design
The full-color logo retains the previously reviewed HD-PNG-based vector reconstruction, including editable colored ribbons, gradients and local overlap shading. It is a reconstruction, not the original authoring file; fine grain is omitted. Its SVGs and PNGs are unchanged from the color v2 pack.

The monochrome logo is the user's approved same-form interpretation: original three-ribbon identity, a widened lower-left opening connecting to the right counter, and a naturally curved lower-right opening reaching the exterior. The approved main outline, lower-left opening and final tangent-matched right opening are preserved in `source/approved-monochrome.svg`.

## Included variants and sizes
Eight variants: color on transparent, white, black and original-style navy rounded badge; white on transparent or black; black on transparent or white.

PNG sizes: 16, 20, 24, 32, 48, 64, 128, 256, 512 and 1024 px. PNG filenames identify the actual size.

- `svg/standard`: four color and four approved monochrome masters.
- `svg/small-optical-color`: unchanged color v2 small masters, used for 16–32 px.
- `svg/optical/<size>`: monochrome masters tuned for each size through 128 px. Both approved openings are widened with rounded mask expansion to survive rasterization. Through 32 px the mark is enlarged 8% around its center. These are optical adaptations of the approved form, not alternative logos.
- `png/<variant>`: ready-to-use images.
- `preview.png`: all variants, with small sizes enlarged for inspection.
- `actual-size-preview.png`: monochrome 16–128 px at native pixels.
- `source/original-hd.png`: unchanged original raster reference.

Use the matching optical SVG for small UI icons; use the standard master for large artwork. At 16–24 px, fine curves inevitably become antialiased pixels; the optical versions preserve the broad woven form and open separations, not every large-size detail. The overlap hierarchy is clearer at 32 px and above. Transparent variants use real alpha, including the open ribbon boundaries. Backgrounds are independent SVG rectangles. No fonts, embedded bitmaps or external resources are required by the SVG logos. Color SVGs use gradients, filters and clipping; monochrome SVGs use editable paths and alpha masks. Use a full SVG renderer such as Inkscape.

## Rebuild and checks
Requires Python 3, Pillow, NumPy, SciPy and Inkscape CLI. Run `python build_pack.py`, then `python qa.py`. The color SVGs are frozen inputs; all other inputs are under `source/`. Rebuilding is offline and does not access other project directories. Preview labels use an available local DejaVu font; the logo assets have no font dependencies.

`QA.json` records image dimensions, monochrome alpha/complement checks, small-size region checks and SVG resource inspection. `manifest.json` lists deliverable hashes. No upstream repository was modified. Earlier exploratory candidates are not included.
