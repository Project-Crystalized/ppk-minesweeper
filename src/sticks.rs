//! Starter sticks: named sticks that start games on right-click.

use pumpkin_plugin_api::{ItemStack, ItemStackExt, Player, common::Hand, text::TextComponent};

use crate::games::NewGameSpec;

const STARTERS: [(&str, NewGameSpec); 4] = [
    ("Minesweeper20x20", NewGameSpec::TWO_D_20),
    ("Minesweeper40x40", NewGameSpec::TWO_D_40),
    ("Minesweeper3d5x5", NewGameSpec::THREE_D_5),
    ("Minesweeper3d7x7", NewGameSpec::THREE_D_7),
];

pub const LOCKED_NAME: &str = "Game is Locked";
pub const UNLOCKED_NAME: &str = "Game is Unlocked";
pub const FLAG_SWITCH_NAME: &str = "Switch flag color";
pub const SUDOKU_NAME: &str = "Sudoku";

pub enum HeldSpecial {
    LockToggle,
    FlagCycle,
}

pub fn make_stick(name: &str) -> ItemStack {
    let stack = ItemStack::of("stick", 1);
    stack.set_custom_name(Some(TextComponent::text(name)));
    stack
}

pub fn starter_for(stack: &ItemStack) -> Option<NewGameSpec> {
    if !stack.matches_item("stick") {
        return None;
    }
    let text = stack.get_custom_name()?.get_text();
    STARTERS
        .iter()
        .find(|(name, _)| *name == text)
        .map(|(_, spec)| *spec)
}

pub fn is_sudoku_stick(stack: &ItemStack) -> bool {
    stack.matches_item("stick")
        && stack
            .get_custom_name()
            .is_some_and(|name| name.get_text() == SUDOKU_NAME)
}

pub fn special_for(stack: &ItemStack) -> Option<HeldSpecial> {
    let text = stack.get_custom_name()?.get_text();
    if text == LOCKED_NAME || text == UNLOCKED_NAME {
        return Some(HeldSpecial::LockToggle);
    }
    if text == FLAG_SWITCH_NAME {
        return Some(HeldSpecial::FlagCycle);
    }
    None
}

pub fn held_special(player: &Player) -> Option<HeldSpecial> {
    for hand in [Hand::Right, Hand::Left] {
        if let Some(stack) = player.get_item_in_hand(hand) {
            if let Some(special) = special_for(&stack) {
                return Some(special);
            }
        }
    }
    None
}

pub fn make_lock_item(locked: bool) -> ItemStack {
    let stack = ItemStack::of(
        if locked {
            "leather_horse_armor"
        } else {
            "iron_horse_armor"
        },
        1,
    );
    stack.set_custom_name(Some(TextComponent::text(if locked {
        LOCKED_NAME
    } else {
        UNLOCKED_NAME
    })));
    stack
}

pub fn make_flag_item(flag_block: &str) -> ItemStack {
    let item_name = flag_block.rsplit(':').next().unwrap_or("red_wool");
    let color = item_name.strip_suffix("_wool").unwrap_or(item_name);
    let stack = ItemStack::of(item_name, 1);
    stack.set_custom_name(Some(TextComponent::text(FLAG_SWITCH_NAME)));
    stack.set_lore(vec![
        TextComponent::text(&format!("Current: {}", color.replace('_', " "))),
        TextComponent::text("Right-click: next color"),
        TextComponent::text("Left-click: reset to red"),
    ]);
    stack
}

pub fn held_starter(player: &Player) -> Option<NewGameSpec> {
    for hand in [Hand::Right, Hand::Left] {
        if let Some(stack) = player.get_item_in_hand(hand) {
            if let Some(spec) = starter_for(&stack) {
                return Some(spec);
            }
        }
    }
    None
}

pub fn held_sudoku(player: &Player) -> bool {
    [Hand::Right, Hand::Left]
        .into_iter()
        .filter_map(|hand| player.get_item_in_hand(hand))
        .any(|stack| is_sudoku_stick(&stack))
}

pub const OFFHAND_SLOT: u8 = 40;
const SUDOKU_SLOT: u8 = 4;
pub const UTILITY_LOCK_SLOT: u8 = 7;
pub const UTILITY_FLAG_SLOT: u8 = 8;

/// The default kit: only the starter sticks. Sticks for the four
/// minesweeper sizes in slots 0-3, Sudoku in slot 4.
fn kit_layout() -> [(u8, ItemStack); 5] {
    [
        (0, make_stick(STARTERS[0].0)),
        (1, make_stick(STARTERS[1].0)),
        (2, make_stick(STARTERS[2].0)),
        (3, make_stick(STARTERS[3].0)),
        (SUDOKU_SLOT, make_stick(SUDOKU_NAME)),
    ]
}

/// Wipe main/armor/offhand, then lay out the 5-stick hotbar kit. Slots 41+
/// are never touched (the client would render ghost duplicates there).
pub fn give_fresh_kit(player: &Player) -> usize {
    player.get_inventory().clear_all();
    for slot in 0..=OFFHAND_SLOT {
        player.set_inventory_item(slot, None);
    }
    let mut given = 0;
    for (slot, stack) in kit_layout() {
        player.set_inventory_item(slot, Some(stack));
        given += 1;
    }
    given
}

/// Hotbar-mode layout for players standing at a minesweeper board: the
/// utility items only (lock toggle + flag color switcher). The hotbar is
/// cleared first so no sticks or leftover items linger — entering the mode
/// already saved the player's real hotbar and leaving restores it.
pub fn apply_utility_items(player: &Player) {
    for slot in 0..9u8 {
        player.set_inventory_item(slot, None);
    }
    let key = crate::helpers::uuid_key(player);
    let any_locked = crate::state::all_games_detailed()
        .iter()
        .any(|g| g.owner == key && g.locked)
        || crate::sudoku::any_locked(key);
    player.set_inventory_item(UTILITY_LOCK_SLOT, Some(make_lock_item(any_locked)));
    player.set_inventory_item(
        UTILITY_FLAG_SLOT,
        Some(make_flag_item(crate::state::flag_block_for(key))),
    );
}

/// Update the hotbar utility item names to the current lock state/flag color.
pub fn refresh_utility_items(player: &Player) {
    use crate::helpers::uuid_key;
    let key = uuid_key(player);
    let any_locked = crate::state::all_games_detailed()
        .iter()
        .any(|g| g.owner == key && g.locked)
        || crate::sudoku::any_locked(key);
    let flag_block = crate::state::flag_block_for(key);
    for slot in 0..=OFFHAND_SLOT {
        let Some(stack) = player.get_inventory_item(slot) else {
            continue;
        };
        let Some(name) = stack.get_custom_name().map(|c| c.get_text()) else {
            continue;
        };
        if name == LOCKED_NAME || name == UNLOCKED_NAME {
            player.set_inventory_item(slot, Some(make_lock_item(any_locked)));
        } else if name == FLAG_SWITCH_NAME {
            player.set_inventory_item(slot, Some(make_flag_item(flag_block)));
        }
    }
}
