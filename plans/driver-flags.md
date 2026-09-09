# Driver flags

**Status: implemented (2026-09-04).** Kept as the record of the decisions.

## What

Each driver's national flag, drawn before their name in the Standings and
the Relative, from the flag they picked on their iRacing profile.

## Where the data comes from

iRacing never publishes a country. Every entry in the session string's
`DriverInfo.Drivers` carries `FlairID` (and `FlairName`), the profile flag as
iRacing's own integer id. `ClubID`/`ClubName` also exist but are regional
groupings (`DE-AT-CH`, `Australia/NZ`, `Pennsylvania`) and too coarse.

The id → country table is what the iRacing Data API's `lookup/flairs`
endpoint returns. That endpoint needs a member login, so the table is
copied into `ui/flags.rs` rather than fetched. It runs alphabetically from
Afghanistan at 3 to Zimbabwe at 235, then the four UK home nations at
236–239 and the Caribbean Netherlands additions at 240–242. Id 147 is
absent upstream (where the dissolved Netherlands Antilles would fall) and
is left absent here. Cross-checked against a captured session from the
open-source irdashies overlay: id 77 with club DE-AT-CH, 223 with New
England, 149 with Australia/NZ — Germany, the US, New Zealand.

This is how every open-source overlay does it (irdashies, iFL03's
successors, SimHub dashboards): a baked table, an SVG per code.

## Decisions

- **Placement.** Relative: between the car number and the name. Standings:
  between the class bar and the name. Left of the name is the convention
  in iRacing's own UI and every commercial overlay, and it keeps the flag
  in a fixed column so the eye can scan flags down the card.
- **Size.** 4:3 files (`lipis/flag-icons`, MIT), because at row height a
  square crop loses the distinguishing part of most flags. 15 px tall in
  the Relative's 44 px row, 14 px in the Standings' 34 px row; 2 px corner
  radius, per the no-slanted, small-radius rule.
- **No flag.** A driver with id 0 (none picked, or a YAML that omits the
  field) or 2 (the pace car) draws nothing but keeps the slot, so names
  stay in one column. No placeholder glyph: an empty slot reads as "no
  flag" better than a generic mark reads as anything.
- **Toggle.** One top-level `show_flags` (default on), a checkbox on the
  settings window's General page beside *Count off-tracks*, since a flag
  means the same thing in both panels. Off means the name moves back to
  its old column: no hole.
- **Dimming.** A flag on a dimmed row (in the pits, off the lead lap) is
  faded the way that row's text is.
- **Wash.** The class-colour wash that fades out past the first letter of
  the name extends by the flag slot's width while flags are on, so it
  still dies where it was tuned to.
- **Assets.** Only the 240 codes the table names are shipped, looked up
  beside the executable then in the working directory, like logos and
  icons. A test asserts every code in the table has a file.
- **Not verified live.** No captured session YAML exists in this repo, so
  the field's presence in Flynn's own sessions is inferred from other
  projects' captures. If every flag is blank in a real session, dump the
  YAML and check the key is `FlairID`.
