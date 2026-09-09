// Rust guideline compliant 2026-02-16

//! Draws a driver's national flag from the `FlairID` iRacing publishes.
//!
//! Every entry in the session string's `DriverInfo.Drivers` carries a
//! `FlairID`: the flag the member picked on their iRacing profile. iRacing
//! never publishes the country itself, only the id, so this module holds
//! the id → ISO 3166 code table (see [`country_code`]) and draws the
//! matching file from `assets/flags/`.
//!
//! The table is the one the iRacing Data API's `lookup/flairs` endpoint
//! returns, which needs a member login and so is copied here rather than
//! fetched. The ids run in alphabetical country order from Afghanistan at 3;
//! ids 0–2 mean no flag chosen (the pace car reports 2), and the four home
//! nations of the UK and the Caribbean Netherlands were appended later, out
//! of order, at 236–242. The flag files are `lipis/flag-icons`' 4:3 set,
//! MIT licensed — see `assets/flags/LICENSE-flag-icons.txt`.
//!
//! A driver with no flag, or an id this table doesn't know, draws the
//! neutral world flag (`world.svg`, a globe on slate drawn for this
//! overlay) — a slot left empty read as a rendering bug, not a choice.
//! [`draw`] returns `false` only when even that file is missing.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use egui::{Color32, Rect, Ui};

use super::Memo;

/// The width-to-height ratio of every flag file, and so of the slot a
/// caller should allocate. The 4:3 set was chosen over the 1:1 set because
/// at row height a square crop loses the distinguishing part of most flags.
pub const ASPECT: f32 = 4.0 / 3.0;

/// The corner radius a flag is drawn with, in points. Small enough to read
/// as a straight-edged block at row height, matching the class tags.
const CORNER_RADIUS: f32 = 2.0;

/// `FlairID` → ISO 3166-1 alpha-2 code (alpha-2 plus subdivision for the UK
/// home nations), sorted by id so [`country_code`] can binary search.
///
/// Id 147 is absent from iRacing's own list — it sat between the
/// Netherlands and New Caledonia, where the dissolved Netherlands Antilles
/// would fall alphabetically — so the table skips it rather than guess.
const FLAIRS: &[(u16, &str)] = &[
    (3, "AF"),
    (4, "AX"),
    (5, "AL"),
    (6, "DZ"),
    (7, "AS"),
    (8, "AD"),
    (9, "AO"),
    (10, "AI"),
    (11, "AQ"),
    (12, "AG"),
    (13, "AR"),
    (14, "AM"),
    (15, "AW"),
    (16, "AU"),
    (17, "AT"),
    (18, "AZ"),
    (19, "BS"),
    (20, "BH"),
    (21, "BD"),
    (22, "BB"),
    (23, "BE"),
    (24, "BZ"),
    (25, "BJ"),
    (26, "BM"),
    (27, "BT"),
    (28, "BO"),
    (29, "BA"),
    (30, "BW"),
    (31, "BR"),
    (32, "VG"),
    (33, "BN"),
    (34, "BG"),
    (35, "BF"),
    (36, "BI"),
    (37, "KH"),
    (38, "CM"),
    (39, "CA"),
    (40, "CV"),
    (41, "KY"),
    (42, "CF"),
    (43, "TD"),
    (44, "CL"),
    (45, "CN"),
    (46, "CX"),
    (47, "CC"),
    (48, "CO"),
    (49, "KM"),
    (50, "CK"),
    (51, "CR"),
    (52, "HR"),
    (53, "CY"),
    (54, "CZ"),
    (55, "CD"),
    (56, "DK"),
    (57, "DJ"),
    (58, "DM"),
    (59, "DO"),
    (60, "EC"),
    (61, "EG"),
    (62, "SV"),
    (63, "GQ"),
    (64, "ER"),
    (65, "EE"),
    (66, "ET"),
    (67, "FK"),
    (68, "FO"),
    (69, "FJ"),
    (70, "FI"),
    (71, "FR"),
    (72, "GF"),
    (73, "PF"),
    (74, "GA"),
    (75, "GM"),
    (76, "GE"),
    (77, "DE"),
    (78, "GH"),
    (79, "GI"),
    (80, "GR"),
    (81, "GL"),
    (82, "GD"),
    (83, "GP"),
    (84, "GU"),
    (85, "GT"),
    (86, "GG"),
    (87, "GN"),
    (88, "GW"),
    (89, "GY"),
    (90, "HT"),
    (91, "HN"),
    (92, "HK"),
    (93, "HU"),
    (94, "IS"),
    (95, "IN"),
    (96, "ID"),
    (97, "IQ"),
    (98, "IE"),
    (99, "IM"),
    (100, "IL"),
    (101, "IT"),
    (102, "CI"),
    (103, "JM"),
    (104, "JP"),
    (105, "JE"),
    (106, "JO"),
    (107, "KZ"),
    (108, "KE"),
    (109, "KI"),
    (110, "KW"),
    (111, "KG"),
    (112, "LA"),
    (113, "LV"),
    (114, "LB"),
    (115, "LS"),
    (116, "LR"),
    (117, "LY"),
    (118, "LI"),
    (119, "LT"),
    (120, "LU"),
    (121, "MO"),
    (122, "MK"),
    (123, "MG"),
    (124, "MW"),
    (125, "MY"),
    (126, "MV"),
    (127, "ML"),
    (128, "MT"),
    (129, "MH"),
    (130, "MQ"),
    (131, "MR"),
    (132, "MU"),
    (133, "YT"),
    (134, "MX"),
    (135, "FM"),
    (136, "MD"),
    (137, "MC"),
    (138, "MN"),
    (139, "ME"),
    (140, "MS"),
    (141, "MA"),
    (142, "MZ"),
    (143, "NA"),
    (144, "NR"),
    (145, "NP"),
    (146, "NL"),
    (148, "NC"),
    (149, "NZ"),
    (150, "NI"),
    (151, "NE"),
    (152, "NG"),
    (153, "NU"),
    (154, "NF"),
    (155, "MP"),
    (156, "NO"),
    (157, "OM"),
    (158, "PK"),
    (159, "PW"),
    (160, "PS"),
    (161, "PA"),
    (162, "PG"),
    (163, "PY"),
    (164, "PE"),
    (165, "PH"),
    (166, "PN"),
    (167, "PL"),
    (168, "PT"),
    (169, "PR"),
    (170, "QA"),
    (171, "CG"),
    (172, "RE"),
    (173, "RO"),
    (174, "RW"),
    (175, "SH"),
    (176, "KN"),
    (177, "LC"),
    (178, "PM"),
    (179, "VC"),
    (180, "BL"),
    (181, "MF"),
    (182, "WS"),
    (183, "SM"),
    (184, "ST"),
    (185, "SA"),
    (186, "SN"),
    (187, "RS"),
    (188, "SC"),
    (189, "SL"),
    (190, "SG"),
    (191, "SK"),
    (192, "SI"),
    (193, "SB"),
    (194, "SO"),
    (195, "ZA"),
    (196, "GS"),
    (197, "KR"),
    (198, "ES"),
    (199, "LK"),
    (200, "SR"),
    (201, "SJ"),
    (202, "SZ"),
    (203, "SE"),
    (204, "CH"),
    (205, "TW"),
    (206, "TJ"),
    (207, "TZ"),
    (208, "TH"),
    (209, "TL"),
    (210, "TG"),
    (211, "TK"),
    (212, "TO"),
    (213, "TT"),
    (214, "TN"),
    (215, "TR"),
    (216, "TM"),
    (217, "TC"),
    (218, "TV"),
    (219, "UG"),
    (220, "UA"),
    (221, "AE"),
    (222, "GB"),
    (223, "US"),
    (224, "UY"),
    (225, "UZ"),
    (226, "VU"),
    (227, "VA"),
    (228, "VE"),
    (229, "VN"),
    (230, "VI"),
    (231, "WF"),
    (232, "EH"),
    (233, "YE"),
    (234, "ZM"),
    (235, "ZW"),
    (236, "GB-ENG"),
    (237, "GB-SCT"),
    (238, "GB-WLS"),
    (239, "GB-NIR"),
    (240, "BQ"),
    (241, "CW"),
    (242, "SX"),
];

/// The ISO code for a `FlairID`, or `None` for "no flag" and unknown ids.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(country_code(77), Some("DE"));
/// assert_eq!(country_code(2), None); // the pace car
/// ```
#[must_use]
pub fn country_code(flair_id: i32) -> Option<&'static str> {
    let id = u16::try_from(flair_id).ok()?;
    FLAIRS.binary_search_by_key(&id, |(known, _)| *known).ok().map(|index| FLAIRS[index].1)
}

/// The directory holding the flag files, resolved once per run.
fn flag_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| super::find_asset_dir("flags")).as_ref()
}

/// Each code's `file://` URI, or `None` where the file isn't there; see [`Memo`].
static URIS: Memo<Option<Arc<str>>> = Memo::new();

/// The URI for a code's flag file, hitting the disk at most once per code.
fn flag_uri(code: &str) -> Option<Arc<str>> {
    URIS.get_or_insert_with(code, |code| {
        let path = flag_dir()?.join(format!("{}.svg", code.to_ascii_lowercase()));
        path.is_file().then(|| Arc::from(format!("file://{}", path.display())))
    })
}

/// The stem of the fallback file for "no flag chosen": a neutral globe.
const WORLD: &str = "world";

/// Draws the flag for `flair_id`, fitted inside `rect` at 4:3 and centred.
///
/// `dimmed` fades the flag the way a dimmed row's text is faded, for a car
/// that is in the pits or off the lead lap. A driver with no flag, an
/// unknown id, or a missing file gets the neutral [`WORLD`] flag instead of
/// a hole; `false` — and nothing drawn — only when even that is missing.
pub fn draw(ui: &Ui, rect: Rect, flair_id: i32, dimmed: bool) -> bool {
    let Some(uri) = country_code(flair_id).and_then(flag_uri).or_else(|| flag_uri(WORLD)) else {
        return false;
    };
    let tint = if dimmed { Color32::from_white_alpha(120) } else { Color32::WHITE };
    let image = egui::Image::new(&*uri).fit_to_exact_size(rect.size()).tint(tint).rounding(CORNER_RADIUS);
    // `paint_at` fills the rectangle it is given regardless of the file's
    // own proportions, so the fitted size is worked out here. The files are
    // all 4:3, which is also the guess for the frame the texture is still
    // loading on.
    let fitted = image.load_and_calc_size(ui, rect.size()).unwrap_or_else(|| fit_aspect(rect.size()));
    image.paint_at(ui, Rect::from_center_size(rect.center(), fitted));
    true
}

/// The largest 4:3 rectangle that fits inside `bounds`.
fn fit_aspect(bounds: egui::Vec2) -> egui::Vec2 {
    if bounds.x / bounds.y > ASPECT {
        egui::vec2(bounds.y * ASPECT, bounds.y)
    } else {
        egui::vec2(bounds.x, bounds.x / ASPECT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checked against drivers in a captured session: a German in the
    /// DE-AT-CH club, two Americans, a New Zealander in Australia/NZ.
    #[test]
    fn known_ids_map_to_their_countries() {
        assert_eq!(country_code(77), Some("DE"));
        assert_eq!(country_code(223), Some("US"));
        assert_eq!(country_code(149), Some("NZ"));
        assert_eq!(country_code(222), Some("GB"));
        assert_eq!(country_code(236), Some("GB-ENG"));
        assert_eq!(country_code(242), Some("SX"));
    }

    /// The pace car reports 2; a YAML that omits the field defaults to 0.
    #[test]
    fn no_flag_and_unknown_ids_map_to_nothing() {
        assert_eq!(country_code(0), None);
        assert_eq!(country_code(1), None);
        assert_eq!(country_code(2), None);
        assert_eq!(country_code(147), None);
        assert_eq!(country_code(243), None);
        assert_eq!(country_code(-1), None);
    }

    /// Binary search needs the table sorted, and a duplicate id would mean
    /// one country silently shadowing another.
    #[test]
    fn table_is_strictly_ascending() {
        assert!(FLAIRS.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    /// The no-flag fallback has to ship, or the drivers it exists for get
    /// the very hole it was drawn to fill.
    #[test]
    fn the_world_fallback_ships() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("flags").join(format!("{WORLD}.svg"));
        assert!(path.is_file(), "missing {}", path.display());
    }

    /// Every code in the table has a file shipped for it, so no known
    /// driver ever gets an empty slot for want of an asset.
    #[test]
    fn every_code_has_a_flag_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("flags");
        let missing: Vec<&str> = FLAIRS
            .iter()
            .map(|(_, code)| *code)
            .filter(|code| !dir.join(format!("{}.svg", code.to_ascii_lowercase())).is_file())
            .collect();
        assert!(missing.is_empty(), "flag files missing: {missing:?}");
    }

    #[test]
    fn fits_the_wider_and_the_taller_slot() {
        assert_eq!(fit_aspect(egui::vec2(40.0, 12.0)), egui::vec2(16.0, 12.0));
        assert_eq!(fit_aspect(egui::vec2(16.0, 40.0)), egui::vec2(16.0, 12.0));
    }
}
