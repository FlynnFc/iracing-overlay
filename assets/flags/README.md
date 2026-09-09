# Driver flags

Drawn before each driver's name in the Standings and Relative widgets (see
`src/bin/race-overlay/ui/flags.rs`), and switched on or off with **Show
driver flags** on the settings window's General page.

Source: [lipis/flag-icons](https://github.com/lipis/flag-icons), MIT
licensed (see `LICENSE-flag-icons.txt`), copied unmodified from its `4x3`
set. One file per ISO 3166-1 alpha-2 code, lowercase (`de.svg`, `us.svg`),
plus the four UK home nations as `gb-eng`, `gb-sct`, `gb-wls` and `gb-nir`.

## Where the country comes from

iRacing does not publish a driver's country. It publishes a `FlairID` in the
session string's `DriverInfo.Drivers` — the flag the member picked on their
profile — and `ui/flags.rs` holds the id → code table. The table is the one
the iRacing Data API's `lookup/flairs` endpoint returns; that endpoint needs
a member login, so the table is copied rather than fetched. Only the codes
that table names are shipped here, which is why this folder has 240 files
rather than the upstream 270.

A driver who has not picked a flag reports id 0 (the pace car reports 2).
Both draw nothing, leaving the slot empty so names stay in one column.

## Adding or replacing one

Drop `<lowercase-code>.svg` in this folder. Files are looked up beside the
executable first, then in the working directory, the same way
`assets/logos/` is. A code in the table with no file here also draws
nothing; `cargo test every_code_has_a_flag_file` says which are missing.
