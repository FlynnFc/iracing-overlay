# Car manufacturer marks

Rendered beside each car in the Standings and Relative widgets (see
`src/bin/race-overlay/ui/logos.rs`), and chosen on the **Logos** page of the
settings window.

Source: [cardog-ai/icons](https://github.com/cardog-ai/icons), MIT licensed
(see `LICENSE-cardog-icons.txt`), copied unmodified from its `core/raw` set.
All 51 brands in all 8 variants, one folder per variant:

| Folder | Upstream name | What it is |
| --- | --- | --- |
| `icon/` | Icon Dark | The emblem alone, white |
| `icon-colour/` | Icon | The emblem alone, brand colours |
| `badge/` | Logo Dark | Emblem over the wordmark, white |
| `badge-colour/` | Logo | Same, brand colours |
| `horizontal/` | Logo Horizontal Dark | Emblem beside the wordmark, white |
| `horizontal-colour/` | Logo Horizontal | Same, brand colours |
| `wordmark/` | Wordmark Dark | The name alone, white |
| `wordmark-colour/` | Wordmark | Same, brand colours |

Upstream's "Dark" means *for dark backgrounds* — white ink — which is why
those are the default here. The colour files were drawn for white pages and
many are black or navy; `manifest.toml` lists the ones that still read on
the overlay, and the overlay only picks a colour file automatically from
that list.

`manifest.toml` also records the default pick per brand (`[curated]`) — the
hand-chosen mix the overlay ships with. Both tables are hand-editable.

File names are the lowercase, hyphenated brand name (`alfa-romeo.svg`,
`rolls-royce.svg`). `mb.svg` is Mercedes-Benz, matching upstream's name;
`ui/logos.rs` aliases `mercedes`, `mercedes-amg` and `amg` onto it.

To add a brand upstream doesn't have, drop `<lowercase-hyphenated-name>.svg`
directly in this folder — a flat file is used whenever no variant folder has
the brand, for every style. To replace one variant of one brand, edit that
folder's file. Cars with no matching file anywhere fall back to a short
uppercase text abbreviation.

The brand names and marks are trademarks of their respective owners, used
here only to identify the car a driver is in.
