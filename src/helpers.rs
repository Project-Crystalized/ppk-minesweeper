//! Thin wrappers over Pumpkin world/player APIs, isolating WIT signatures.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use pumpkin_plugin_api::{
    common::BlockPos, text::TextComponent,
    wit::pumpkin::plugin::{game_rules::GameRule, world::Block, world::BlockFlags},
    Player, World,
};

use crate::{core::{CellContent, Pos3}, state::UuidKey};

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
    // Same flags as before; going through the cached state id avoids
    // re-marshaling the block name across the WASM->host ABI per cell.
    match state_id(name, None) {
        Some(id) => {
            world.set_block_state(to_block_pos(pos), id, update_flags());
            true
        }
        None => world.set_block_by_name(to_block_pos(pos), name, update_flags()),
    }
}

/// Cheap bulk write: `set_block` measured ~5ms per skylight-occluding cell
/// live (12.3s to build a 2D-50 board). Root cause (verified in the 0.2.0
/// host source): the world-level path runs the light engine, six
/// BlockPhysicsEvent dispatches and POI updates per cell. The chunk-level
/// API is a raw voxel set that still lands in the client broadcast queue,
/// but skips all of that.
pub struct BulkWriter<'w> {
    world: &'w World,
    chunks: HashMap<(i32, i32), Option<pumpkin_plugin_api::wit::pumpkin::plugin::world::Chunk>>,
}

impl<'w> BulkWriter<'w> {
    pub fn new(world: &'w World) -> Self {
        Self {
            world,
            chunks: HashMap::new(),
        }
    }

    /// Fallback: world-level write when the chunk API is unavailable
    /// (unloaded chunk, unknown block). Slower, but correct.
    pub fn set(&mut self, pos: Pos3, name: &str) {
        let Some(id) = state_id(name, None) else {
            set_block(self.world, pos, name);
            return;
        };
        self.set_state(pos, id);
    }

    /// State-resolved variant for loops that pre-resolve (e.g. digit tiles
    /// with `facing` variants). Falls back to the world-level per-cell write
    /// only when the chunk is unloaded.
    pub fn set_state(&mut self, pos: Pos3, id: u16) {
        let (cx, lx) = (pos.x >> 4, pos.x & 15);
        let (cz, lz) = (pos.z >> 4, pos.z & 15);
        let chunk = self.chunks.entry((cx, cz)).or_insert_with(|| self.world.get_chunk(cx, cz));
        let Some(chunk) = chunk else {
            self.world.set_block_state(to_block_pos(pos), id, update_flags());
            return;
        };
        chunk.set_block_state(
            BlockPos { x: lx, y: pos.y, z: lz },
            id,
        );
    }
}

/// Resolve a block state id, memoized across calls. `facing` is the optional
/// `facing` property (used by the digit tiles). `None` means the block's
/// default state. Names unknown to the registry return `None` so callers
/// can fall back to `set_block_by_name`. Public wrapper for bulk loops that
/// pre-resolve variant ids before writing.
pub fn state_id_for(name: &str, facing: Option<&str>) -> Option<u16> {
    state_id(name, facing)
}

/// Memoized core. Cached misses (None) are stored too, so a repeated
/// unknown name doesn't re-demo the registry every call.
fn state_id(name: &str, facing: Option<&str>) -> Option<u16> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<u16>>>> = OnceLock::new();
    let key = match facing {
        Some(face) => format!("{name}#facing={face}"),
        None => name.to_string(),
    };
    let mut cache = CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    if !cache.contains_key(&key) {
        cache.insert(key.clone(), resolve_state_id(name, facing));
    }
    cache[&key]
}

fn resolve_state_id(name: &str, facing: Option<&str>) -> Option<u16> {
    let block = Block::from_name(name)?;
    let Some(face) = facing else {
        return Some(block.default_state_id);
    };
    block
        .get_states()
        .into_iter()
        .find(|s| s.get_properties().iter().any(|(k, v)| k == "facing" && v == face))
        .map(|s| s.id)
}

/// Places a specific `facing` state of a block, resolved through the block
/// registry. Falls back to the block's default state if the lookup fails.
pub fn set_block_facing(world: &World, pos: Pos3, name: &str, facing: &str) -> bool {
    if let Some(id) = state_id(name, Some(facing)) {
        world.set_block_state(to_block_pos(pos), id, update_flags());
        return true;
    }
    set_block(world, pos, name)
}

/// Reveals a minesweeper cell. Digit tiles carry their `facing` so Java-RP
/// can render the matching tile art per block state.
pub fn set_tile(world: &World, pos: Pos3, content: CellContent) -> bool {
    let (name, facing) = crate::core::revealed_tile(content);
    match facing {
        Some(face) => set_block_facing(world, pos, name, face),
        None => set_block(world, pos, name),
    }
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

pub fn action_bar_component(player: &Player, comp: TextComponent) {
    player.send_system_message(comp, true);
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
