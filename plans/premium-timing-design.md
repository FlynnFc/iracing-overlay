# Premium timing overlay redesign

## Goal
Give Relative and Standings a distinct, commercially polished identity while preserving their information, configuration, telemetry and racing controls. Translate the restraint and clarity of Apple, TrustMRR and DataFast into a compact dark racing instrument.

## Visual direction
- Graphite surfaces with restrained differences between table bands; enough opacity to support text over the track.
- Position, driver and gap lead. Ratings, car numbers and session metadata form a quieter supporting layer.
- Inset position tiles, a near-white focus tile and a neutral selected row identify the driver.
- Short class and licence markers preserve meaning without bright repeated badge outlines.
- A quiet page rail with an unmistakable selected state.
- Preserve established warning, lapped, fastest-lap and pit colours. Keep fixed numeric columns, clipping and configurable widths.
- Keep Instrument mode coherent; do not introduce fonts, network assets, blur, or new telemetry work.

## Implementation ownership
1. Shared visual details: shared position helper and page rail (shared_design agent).
2. Relative: header, footer, row hierarchy and supporting rating treatment (relative agent).
3. Standings: class grouping, table surfaces, timing hierarchy and endurance presentation (standings agent).
4. Integration: build, inspect actual demo screenshots, fix defects, and run existing Rust tests (primary agent).

## Acceptance
- All current data and control paths retained.
- Narrow Relative, multi-class Standings and endurance remain aligned and readable.
- Player focus and urgent states remain immediately distinguishable.
- Demo screenshots reviewed for clipping, competing contrast and consistency.
- cargo fmt and cargo test completed; compile checked through demo binary build.

## Scope
Relative, Standings and the shared page rail/position treatment. No changes to site/, telemetry, settings schema or other widget layouts.

## Completed validation
- Implemented by three agents and integrated by the primary agent.
- `cargo build --bin race-overlay`: passes without warnings.
- `cargo test --all-targets`: passes (including 533 overlay tests).
- `cargo fmt --all -- --check` and `git diff --check`: pass.
- Inspected actual demo captures: default Relative, 520px Relative while spectating, caution, multiclass endurance Standings, and Instrument Relative.
- Reduced alternating row contrast after the first visual review so the focus row remains distinct.
- Review images: [Relative](../docs/design/premium-relative.png), [Standings](../docs/design/premium-standings.png).
- Validation uses demo telemetry. Live in-race readability has not been assessed.


## Phase 2 ? Black box and settings
Carry the approved timing language into Fuel, Tires, Pit Window, In-Car, Weather and their crew variants, plus the complete settings window.

- Keep page layouts, wheel/mouse interactions, telemetry and warning meanings.
- Black box: graphite base; neutral focus treatment; quieter tiles and control rows; amber stays explicit for armed pit services.
- Settings: scoped styling, a clear sidebar selection, calmer controls, stronger page hierarchy and bounded scrolling for long pages.
- Add a demo-only settings page switch so settings can be reviewed through the existing screenshot capture route.
- Review actual captures of all black box pages, crew variants and representative settings pages; build, test and check formatting.

Ownership: blackbox/mod.rs (shared_design), settings.rs (relative), preview CLI in main.rs/app.rs (standings), integration/docs/theme copy (primary).


### Phase 2 validation complete
- Build passes without compiler warnings; 542 existing tests pass across all targets.
- Formatting and whitespace checks pass.
- Rendered and inspected all five black box pages, populated pit-window forecasts, crew Fuel/Tires/Strategy variants, tyre wear alerts and Instrument Fuel.
- Rendered and inspected all eleven settings pages. Fixed an inherited horizontal layout discovered in the first capture; page content now stacks vertically inside a bounded viewport.
- Screenshot runs ignore desktop input so incidental hover tooltips cannot obscure previews. Live settings retain normal input and tooltips.
- Final previews are in `docs/design/`: premium-settings, premium-fuel, premium-tires, premium-pit-window, premium-in-car and premium-weather.
- Live in-race and wheel hardware interaction testing remains outside this demo-based validation.

### Alert frame refinement
- Replaced the solid alert-coloured Relative gutter with graphite.
- Retained the semantic coloured outline and label, plus the existing BOX BOX approach pulse and status priority.
- Reserved a 36px alert header above navigation so the label cannot overlap tabs or clip above a top-aligned widget.
- Reviewed actual caution and BOX BOX Relative captures and BOX BOX on Fuel; build, formatting and the status-priority test pass.
