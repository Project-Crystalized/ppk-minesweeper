//! Starter sticks: named sticks that start games on right-click,
//! mirroring the old Valence `StartItemType` items.

use pumpkin_plugin_api::{
    common::Hand, text::TextComponent, ItemStack, ItemStackExt, Player,
};

use crate::games::NewGameSpec;

/// (display name, game) pairs. Names match the old Valence items.
const STARTERS: [(&str, NewGameSpec); 6] = [
    ("Minesweeper20x20", NewGameSpec::TWO_D_20),
    ("Minesweeper30x30", NewGameSpec::TWO_D_30),
    ("Minesweeper50x50", NewGameSpec::TWO_D_50),
    ("Minesweeper3d5x5", NewGameSpec::THREE_D_5),
    ("Minesweeper3d7x7", NewGameSpec::THREE_D_7),
    ("Minesweeper3d10x10", NewGameSpec::THREE_D_10),
];

/// Slot-43 lock item names (mirrors old `GameLock` horse-armor items).
pub const LOCKED_NAME: &str = "Game is Locked";
pub const UNLOCKED_NAME: &str = "Game is Unlocked";
/// Slot-44 flag color item name (mirrors old `FlagType` wool item).
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

/// Which game (if any) the held stack starts.
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

/// Lock/flag utility held in either hand (checked after starters).
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

/// Utility item (lock toggle or flag cycle) held in either hand.
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
    // Wool block item matching the flag color (e.g. "minecraft:red_wool").
    let item_name = flag_block.rsplit(':').next().unwrap_or("red_wool");
    let stack = ItemStack::of(item_name, 1);
    stack.set_custom_name(Some(TextComponent::text(FLAG_SWITCH_NAME)));
    stack
}

/// Starter held in either hand, main hand first.
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

/// Pumpkin storage slot layout (verified against the host
/// `pumpkin-inventory` crate): `get/set_inventory_item` address the raw
/// `PlayerInventory` storage, where **0-35 is the main inventory with 0-8
/// as the hotbar**, 36-39 are armor and 40 is the offhand.
///
/// WARNING: the host forwards that index verbatim as the *window* slot in
/// `CSetContainerSlot` (`CSetContainerSlot::new(0, 0, slot)`), but vanilla
/// window 0 numbers slots differently: 0 = crafting output, 1-4 = crafting,
/// 5-8 = armor, 9-35 = main, 36-44 = hotbar, 45 = offhand. The host's own
/// typed setters translate (e.g. chestplate storage 38 -> packet slot 6,
/// offhand storage 40 -> packet slot 45); the generic per-slot API does
/// not. Consequence: only storage slots **9-35 render where they live**.
/// Anything placed at 0-8 shows up in the crafting/armor area (invisible in
/// the HUD, appearing as a "second set" when the inventory is opened), and
/// 36-40 render over the hotbar while actually sitting in equipment slots.
/// So the kit lives in the main grid (9-15) plus one starter in the offhand
/// (via the translating typed setter), and generic writes never touch 0-8
/// or 36-40.
/// Last storage slot: 36-39 armor, 40 offhand.
pub const OFFHAND_SLOT: u8 = 40;
/// Display-safe grid range: storage and window numbering agree here.
const GRID_START: u8 = 9;

/// Fixed main-grid layout: 6 starters, then the lock toggle and the flag
/// color switcher (slots 9-16).
fn kit_grid_layout(player: &Player) -> [(u8, ItemStack); 8] {
    use crate::helpers::uuid_key;
    let key = uuid_key(player);
    let any_locked = crate::state::all_games_detailed()
        .iter()
        .any(|g| g.owner == key && g.locked);
    [
        (9, make_stick(STARTERS[0].0)),
        (10, make_stick(STARTERS[1].0)),
        (11, make_stick(STARTERS[2].0)),
        (12, make_stick(STARTERS[3].0)),
        (13, make_stick(STARTERS[4].0)),
        (14, make_stick(STARTERS[5].0)),
        (15, make_lock_item(any_locked)),
        (
            16,
            make_flag_item(crate::state::flag_block_for(key)),
        ),
    ]
}

/// Join flow: wipe everything (main, armor, offhand) so no stale kit pieces
/// survive anywhere, then lay out the full 8-piece kit in the display-safe
/// grid. Equipment slots are cleared explicitly on top of `clear_all` so a
/// host that ever changes `clear_all` semantics can't leave ghosts behind.
/// Slots 41+ are never touched: the server stores nothing there while the
/// client would still render an item (ghost duplicates). The offhand is
/// deliberately left empty.
pub fn give_fresh_kit(player: &Player) -> usize {
    player.get_inventory().clear_all();
    for slot in 0..=OFFHAND_SLOT {
        player.set_inventory_item(slot, None);
    }
    let mut given = 0;
    for (slot, stack) in kit_grid_layout(player) {
        player.set_inventory_item(slot, Some(stack));
        given += 1;
    }
    given
}

/// Refresh the lock item name(s) to the player's current aggregate lock
/// state and the flag item to the current flag color. Skips hotbar slots
/// 0-8: those render at window slots 0-8 (crafting area) through this API,
/// so rewriting them would visually teleport a correctly-placed hotbar item
/// away. A hotbar utility item keeps a stale name until the next rewrite
/// from the grid rather than jumping around the screen.
pub fn refresh_utility_items(player: &Player) {
    use crate::helpers::uuid_key;
    let key = uuid_key(player);
    let any_locked = crate::state::all_games_detailed()
        .iter()
        .any(|g| g.owner == key && g.locked);
    let flag_block = crate::state::flag_block_for(key);
    for slot in 0..=OFFHAND_SLOT {
        if slot < GRID_START {
            continue;
        }
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
