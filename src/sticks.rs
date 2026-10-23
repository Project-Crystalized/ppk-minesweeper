//! Starter sticks: named sticks that start games on right-click,
//! mirroring the old Valence `StartItemType` items.

use pumpkin_plugin_api::{
    common::Hand, text::TextComponent, ItemStack, ItemStackExt, Player,
};

use crate::{games::NewGameSpec, helpers::tell};

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
const GRID_END: u8 = 36; // exclusive

/// Fixed main-grid layout: 5 starters, then the lock toggle and the flag
/// color switcher. (The 6th starter rides in the offhand, see below.)
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

fn has_named(player: &Player, name: &str) -> bool {
    // Full storage range: kit pieces stranded in armor/offhand slots (from
    // before the hotbar fix) must still count as "already have", or `/ms kit`
    // grants a second copy.
    (0..=OFFHAND_SLOT).any(|slot| {
        player
            .get_inventory_item(slot)
            .is_some_and(|s| s.get_custom_name().is_some_and(|c| c.get_text() == name))
    })
}

fn first_free_grid_slot(player: &Player) -> Option<u8> {
    (GRID_START..GRID_END).find(|slot| player.get_inventory_item(*slot).is_none())
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

/// Remove duplicate kit pieces across the whole storage (0-40) and relocate
/// lone equipment-slot copies into the grid when there is room. Keeps one
/// copy per piece, preferring the display-safe grid. Returns stacks removed.
pub fn dedupe_kit(player: &Player) -> usize {
    let mut removed = 0;
    for (name, _) in STARTERS {
        removed += dedupe_named(player, name, None);
    }
    removed += dedupe_named(player, FLAG_SWITCH_NAME, None);
    // Lock toggle: two display names, one identity.
    removed += dedupe_named(player, LOCKED_NAME, Some(UNLOCKED_NAME));
    removed
}

/// Keep a single copy of the item called `primary` (or `alias`): a copy in
/// the display-safe grid wins, otherwise the first copy overall. A lone copy
/// stuck in an equipment slot moves to the grid when a slot is free, else it
/// stays; every other occurrence is cleared.
fn dedupe_named(player: &Player, primary: &str, alias: Option<&str>) -> usize {
    let mut hits = Vec::new();
    for slot in 0..=OFFHAND_SLOT {
        let hit = player.get_inventory_item(slot).is_some_and(|s| {
            s.get_custom_name()
                .is_some_and(|c| c.get_text() == primary || alias.is_some_and(|a| c.get_text() == a))
        });
        if hit {
            hits.push(slot);
        }
    }
    if hits.is_empty() {
        return 0;
    }
    // Grid copy wins (renders correctly); otherwise the first copy overall.
    let mut keeper = hits
        .iter()
        .find(|s| (GRID_START..GRID_END).contains(s))
        .or(hits.first())
        .copied()
        .unwrap_or(hits[0]);
    // A keeper stuck in an equipment slot (36-40) shows in the wrong place:
    // move it to the grid when a slot is free, else leave it.
    if keeper >= GRID_END {
        if let Some(free) = first_free_grid_slot(player) {
            if let Some(stack) = player.get_inventory_item(keeper) {
                player.set_inventory_item(free, Some(stack));
                player.set_inventory_item(keeper, None);
                keeper = free;
            }
        }
    }
    let mut removed = 0;
    for slot in hits {
        if slot == keeper {
            continue;
        }
        player.set_inventory_item(slot, None);
        removed += 1;
    }
    removed
}

/// Additive top-up (`/ms kit`): first collapse any duplicates (and pull
/// equipment-slot strays back to the grid), then grant only truly missing
/// pieces. Grid pieces prefer their fixed layout slot and fall back to the
/// first free grid slot. Generic writes never touch 0-8 or 36-40 (host
/// renders those in the wrong place); the offhand is left alone.
pub fn give_kit(player: &Player) -> usize {
    dedupe_kit(player);
    refresh_utility_items(player);
    let mut given = 0;
    let place_grid = |player: &Player, slot: u8, stack: ItemStack| {
        if player.get_inventory_item(slot).is_none() {
            player.set_inventory_item(slot, Some(stack));
            true
        } else if let Some(free) = first_free_grid_slot(player) {
            player.set_inventory_item(free, Some(stack));
            true
        } else {
            false
        }
    };
    for (slot, stack) in kit_grid_layout(player) {
        let name = stack
            .get_custom_name()
            .map(|c| c.get_text())
            .unwrap_or_default();
        // The lock piece changes identity with state; either name counts.
        let present = if name == LOCKED_NAME || name == UNLOCKED_NAME {
            has_named(player, LOCKED_NAME) || has_named(player, UNLOCKED_NAME)
        } else {
            has_named(player, &name)
        };
        if present {
            continue;
        }
        if place_grid(player, slot, stack) {
            given += 1;
        }
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

pub fn give_kit_and_tell(player: &Player) {
    let given = give_kit(player);
    if given > 0 {
        tell(
            player,
            "Starter sticks added! Right-click with one to start that game.",
        );
    } else {
        tell(player, "You already have all starter sticks.");
    }
}
