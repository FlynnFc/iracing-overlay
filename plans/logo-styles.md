# Manufacturer logo styles — every cardog variant, chosen in Settings

The Standings and Relative widgets put a manufacturer mark beside each car.
Today that mark is one SVG per brand in `assets/logos/`, and which of the
upstream set's eight variants each brand got was a hand pick made file by
file — BMW and Porsche are the colour roundel and shield, Aston Martin and
Pagani the wide badge, GMC and Kia the wordmark, most of the rest the white
icon. `assets/logos/README.md` says they are all the "Icon Dark" variant;
they aren't, and that is why the set looks inconsistent next to the upstream
gallery. This is the spec for shipping every variant and letting the driver
choose, with the current picks kept as the default so nothing changes until
someone asks it to.

## Table of contents

- [What upstream provides](#what-upstream-provides)
- [Decisions](#decisions)
- [Files on disk](#files-on-disk)
- [The manifest](#the-manifest)
- [Config](#config)
- [Resolution](#resolution)
- [Settings page](#settings-page)
- [Slot sizes](#slot-sizes)
- [Edge cases](#edge-cases)
- [Build order](#build-order)
- [Out of scope](#out-of-scope)

---

## What upstream provides

[cardog-ai/icons](https://github.com/cardog-ai/icons) (MIT) has 51 brands ×
8 files, every one a 512 × 512 box with the mark letterboxed inside, so
they all sit at the same scale. The eight are four *shapes* in two
*inks*:

| Upstream name | Ours | What it is |
| --- | --- | --- |
| `Icon Dark` | `icon` | The emblem alone, white |
| `Icon` | `icon-colour` | The emblem alone, brand colours (or black) |
| `Logo Dark` | `badge` | Emblem over the wordmark, white |
| `Logo` | `badge-colour` | Same, brand colours |
| `Logo Horizontal Dark` | `horizontal` | Emblem beside the wordmark, white |
| `Logo Horizontal` | `horizontal-colour` | Same, brand colours |
| `Wordmark Dark` | `wordmark` | The name alone, white |
| `Wordmark` | `wordmark-colour` | Same, brand colours (or black) |

"Dark" upstream means *for dark backgrounds*, which is every one of these
files being `fill="white"` — that is why `ui::logos` can tint them. We call
that ink **mono**, because "dark" would read as the opposite of what it is.

The colour files are the catch. They were drawn for white pages, so a brand
whose colour is black or navy (Audi's rings, Chevrolet's bowtie, Hyundai,
VW, Nissan's icon) is invisible on the overlay's `#101418` ground. Rendered
onto that ground at 96 px, and counting the fraction of the mono variant's
ink that a colour variant reproduces at a contrast of 2:1 or better, the
colour files that actually read are:

| Shape | Brands whose colour file reads on the overlay |
| --- | --- |
| `icon` (24) | Alfa Romeo, BMW, BYD, Bentley, Bugatti, Cadillac, Fiat, Ford, Honda, Jeep, Koenigsegg, Lamborghini, Land Rover, Lotus, Mercedes, Maserati, Mazda, McLaren, Mitsubishi, Porsche, Rivian, Subaru, Tesla, Toyota |
| `badge` (23) | Alfa Romeo, Aston Martin, BMW, BYD, Bentley, Cadillac, Dodge, Ferrari, Fiat, Ford, Honda, Jeep, Koenigsegg, Lamborghini, Lotus, Mazda, Mitsubishi, Porsche, Rivian, Rolls-Royce, Subaru, Tesla, Toyota |
| `horizontal` (21) | Alfa Romeo, BMW, BYD, Bentley, Bugatti, Cadillac, Dodge, Ford, Honda, Jeep, Koenigsegg, Land Rover, Mazda, McLaren, Mitsubishi, Nissan, Porsche, Rivian, Subaru, Tesla, Toyota |
| `wordmark` (12) | Audi, BMW, BYD, Honda, Jaguar, Jeep, Mazda, Nissan, Subaru, Tesla, Toyota, Volkswagen |

(The rule was: at least a quarter of the mono mark's ink at ≥ 2:1. A
stricter 3:1 threw out every dark-blue brand, which at 88 px still read;
a looser one let in Chevrolet's black bowtie, which doesn't.) Everything
not in a row falls back to mono when colour is asked for. The table is
the starting point, checked into the manifest below where anyone can
correct it — not a rule baked into the code.

## Decisions

1. **Ship all eight variants for all 51 brands.** 4.9 MB of SVG beside the
   exe. The user asked for the lot, and the alternative — picking for
   them — is what produced today's inconsistent set.
2. **The default is the current hand-picked set, brand by brand.** Nothing
   changes for anyone who never opens the page. It is written down in the
   manifest as `curated`, so it stops being a fact hidden in file hashes.
3. **One global choice, then per-brand overrides.** Style (Curated / Icon /
   Badge / Horizontal / Wordmark) and a Colour toggle set every brand at
   once; a per-brand pick wins over both. Most people want "make them all
   badges" or "give me colour"; the per-brand grid is for the driver who
   wants BMW in colour and everything else white.
4. **Colour never means invisible.** With Colour on, a brand gets its colour
   file only if the manifest says that file reads on the overlay; otherwise
   it gets mono. A per-brand override can still force the colour file — the
   grid shows what you'll get, on the real ground, so that is an informed
   choice.
5. **Marks stay in the same slots.** Icon and Badge are square and fit
   today's slots. Horizontal and Wordmark are wide, so choosing either as
   the *global* style widens the slot (see [Slot sizes](#slot-sizes)); a
   per-brand wide override inside a square slot just draws smaller, so that
   rows in a column stay aligned.
6. **Files stay file-based.** The loader keeps reading SVGs by name from
   `assets/logos/`, so adding a brand upstream doesn't have, or replacing a
   mark you dislike, is still "drop a file in a folder".

## Files on disk

```
assets/logos/
  manifest.toml                 # curated picks + colour legibility (below)
  icon/audi.svg                 # one folder per variant, 51 files each
  icon-colour/audi.svg
  badge/audi.svg
  badge-colour/audi.svg
  horizontal/audi.svg
  horizontal-colour/audi.svg
  wordmark/audi.svg
  wordmark-colour/audi.svg
  <brand>.svg                   # a flat file is a user's own mark (see Resolution)
  LICENSE-cardog-icons.txt
  README.md
```

File names stay the lowercase hyphenated brand as today (`alfa-romeo`,
`rolls-royce`, `landrover`, `mb`); `ui::logos::ALIASES` is unchanged. The
51 flat files that exist today are removed — every one of them is
reproduced byte-for-byte in one of the folders, and `curated` in the
manifest records which. Git history keeps them anyway.

Files are copied from upstream `core/raw` unmodified, so a future upstream
refresh is a copy, not a merge.

## The manifest

`assets/logos/manifest.toml`, read once at startup, hand-editable:

```toml
# Which variant each brand gets when the style is "Curated". Absent brands
# get "icon".
[curated]
bmw = "icon-colour"
porsche = "icon-colour"
aston-martin = "horizontal"
gmc = "wordmark"
# ... one line per brand that isn't plain "icon"

# Colour files that read on the overlay's dark ground. A brand missing from
# a shape's list gets the mono file when colour is asked for.
[colour_legible]
icon = ["alfa-romeo", "bmw", "byd", ...]
badge = [...]
horizontal = [...]
wordmark = [...]
```

Both tables come from the analysis above — `curated` is today's file set
(`acura = icon`, `alfa-romeo = icon-colour`, `aston-martin = horizontal`,
`bentley = icon-colour`, `bmw = icon-colour`, `bugatti = icon-colour`,
`byd = badge-colour`, `cadillac = icon-colour`, `fiat = badge-colour`,
`ford = badge-colour`, `gmc = wordmark`, `honda = icon-colour`,
`hummer = wordmark`, `hyundai = icon-colour`, `jeep = icon-colour`,
`kia = wordmark`, `koenigsegg = icon-colour`, `lamborghini = badge-colour`,
`landrover = icon-colour`, `lotus = icon-colour`, `lucid = wordmark`,
`maserati = icon-colour`, `mazda = icon-colour`, `mb = icon-colour`,
`mclaren = icon-colour`, `mitsubishi = icon-colour`, `pagani = horizontal`,
`porsche = icon-colour`, `rivian = icon-colour`, `subaru = icon-colour`,
`tesla = icon-colour`, `toyota = icon-colour`, `volkswagen = icon-colour`;
everything else `icon`). Two of those — Hyundai and Volkswagen in colour
icon — are dark blue and fail the legibility rule; they stay in `curated`
because that is what ships today, and the per-brand grid will show the
driver exactly how faint they are.

A missing or unparseable manifest is not fatal: `curated` becomes "icon for
everything" and `colour_legible` becomes empty (colour never used), and one
line goes to the log. The overlay must draw marks with no manifest at all.

## Config

In `%APPDATA%\race\race-overlay.toml`, top level, since both widgets share
it:

```toml
[logos]
style = "curated"        # curated | icon | badge | horizontal | wordmark
colour = false           # prefer the colour file where it reads

[logos.overrides]        # per-brand, file-name keys, variant values
bmw = "badge-colour"
ferrari = "badge"
```

`LogoConfig { style: LogoStyle, colour: bool, overrides: BTreeMap<String,
LogoVariant> }`, `#[serde(default)]` throughout so an old file loads. An
unknown variant string in `overrides` is dropped with a log line rather
than failing the whole file. Saved through the same settle-and-write path
as every other settings row.

## Resolution

For a car whose `CarScreenName` starts with brand `B` (after `ALIASES`),
the variant wanted is:

1. `overrides[B]` if present;
2. else if `style == curated`: `curated[B]` from the manifest (default
   `icon`), then if `colour` is on and that variant is mono and `B` is in
   `colour_legible[shape]`, its colour twin;
3. else `<style>` — and its colour twin under the same legibility test.

Then the file is the first that exists of:

1. `assets/logos/<variant>/B.svg`;
2. if the variant is colour, `assets/logos/<shape>/B.svg` (its mono twin);
3. `assets/logos/<variant of curated[B]>/B.svg` — the shape asked for
   doesn't exist for this brand, so fall back to what always worked;
4. `assets/logos/B.svg` — a flat file, which is how a user adds a brand
   upstream lacks (or replaces a mark for good, since it is tried when
   none of the folders have the brand);
5. the text abbreviation, as today.

The memo in `ui::logos` is keyed by `(variant, brand)`, not `brand`, so
changing the style in Settings takes effect on the next frame with no
cache to flush; nothing on disk is re-read more than once per run.

## Settings page

A new **Logos** page in the rail, between Standings and Radar Bars (it
serves Standings and Relative, and it is a look-and-feel page rather than
a per-widget one):

```
Style     ( Curated ) ( Icon ) ( Badge ) ( Horizontal ) ( Wordmark )
[x] Colour where it reads

 ┌──────────────────────────────────────────────────────────────┐
 │  [BMW]  [MB]  [Porsche]  [Ferrari]  [Audi]  [McLaren] ...    │  scrollable grid,
 │  [Toyota] ...                                                │  9 per row, 44 px
 └──────────────────────────────────────────────────────────────┘  tiles, on the
                                                                   overlay's ground
 Ferrari — Curated: Icon (white)          ( Icon ) ( Badge ) ( Horizontal ) ( Wordmark )
                                          [ ] Colour        [ Use default ]

                                                         [ Reset all logos ]
```

- The grid shows every brand in the manifest **as it will actually draw**:
  the same `ui::logos::draw` call, on a panel painted the overlay's ground
  colour, so a faint colour file looks faint here too. Tiles are clickable;
  the selected one is outlined. Brands with an override show a small dot.
- Style and Colour apply to every brand at once and the grid re-renders
  that frame.
- Below the grid, the selected brand's row: what it currently resolves to
  and why ("Curated", "Style", or "Override"), a shape picker and colour
  toggle that write `overrides[brand]`, and **Use default** which removes
  the override.
- **Reset all logos** clears `overrides` and puts style/colour back to
  Curated / off.
- The grid's order is the manifest's `curated` table plus every folder the
  loader found a file in, alphabetical by display name; the display name is
  the file stem title-cased with the aliases reversed (`mb` → "Mercedes",
  `landrover` → "Land Rover").

Nothing here is slanted. Tiles are square, straight-edged.

## Slot sizes

Every file is a square box, so a wide mark in a square slot is drawn small:
a wordmark in Standings' 28 px slot is 28 × ~6 px. Rather than let that be
the experience of choosing Wordmark:

- **Standings**: the mark slot is `LOGO_SIZE` tall and `LOGO_SIZE` wide for
  Curated/Icon/Badge, `2 × LOGO_SIZE` wide for Horizontal/Wordmark. The
  iRating pill sits to its left as now, so the pill moves 28 px left and
  the name column loses 28 px. If a long name meets a wide slot the name
  is clipped at the pill, not overdrawn.
- **Relative**: `BRAND_WIDTH` becomes `2 × BRAND_HEIGHT` for the wide
  styles (the row is flow-laid, so the lap time just moves right).
- The wide files letterbox their content in the middle ~70 % of the box
  vertically, so a 2:1 slot fed a 1:1 file still shows the mark at the
  slot's height — `draw` keeps fitting the square file to the slot's
  height and centring it. No file is cropped or rewritten.
- Slot width follows the **global** style only. A per-brand wide override
  under a square global style draws in the square slot; the grid in
  Settings shows it at that size so nobody is surprised.

## Edge cases

- **Manifest missing/bad** — see above; marks still draw, colour is never
  chosen automatically, and the page still works (grid built from folders).
- **A folder missing** (someone deleted `wordmark-colour/`) — resolution
  falls through to the mono twin, then curated, then flat, then text.
- **Brand upstream lacks** (Dallara, Ligier, Radical, iRacing's fictional
  makes) — as today: flat file if the user added one, else abbreviation.
  The grid lists only brands with a file somewhere.
- **A flat file and a folder file both exist** — the folder wins for the
  variant it holds; the flat file is only reached when no folder has the
  brand. Someone who wants to replace one variant edits that folder's file.
- **Old config with no `[logos]`** — defaults; identical rendering to
  today.
- **Override for a brand the manifest doesn't know** (typo, or a user's own
  brand) — kept and applied; the grid shows it if a file exists.
- **Colour on, override to a mono variant** — the override wins; colour is
  a preference, an override is an instruction.
- **Dimmed rows** — the tint is `from_white_alpha(80)` multiplied over
  whatever the file's colours are, so colour marks dim the same way.
- **Layout mode / demo mode** — draw the same as live; the demo roster
  already spans a dozen brands, so `--demo` is the preview.
- **DPI / scale** — unchanged: egui's SVG loader rasterises at the
  requested pixel size.

## Status (2026-08-29)

All four phases are written, uncompiled at the time of writing (done
mid-race, with a no-build rule in force): the eight folders and
`manifest.toml` are in `assets/logos/`, `LogoConfig` in `config.rs`,
resolution in `ui/logos.rs` (config applied per frame through
`logos::apply`, memo keyed by variant), the **Logos** page in
`ui/settings.rs`, and the wide slots in Standings and Relative. Two
deliberate departures from the text above: the 51 flat files are still in
place (the running overlay reads them live; delete them after a race, they
are byte-copies of folder files), and a long name meeting a wide Standings
slot is not clipped at the pill yet — names in that column are short in
practice. To finish: `cargo build --release`, then `--demo` before/after
screenshots to confirm the default render is unchanged.

## Build order

1. **Assets and manifest.** Copy the 408 files into the eight folders,
   write `manifest.toml` from the tables above, remove the flat files,
   correct `assets/logos/README.md` (it currently misstates the source
   variant) and the root README's asset note.
2. **Resolution.** `LogoConfig` in `config.rs`; `ui::logos` learns the
   manifest, variants and the fallback chain; memo keyed by variant. The
   overlay renders identically to today with a default config — verify by
   `--demo` screenshot before and after.
3. **Settings page** with style/colour and the grid, per-brand overrides,
   Reset.
4. **Slot widening** for the wide styles in Standings and Relative.

Each phase is shippable; 1 + 2 alone fix the README lie and make the picks
explicit even if nobody opens the page.

## Out of scope

- Trimming the letterbox out of each file (a bounds manifest or rewriting
  `viewBox`). Upstream padding is consistent, and the wide-slot rule
  covers the one case where it hurts.
- Custom colour for the mono marks (tinting them a team colour). The
  palette is the design.
- Per-car (as opposed to per-brand) marks, e.g. a different mark for the
  GT3 and the GT4 of one make.
- A tool to regenerate `colour_legible`. The rule is documented above; if
  the upstream set changes, rerun it by hand or edit the table.
