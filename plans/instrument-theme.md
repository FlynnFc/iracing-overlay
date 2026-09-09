# Instrument theme: the Dash's style, applied everywhere

The Dash looks unlike the rest of the overlay, and the difference is not
accidental — it is a coherent style that could be offered as a theme for
Standings, Relative and the Radar. This names that style, says what a theme
switch would have to abstract, and phases the work.

**Built 2026-08-31.** `ui/theme.rs` holds the tokens, `config.theme` selects
one, and the General settings page switches it live. What follows records the
style and the decisions the build settled.

## Naming the style

The Dash imitates a hardware display — a Cosworth/AiM-style DDU — and four
choices carry that read:

1. **Legend boxes, not cards.** A group is a 1.5 px near-white outline
   (`#E5E5E5`) with its title set into a *gap in the top edge*. Nothing is
   filled. The rest of the overlay does the opposite: filled near-black cards
   (`PANEL_BG`) with a shadow and no border, titles inside the fill.
2. **Ink is the background, light is the content.** The Dash is black with
   bright marks on it. The other widgets are dark-grey surfaces floating on
   the track, distinguished by fill.
3. **Saturated, non-semantic accents.** Pure green `#00FF00`, magenta
   `#C70CCF`, yellow `#FFEB5F` — chosen because a hardware display uses the
   full gamut, not because green means "good". The overlay's own set is
   deliberately muted and semantic (`SIGNAL`, `ALERT`, `CAUTION`, `LAPPED`).
4. **Numerals dominate, labels whisper.** Values are set as large as the box
   allows — tracked out to span it where the face is too narrow
   (`paint_tracked`) — over 8 px uppercase labels at `text_tertiary`.

Call it **Instrument**; the existing look is **Panel**.

## What a theme has to abstract

The two styles differ in *structure*, not just colour, which is what makes
this more than a palette swap:

| Concern | Panel | Instrument |
| --- | --- | --- |
| Group container | filled card + shadow, rounded 16 | 1.5 px outline, legend in a top-edge gap, rounded 12 |
| Section heading | text inside the card, accent underline | legend set into the border |
| Background | `PANEL_BG` translucent ink | transparent; the card fill *is* the black |
| Row separation | grooves, alternating stripes | thin crosshair dividers |
| Accents | muted semantic set | saturated display set |
| Emphasis | weight and fill | size and tracking |

So the theme has to be a **trait or token struct**, not a colour table:

```rust
/// How a widget draws its chrome, so a theme can restyle every panel at once.
pub struct Chrome {
    /// Draws a group's container and its heading.
    pub group: fn(&Ui, Metrics, Rect, &str),
    pub surface: Color32,
    pub border: Color32,
    pub accents: Accents,
    pub group_radius: f32,
}
```

`ui::mod` already centralises the tokens every widget reads (`PANEL_BG`,
`card_frame`, `text_primary`, …), so the migration is mostly turning those
constants into lookups on the active `Chrome`. The Dash's `legend_box` and the
existing `card_frame` become the two implementations of `group`.

## What shipped

`ui::theme` holds a `Theme` enum and the tokens that differ, published once a
frame by `theme::apply` from `gui_run` — the same shape `ui::logos` already
uses, but behind an `AtomicU8` rather than a `Mutex`, because these are read
dozens of times a frame on the draw path and a relaxed load is one
instruction.

The Panel constants stay exactly as they were and the accessors pick between
them, so every `[`SIGNAL`]`-style doc link still resolves and Panel renders
byte-identically to before.

| Token | Panel | Instrument |
| --- | --- | --- |
| card edge | drop shadow | 1.5 px `#E5E5E5` border |
| card radius | 16 | 12 |
| card fill | as the widget asked | backed to alpha 232, ink preserved |
| inner tiles | lifted fill | hollow |
| row separation | bevel groove + alternating stripe | one quiet divider, no stripe |
| accents | muted semantic set | saturated display set |

Applied across every widget: the chrome through `card_frame`/`card_rounding`/
`row_stripe`/`paint_row_groove`, and the accents in Standings, Relative,
Radar, Pit Stall and the black box. The Dash is deliberately untouched — it
*is* the Instrument look, and already carries its own palette.

## Decisions the build settled

- **Instrument keeps a card fill** — the open question from the spec. An
  outline over a translucent fill reads as a wire frame with the track showing
  through it, so `theme::card_fill` backs the alpha to 232 while rescaling the
  channels so the ink colour survives. Not fully opaque: the overlay is still
  an overlay.
- **No legend-in-the-border for Standings or Relative.** The Dash's signature
  flourish needs a title, and neither panel has one today; inventing
  "STANDINGS" to sit in a border would add chrome rather than adopt a style.
  The border, radius, hollow tiles and accents carry the language instead.
  `stroke_box_with_top_gap` is there in `ui::dash` if a titled group ever
  wants it.
- **Stripes go, dividers stay.** An alternating fill inside a bordered card is
  a second surface, which is the thing this theme does not have.

## Still open

- Class sections in Standings would be the natural place for a legend box —
  the class name is a title, and it would look exactly like the Dash. Worth
  doing if Instrument sticks.
- Weather's stat tiles are hollowed by `tile_fill` only where a widget routes
  its fill through it; Weather paints `TILE_BG` directly and so keeps its
  fills under Instrument.
