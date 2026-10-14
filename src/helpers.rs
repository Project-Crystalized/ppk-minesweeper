//! Thin wrappers over Pumpkin world/player APIs, isolating WIT signatures.

use pumpkin_plugin_api::{
    common::BlockPos, text::TextComponent,
    wit::pumpkin::plugin::{game_rules::GameRule, world::BlockFlags},
    Player, World,
};

use crate::{core::Pos3, state::UuidKey};

pub use pumpkin_plugin_api::wit::pumpkin::plugin::sounds::{Sound, SoundCategory};

/// Flags for plugin board writes: notify listeners (client sync) only.
/// Neighbor updates are skipped on purpose — board blocks are inert
/// decoration, and fanning out physics/shape updates on every flood-fill
/// click causes a visible hitch.
pub fn update_flags() -> BlockFlags {
    BlockFlags::NOTIFY_LISTENERS
}

pub fn to_block_pos(p: Pos3) -> BlockPos {
    BlockPos {
        x: p.x,
        y: p.y,
        z: p.z,
    }
}

pub fn to_pos3(p: BlockPos) -> Pos3 {
    Pos3::new(p.x, p.y, p.z)
}

pub fn block_name(world: &World, pos: BlockPos) -> String {
    world.get_block(pos).name
}

pub fn set_block(world: &World, pos: Pos3, name: &str) -> bool {
    world.set_block_by_name(to_block_pos(pos), name, update_flags())
}

pub fn is_air_name(name: &str) -> bool {
    // Compare the path segment only: some hosts report "air", others
    // "minecraft:air" (plus cave/void variants).
    match name.rsplit(':').next().unwrap_or(name) {
        "air" | "cave_air" | "void_air" => true,
        _ => false,
    }
}

/// Namespace-insensitive block name comparison: the host may report
/// "moss_block" where we write "minecraft:moss_block" (both forms are
/// accepted on write, but reads must match either).
pub fn block_is(name: &str, expected: &str) -> bool {
    name == expected || name.rsplit(':').next() == expected.rsplit(':').next()
}

pub fn is_air(world: &World, pos: Pos3) -> bool {
    is_air_name(&block_name(world, to_block_pos(pos)))
}

pub fn uuid_key(player: &Player) -> UuidKey {
    let id = player.get_id();
    (id.high, id.low)
}

pub fn msg(text: &str) -> TextComponent {
    TextComponent::text(text)
}

pub fn tell(player: &Player, text: &str) {
    player.send_system_message(msg(text), false);
}

pub fn action_bar(player: &Player, text: &str) {
    player.send_system_message(msg(text), true);
}

/// Positional world sound, mirroring the old Valence sound cues.
///
/// NOTE: this deliberately uses `play_custom_sound` with explicit vanilla
/// names instead of `play_sound`: Pumpkin 0.2.0's host builds the lookup name
/// via `format!("{sound:?}").to_lowercase()` which yields "entityfrogstep"
/// (no dots), so `play_sound` traps for EVERY sound. Raw dotted names work.
pub fn play_sound(world: &World, pos: Pos3, sound: Sound) {
    let name = match sound {
        Sound::BlockWoolPlace => "block.wool.place",
        Sound::BlockWoolBreak => "block.wool.break",
        Sound::EntityGenericExplode => "entity.generic.explode",
        Sound::EntityFrogStep => "entity.frog.step",
        Sound::ItemGoatHornSound1 => "item.goat_horn.sound.1",
        _ => return,
    };
    world.play_custom_sound(
        name,
        SoundCategory::Ambient,
        (pos.x as f64, pos.y as f64, pos.z as f64),
        1.0,
        1.0,
    );
}

/// Peaceful-minigame gamerules: no natural/spawner/patrol/phantom/trader/
/// warden spawns, no TNT, no mob griefing (e.g. endermen stealing boards).
/// Applied to every loaded world on startup and to the infinite world on
/// creation. This does the real work: 0.2.0 never fires the spawn events.
pub fn apply_peaceful_rules(world: &World) {
    for rule in [
        GameRule::SpawnMobs,
        GameRule::SpawnMonsters,
        GameRule::SpawnerBlocksWork,
        GameRule::SpawnPatrols,
        GameRule::SpawnPhantoms,
        GameRule::SpawnWanderingTraders,
        GameRule::SpawnWardens,
        GameRule::TntExplodes,
        GameRule::MobGriefing,
    ] {
        world.set_game_rule_bool(rule, false);
    }
}
