//! Saved hotbar "modes". The default kit is only the starter sticks; while a
//! player is close to a board the hotbar swaps to that board's play items:
//! nine digit blocks near a Sudoku board, and just the lock/flag utility
//! items near a minesweeper board. Entering any mode saves the player's
//! whole hotbar once, switching modes swaps the items, and leaving every
//! mode restores the saved kit.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use pumpkin_plugin_api::{ItemStack, ItemStackExt, Player, text::TextComponent};

use crate::{
    helpers::{action_bar_component, uuid_key},
    state::UuidKey,
};

/// The hotbar swaps when the player is this close to a board (squared
/// distance, to avoid a square root in the tick loop). Sudoku boards can
/// spawn up to ten blocks below the player, so this also covers the
/// worst-case vertical drop.
pub const MODE_RADIUS_SQ: f64 = 14.0 * 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Nine colored concrete digit blocks in slots 1-9.
    Sudoku,
    /// Just the lock toggle (slot 1) and flag color item (slot 2): no
    /// starter sticks while standing at a minesweeper board.
    Utility,
}

impl Mode {
    fn message(self) -> &'static str {
        match self {
            Self::Sudoku => "Sudoku mode: select 1-9, right-click to place, left-click to clear",
            Self::Utility => {
                "Minesweeper mode: sticks swapped for lock/flag; step away to restore your kit"
            }
        }
    }
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
                action_bar_component(player, TextComponent::text(kind.message()));
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
