// Rust guideline compliant 2026-02-16

//! Pure standings math: estimating where the player would rank after a pit
//! stop. Kept free of telemetry types so it's cheap to unit test.

/// Estimates the player's position if they pitted right now.
///
/// Counts how many cars in `field_gaps_to_leader_secs` (which must exclude
/// the player's own entry) currently have a smaller gap-to-leader than the
/// player's *projected* gap after adding `pit_loss_secs` — i.e. cars that
/// would still be ahead once the player rejoins.
#[must_use]
pub fn project_pit_position(my_gap_to_leader_secs: f32, pit_loss_secs: f32, field_gaps_to_leader_secs: &[f32]) -> i32 {
    let projected_gap = my_gap_to_leader_secs + pit_loss_secs;
    let ahead_count = field_gaps_to_leader_secs.iter().filter(|&&gap| gap < projected_gap).count();
    i32::try_from(ahead_count).unwrap_or(i32::MAX).saturating_add(1)
}

/// Decides which class positions the windowed Standings view should show:
/// the top `top_n` positions plus a window of `before`/`after` positions
/// around the player, merged into one contiguous run (no gap marker) when
/// they already touch or overlap, otherwise separated by a `None` "skip"
/// marker the caller renders as a `...` row.
///
/// The window is then widened until at least `min_shown` positions are on
/// screen, or the class runs out of cars — so a player near the front, whose
/// window merges into the top of the order and leaves only a handful of rows,
/// still gets a useful view of the class. It grows backwards through the field
/// first, since the positions ahead of the player are already covered by
/// `top_n` and the window itself.
///
/// `field_size` and `player_position` are both 1-based class positions.
/// Returns an empty list for a field of size zero.
#[must_use]
pub fn windowed_class_positions(
    field_size: i32,
    player_position: i32,
    top_n: i32,
    before: i32,
    after: i32,
    min_shown: i32,
) -> Vec<Option<i32>> {
    if field_size <= 0 {
        return Vec::new();
    }
    let top_end = top_n.clamp(0, field_size);
    let mut window_start = (player_position - before).max(1);
    let mut window_end = (player_position + after).min(field_size);

    // How many driver rows a given window produces, skip marker excluded:
    // one contiguous run when the two blocks touch, otherwise both blocks.
    let shown = |start: i32, end: i32| {
        if start <= top_end + 1 { end.max(top_end) } else { top_end + (end - start + 1) }
    };
    // Never asks for more of the class than exists, so this always terminates
    // on one of the two bounds rather than spinning at a full field.
    let target = min_shown.min(field_size);
    while shown(window_start, window_end) < target {
        if window_end < field_size {
            window_end += 1;
        } else if window_start > 1 {
            window_start -= 1;
        } else {
            break;
        }
    }

    if window_start <= top_end + 1 {
        (1..=window_end.max(top_end)).map(Some).collect()
    } else {
        let mut positions: Vec<Option<i32>> = (1..=top_end).map(Some).collect();
        positions.push(None);
        positions.extend((window_start..=window_end).map(Some));
        positions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimum of zero leaves the window exactly as asked for, which is what
    /// the shape tests below are about.
    fn window(field_size: i32, player: i32, before: i32, after: i32) -> Vec<Option<i32>> {
        windowed_class_positions(field_size, player, 3, before, after, 0)
    }

    #[test]
    fn window_far_from_top_gets_a_skip_marker() {
        let positions = window(20, 6, 1, 2);
        assert_eq!(positions, vec![Some(1), Some(2), Some(3), None, Some(5), Some(6), Some(7), Some(8)]);
    }

    #[test]
    fn window_touching_top_merges_with_no_skip_marker() {
        let positions = window(20, 4, 1, 2);
        assert_eq!(positions, vec![Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)]);
    }

    #[test]
    fn small_field_never_shows_a_skip_marker() {
        let positions = window(5, 2, 1, 2);
        assert_eq!(positions, vec![Some(1), Some(2), Some(3), Some(4)]);
    }

    #[test]
    fn a_lone_entry_does_not_panic() {
        assert_eq!(window(1, 1, 1, 2), vec![Some(1)]);
    }

    #[test]
    fn empty_field_returns_nothing() {
        assert_eq!(window(0, 1, 1, 2), Vec::new());
    }

    /// How many driver rows a result actually draws.
    fn drivers(positions: &[Option<i32>]) -> usize {
        positions.iter().flatten().count()
    }

    /// The leader's own window merges into the top of the order and leaves
    /// four rows on a screen with room for far more.
    #[test]
    fn a_player_near_the_front_still_fills_the_view() {
        let positions = windowed_class_positions(20, 1, 3, 3, 3, 10);
        assert_eq!(positions, (1..=10).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn a_player_mid_field_already_meets_the_minimum_and_is_left_alone() {
        let positions = windowed_class_positions(20, 12, 3, 3, 3, 10);
        assert_eq!(drivers(&positions), 10);
        assert_eq!(positions.first(), Some(&Some(1)));
        assert_eq!(positions.last(), Some(&Some(15)));
    }

    /// A player at the very back can only be filled out by reaching further
    /// forward, since there is nothing behind them to show.
    #[test]
    fn a_player_at_the_back_grows_the_window_forwards() {
        let positions = windowed_class_positions(12, 12, 3, 3, 3, 10);
        assert_eq!(drivers(&positions), 10);
        assert_eq!(positions.last(), Some(&Some(12)));
    }

    /// A class smaller than the minimum shows all of it and stops, rather than
    /// looping for rows that do not exist.
    #[test]
    fn a_class_smaller_than_the_minimum_is_shown_whole() {
        let positions = windowed_class_positions(6, 2, 3, 3, 3, 10);
        assert_eq!(positions, (1..=6).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn nobody_ahead_projects_first() {
        assert_eq!(project_pit_position(0.0, 30.0, &[]), 1);
    }

    #[test]
    fn a_long_pit_stop_can_drop_the_leader_behind_the_field() {
        // Player leads (gap 0.0); a 30s stop drops them behind the two cars
        // currently within 30s, but not the one 50s back.
        let field = [15.0, 25.0, 50.0];
        assert_eq!(project_pit_position(0.0, 30.0, &field), 3);
    }

    #[test]
    fn zero_pit_loss_keeps_the_current_position() {
        // Player is 5s behind the leader (P2 in a 4-car field).
        let field = [0.0, 15.0, 30.0];
        assert_eq!(project_pit_position(5.0, 0.0, &field), 2);
    }
}
