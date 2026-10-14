//! Starter sticks: named sticks that start games on right-click,
//! mirroring the old Valence `StartItemType` items.

use pumpkin_plugin_api::{
    common::Hand, text::TextComponent, ItemStack, ItemStackExt, Player,
};

use crate::{games::NewGameSpec, helpers::tell};

/// (display name, game) pairs. Names match the old Valence items.
const STARTERS: [(&str, NewGameSpec); 4] = [
    ("Minesweeper30x30", NewGameSpec::TWO_D_30),
    ("Minesweeper50x50", NewGameSpec::TWO_D_50),
    ("Minesweeper3d5x5", NewGameSpec::THREE_D_5),
    ("Minesweeper3d7x7", NewGameSpec::THREE_D_7),
];

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

/// Starter held in either hand, main hand first.
pub fn held_starter(player: &Player) -> Option<NewGameSpec> {
    for hand in [Hand::Right, Hand::Left] {
        if let Some(stack) = player.get_item_in_hand(hand) {
            tracing::info!(
                "stick-check: hand={hand:?} key={} custom={:?}",
                stack.get_registry_key(),
                stack.get_custom_name().map(|c| c.get_text()),
            );
            if let Some(spec) = starter_for(&stack) {
                return Some(spec);
            }
        }
    }
    None
}

fn has_starter(player: &Player, name: &str) -> bool {
    (0..45u8).any(|slot| {
        player.get_inventory_item(slot).is_some_and(|s| {
            s.matches_item("stick") && s.get_custom_name().is_some_and(|c| c.get_text() == name)
        })
    })
}

/// Give missing starter sticks into first-empty slots (hotbar preferred).
pub fn give_kit(player: &Player) -> usize {
    let mut given = 0;
    for (name, _) in STARTERS {
        if has_starter(player, name) {
            continue;
        }
        let slot = (36..45u8)
            .chain(9..36u8)
            .chain(0..9u8)
            .find(|slot| player.get_inventory_item(*slot).is_none());
        if let Some(slot) = slot {
            player.set_inventory_item(slot, Some(make_stick(name)));
            tracing::info!("stick-kit: gave {name} in slot {slot}");
            given += 1;
        } else {
            tracing::info!("stick-kit: no empty slot for {name}");
        }
    }
    given
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
