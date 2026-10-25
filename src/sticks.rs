//! Starter sticks: named sticks that start games on right-click.

use pumpkin_plugin_api::{
    common::Hand, text::TextComponent, ItemStack, ItemStackExt, Player,
};

use crate::games::NewGameSpec;

const STARTERS: [(&str, NewGameSpec); 5] = [
    ("Minesweeper20x20", NewGameSpec::TWO_D_20),
    ("Minesweeper30x30", NewGameSpec::TWO_D_30),
    ("Minesweeper50x50", NewGameSpec::TWO_D_50),
    ("Minesweeper3d5x5", NewGameSpec::THREE_D_5),
    ("Minesweeper3d7x7", NewGameSpec::THREE_D_7),
];

pub const LOCKED_NAME: &str = "Game is Locked";
pub const UNLOCKED_NAME: &str = "Game is Unlocked";
pub const FLAG_SWITCH_NAME: &str = "Switch flag color";

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
    let stack = ItemStack::of(item_name, 1);
    stack.set_custom_name(Some(TextComponent::text(FLAG_SWITCH_NAME)));
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

/// Pumpkin storage layout: `get/set_inventory_item` address the raw
/// `PlayerInventory`, where **0-35 is the main inventory with 0-8 as the
/// hotbar**, 36-39 are armor, and 40 is the offhand.
///
/// Pumpkin 0.2.1's plugin host syncs writes through
/// `Player::sync_hand_slot`, sending the raw storage index in
/// `CSetPlayerInventory` and refreshing the player screen. That makes
/// hotbar slots 0-8 safe to write directly (older hosts used the raw index
/// as a window slot and needed the main-grid workaround).
pub const OFFHAND_SLOT: u8 = 40;
const LOCK_SLOT: u8 = 7;
const FLAG_SLOT: u8 = 8;

/// Fixed hotbar layout: five starters in slots 0-4, then the lock toggle
/// and flag color switcher in slots 7-8. Slots 5-6 stay empty.
fn kit_layout(player: &Player) -> [(u8, ItemStack); 7] {
    use crate::helpers::uuid_key;
    let key = uuid_key(player);
    let any_locked = crate::state::all_games_detailed()
        .iter()
        .any(|g| g.owner == key && g.locked);
    [
        (0, make_stick(STARTERS[0].0)),
        (1, make_stick(STARTERS[1].0)),
        (2, make_stick(STARTERS[2].0)),
        (3, make_stick(STARTERS[3].0)),
        (4, make_stick(STARTERS[4].0)),
        (LOCK_SLOT, make_lock_item(any_locked)),
        (
            FLAG_SLOT,
            make_flag_item(crate::state::flag_block_for(key)),
        ),
    ]
}

/// Wipe main/armor/offhand, then lay out the 7-piece hotbar kit. Slots 41+
/// are never touched (the client would render ghost duplicates there).
pub fn give_fresh_kit(player: &Player) -> usize {
    player.get_inventory().clear_all();
    for slot in 0..=OFFHAND_SLOT {
        player.set_inventory_item(slot, None);
    }
    let mut given = 0;
    for (slot, stack) in kit_layout(player) {
        player.set_inventory_item(slot, Some(stack));
        given += 1;
    }
    given
}

/// Update the hotbar utility item names to the current lock state/flag color.
pub fn refresh_utility_items(player: &Player) {
    use crate::helpers::uuid_key;
    let key = uuid_key(player);
    let any_locked = crate::state::all_games_detailed()
        .iter()
        .any(|g| g.owner == key && g.locked);
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
