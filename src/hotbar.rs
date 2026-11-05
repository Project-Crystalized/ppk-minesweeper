//! Saved hotbar "modes". The default kit is only the starter sticks; while a
//! player is close to a board the hotbar swaps to that board's play items:
//! nine digit blocks near a Sudoku board, and just the lock/flag utility
//! items near a minesweeper board. Entering any mode saves the player's
//! whole hotbar once, switching modes swaps the items, and leaving every
//! mode restores the saved kit. (The endless infinite world instead has a
//! plain kit handed out on teleport/join - see `sticks::apply_infinite_items`.)

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use pumpkin_plugin_api::{ItemStack, ItemStackExt, Player, text::TextComponent};

use crate::{helpers::uuid_key, state::UuidKey};

/// The hotbar swaps when the player is this close to a board (squared
/// distance, to avoid a square root in the tick loop). Sudoku boards can
/// spawn up to ten blocks below the player, so this also covers the
/// worst-case vertical drop.
pub const MODE_RADIUS_SQ: f64 = 14.0 * 14.0;

/// Players already inside a mode leave it at this larger radius, so walking
/// exactly along the movement boundary does not flip the kit back and
/// forth every tick.
pub const MODE_EXIT_RADIUS_SQ: f64 = 16.0 * 16.0;

/// Run the hotbar proximity scan every N ticks (N = 5 -> worst-case 250 ms
/// reaction time) instead of every tick, keeping the tick loop cheap.
pub const SYNC_INTERVAL_TICKS: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Nine colored wool digit blocks in slots 1-9.
    Sudoku,
    /// Just the lock toggle (slot 1) and flag color item (slot 2): no
    /// starter sticks while standing at a minesweeper board.
    Utility,
}

#[derive(Clone)]
struct SavedItem {
    key: String,
    count: u8,
    name: Option<String>,
    lore: Vec<String>,
}

struct ModeState {
    kind: Mode,
    saved: [Option<SavedItem>; 9],
}

fn modes() -> &'static Mutex<HashMap<UuidKey, ModeState>> {
    static MODES: OnceLock<Mutex<HashMap<UuidKey, ModeState>>> = OnceLock::new();
    MODES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn save_slot(player: &Player, slot: u8) -> Option<SavedItem> {
    let stack = player.get_inventory_item(slot)?;
    Some(SavedItem {
        key: stack.get_registry_key(),
        count: stack.get_count().max(1),
        name: stack.get_custom_name().map(|name| name.get_text()),
        lore: stack
            .get_lore()
            .into_iter()
            .map(|line| line.get_text())
            .collect(),
    })
}

fn restore_slot(player: &Player, slot: u8, saved: Option<SavedItem>) {
    let stack = saved.map(|item| {
        let stack = ItemStack::of(item.key, item.count.max(1));
        if let Some(name) = item.name {
            stack.set_custom_name(Some(TextComponent::text(&name)));
        }
        if !item.lore.is_empty() {
            stack.set_lore(
                item.lore
                    .into_iter()
                    .map(|line| TextComponent::text(&line))
                    .collect(),
            );
        }
        stack
    });
    player.set_inventory_item(slot, stack);
}

fn restore_hotbar(player: &Player, saved: [Option<SavedItem>; 9]) {
    for (slot, item) in saved.into_iter().enumerate() {
        restore_slot(player, slot as u8, item);
    }
}

fn apply_mode(player: &Player, kind: Mode) {
    match kind {
        Mode::Sudoku => crate::sudoku::apply_digit_hotbar(player),
        Mode::Utility => crate::sticks::apply_utility_items(player),
    }
}

/// Switch the player's kit: `Some(kind)` puts them into that mode (saving
/// the whole hotbar on first entry), `None` leaves every mode and restores
/// the saved kit.
pub fn sync(player: &Player, mode: Option<Mode>) {
    let key = uuid_key(player);
    match mode {
        Some(kind) => {
            let changed = {
                let mut map = modes().lock().unwrap_or_else(|e| e.into_inner());
                match map.get_mut(&key) {
                    Some(state) if state.kind == kind => false,
                    Some(state) => {
                        state.kind = kind;
                        true
                    }
                    None => {
                        let saved = std::array::from_fn(|slot| save_slot(player, slot as u8));
                        map.insert(key, ModeState { kind, saved });
                        true
                    }
                }
            };
            if changed {
                apply_mode(player, kind);
            }
        }
        None => leave(player),
    }
}

/// Leave whatever mode the player is in and restore their saved hotbar.
pub fn leave(player: &Player) {
    let saved = modes()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&uuid_key(player))
        .map(|state| state.saved);
    if let Some(saved) = saved {
        restore_hotbar(player, saved);
    }
}

/// Drop any saved hotbar without writing it back (used on join, where the
/// player is given a fresh kit anyway).
pub fn forget(owner: UuidKey) {
    modes()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&owner);
}

/// Is the player currently in the given mode?
pub fn in_mode(player: &Player, kind: Mode) -> bool {
    modes()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&uuid_key(player))
        .is_some_and(|state| state.kind == kind)
}

/// Which mode (if any) is the player in right now?
pub fn active_mode(player: &Player) -> Option<Mode> {
    modes()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&uuid_key(player))
        .map(|state| state.kind)
}

/// Pick the active board mode from the two board distances (squared, to
/// the nearest minesweeper board edge and Sudoklu board center). The mode
/// the player is already in gets the wider exit radius so kits do not
/// flap at the movement boundary; with both boards in range, the closer
/// one wins (ties go to minesweeper, matching the sidebar focus).
pub fn choose_mode(
    current: Option<Mode>,
    mine_d2: Option<f64>,
    sudoku_d2: Option<f64>,
) -> Option<Mode> {
    let (mine_limit, sudoku_limit) = match current {
        Some(Mode::Utility) => (MODE_EXIT_RADIUS_SQ, MODE_RADIUS_SQ),
        Some(Mode::Sudoku) => (MODE_RADIUS_SQ, MODE_EXIT_RADIUS_SQ),
        None => (MODE_RADIUS_SQ, MODE_RADIUS_SQ),
    };
    let mine_hot = mine_d2.filter(|d| *d <= mine_limit);
    let sudoku_hot = sudoku_d2.filter(|d| *d <= sudoku_limit);
    match (mine_hot, sudoku_hot) {
        (Some(dm), Some(ds)) => {
            if ds < dm {
                Some(Mode::Sudoku)
            } else {
                Some(Mode::Utility)
            }
        }
        (Some(_), None) => Some(Mode::Utility),
        (_, Some(_)) => Some(Mode::Sudoku),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(blocks: f64) -> f64 {
        blocks.powi(2)
    }

    #[test]
    fn choose_mode_follows_the_closest_board() {
        // Nothing in range.
        assert_eq!(choose_mode(None, Some(r(30.0)), Some(r(30.0))), None);
        // Single candidates.
        assert_eq!(choose_mode(None, Some(r(5.0)), None), Some(Mode::Utility));
        assert_eq!(choose_mode(None, None, Some(r(5.0))), Some(Mode::Sudoku));
        // Both in range: the closer one wins, ties go to minesweeper.
        assert_eq!(
            choose_mode(None, Some(r(10.0)), Some(r(5.0))),
            Some(Mode::Sudoku)
        );
        assert_eq!(
            choose_mode(None, Some(r(5.0)), Some(r(5.0))),
            Some(Mode::Utility)
        );
    }

    #[test]
    fn current_mode_gets_the_wider_exit_radius() {
        // Standing in the hysteresis band (past the entry radius, inside the
        // exit radius) keeps the mode instead of flapping.
        let band = 15.0_f64.powi(2);
        assert_eq!(
            choose_mode(Some(Mode::Utility), Some(band), None),
            Some(Mode::Utility)
        );
        assert_eq!(
            choose_mode(Some(Mode::Sudoku), None, Some(band)),
            Some(Mode::Sudoku)
        );
        // From outside no mode the band is still out of reach.
        assert_eq!(choose_mode(None, Some(band), None), None);
        // Past the exit radius the mode is dropped even with hysteresis.
        assert_eq!(choose_mode(Some(Mode::Utility), Some(r(20.0)), None), None);
    }
}
