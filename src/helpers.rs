//! Thin wrappers over Pumpkin world/player APIs, isolating WIT signatures.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use pumpkin_plugin_api::{
    Player, Server, World,
    common::BlockPos,
    text::TextComponent,
    wit::pumpkin::plugin::{game_rules::GameRule, world::Block, world::BlockFlags},
};

use crate::{
    core::{CellContent, Pos3},
    state::UuidKey,
};

pub use pumpkin_plugin_api::wit::pumpkin::plugin::{
    particles::Particle,
    sounds::{Sound, SoundCategory},
};

/// Board writes notify listeners only; physics/shape updates are skipped
/// (blocks are inert decoration, and they hitch every flood fill).
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
    // Go through the cached state id to avoid re-marshaling the name per cell.
    match state_id(name, None) {
        Some(id) => {
            world.set_block_state(to_block_pos(pos), id, update_flags());
            true
        }
        None => world.set_block_by_name(to_block_pos(pos), name, update_flags()),
    }
}

/// Cheap bulk write: the world-level path runs lighting, physics events and
/// POI updates per cell (~5ms/cell). The chunk API is a raw voxel set that
/// still syncs to clients.
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

    pub fn set(&mut self, pos: Pos3, name: &str) {
        let Some(id) = state_id(name, None) else {
            set_block(self.world, pos, name);
            return;
        };
        self.set_state(pos, id);
    }

    /// State-resolved variant for loops that pre-resolve variant ids.
    pub fn set_state(&mut self, pos: Pos3, id: u16) {
        let (cx, lx) = (pos.x >> 4, pos.x & 15);
        let (cz, lz) = (pos.z >> 4, pos.z & 15);
        let chunk = self
            .chunks
            .entry((cx, cz))
            .or_insert_with(|| self.world.get_chunk(cx, cz));
        let Some(chunk) = chunk else {
            self.world
                .set_block_state(to_block_pos(pos), id, update_flags());
            return;
        };
        chunk.set_block_state(
            BlockPos {
                x: lx,
                y: pos.y,
                z: lz,
            },
            id,
        );
    }
}

/// Resolve a block state id (memoized). `None` when the registry does not
/// know the name, so callers can fall back to `set_block_by_name`.
pub fn state_id_for(name: &str, facing: Option<&str>) -> Option<u16> {
    state_id(name, facing)
}

/// Memoized core; unknown names are cached as `None` too.
fn state_id(name: &str, facing: Option<&str>) -> Option<u16> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<u16>>>> = OnceLock::new();
    let key = match facing {
        Some(face) => format!("{name}#facing={face}"),
        None => name.to_string(),
    };
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
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
        .find(|s| {
            s.get_properties()
                .iter()
                .any(|(k, v)| k == "facing" && v == face)
        })
        .map(|s| s.id)
}

pub fn set_block_facing(world: &World, pos: Pos3, name: &str, facing: &str) -> bool {
    if let Some(id) = state_id(name, Some(facing)) {
        world.set_block_state(to_block_pos(pos), id, update_flags());
        return true;
    }
    set_block(world, pos, name)
}

/// Reveals a cell; digit tiles carry `facing` for Java-RP's tile art.
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

/// Minecraft fades positional sounds out at roughly `volume * 16` blocks, so
/// 1.25 keeps feedback audible to nearby players without carrying across the
/// whole area.
const SOUND_VOLUME: f32 = 1.25;
/// The shard detonation is the main bomb hit.
const CUSTOM_BOMB_VOLUME: f32 = 1.25;
/// The vanilla explosion is only a low-end layer under the detonation.
const BOMB_EXPLOSION_VOLUME: f32 = 0.9;

fn play_named_sound_at_pitched(world: &World, pos: Pos3, name: &str, volume: f32, pitch: f32) {
    world.play_custom_sound(
        name,
        SoundCategory::Ambient,
        (pos.x as f64, pos.y as f64, pos.z as f64),
        volume,
        pitch,
    );
}

fn play_named_sound_at(world: &World, pos: Pos3, name: &str, volume: f32) {
    play_named_sound_at_pitched(world, pos, name, volume, 1.0);
}

/// Positional world sound. NOTE: Pumpkin 0.2.0's `play_sound` builds the
/// lookup name as `format!("{sound:?}").to_lowercase()` ("entityfrogstep"),
/// which traps, so this uses dotted custom sound names instead.
pub fn play_named_sound(world: &World, pos: Pos3, name: &str) {
    play_named_sound_at(world, pos, name, SOUND_VOLUME);
}

pub fn play_named_sound_pitched(world: &World, pos: Pos3, name: &str, pitch: f32) {
    play_named_sound_at_pitched(world, pos, name, SOUND_VOLUME, pitch);
}

pub fn play_sound(world: &World, pos: Pos3, sound: Sound) {
    let name = match sound {
        Sound::BlockWoolPlace => "block.wool.place",
        Sound::BlockWoolBreak => "block.wool.break",
        Sound::ItemGoatHornSound1 => "item.goat_horn.sound.1",
        _ => return,
    };
    play_named_sound(world, pos, name);
}

pub const OPEN_CASCADE_DEFAULT_SOUND: &str = "block.note_block.cow_bell";

fn open_cascade_sound_setting() -> &'static Mutex<String> {
    static SOUND: OnceLock<Mutex<String>> = OnceLock::new();
    SOUND.get_or_init(|| Mutex::new(OPEN_CASCADE_DEFAULT_SOUND.to_string()))
}

pub fn open_cascade_sound() -> String {
    open_cascade_sound_setting()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub fn set_open_cascade_sound(name: &str) {
    *open_cascade_sound_setting()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = name.to_string();
}

const OPEN_CASCADE_MIN_NOTES: usize = 2;
const OPEN_CASCADE_MAX_NOTES: usize = 10;
const OPEN_CASCADE_BIG_THRESHOLD: usize = 25;
const OPEN_CASCADE_BASS_VOLUME: f32 = 1.15;
const OPEN_CASCADE_BASS_PITCH: f32 = 0.85;
const OPEN_CASCADE_NOTE_VOLUME: f32 = 1.25;
const OPEN_CASCADE_MID_VOLUME: f32 = 1.2;
const OPEN_CASCADE_FINAL_VOLUME: f32 = 1.25;
const OPEN_CASCADE_CHIME_VOLUME: f32 = 1.2;
const OPEN_CASCADE_CHIME_PITCH: f32 = 1.2;

/// C-major scale over a tenth, starting at 0.75. Taking the first N notes lets
/// small fills stay low and big fills climb to a bright finish.
const OPEN_CASCADE_PITCHES: [f32; OPEN_CASCADE_MAX_NOTES] = [
    0.75, 0.84375, 0.9375, 1.0, 1.125, 1.25, 1.40625, 1.5, 1.6875, 1.875,
];

struct OpenCascade {
    world_name: String,
    origin: Pos3,
    positions: Vec<Pos3>,
    big: bool,
    next_tick: u64,
    next_note: usize,
}

fn open_cascades() -> &'static Mutex<Vec<OpenCascade>> {
    static CASCADES: OnceLock<Mutex<Vec<OpenCascade>>> = OnceLock::new();
    CASCADES.get_or_init(|| Mutex::new(Vec::new()))
}

fn open_note_volume(note: usize, total: usize) -> f32 {
    if note == 0 {
        OPEN_CASCADE_NOTE_VOLUME
    } else if note + 1 == total {
        OPEN_CASCADE_FINAL_VOLUME
    } else {
        OPEN_CASCADE_MID_VOLUME
    }
}

pub fn play_open_cascade(world: &World, origin: Pos3, opened: usize, mut positions: Vec<Pos3>) {
    if positions.is_empty() {
        return;
    }
    // sqrt(64) = 8, so the 1.25 factor makes 64 cells hit the 10-note cap.
    let total_notes = ((opened as f64).sqrt() * 1.25).round() as usize;
    let total_notes = total_notes
        .clamp(OPEN_CASCADE_MIN_NOTES, OPEN_CASCADE_MAX_NOTES)
        .min(positions.len());
    positions.sort_by_key(|p| {
        let dx = p.x as i64 - origin.x as i64;
        let dy = p.y as i64 - origin.y as i64;
        let dz = p.z as i64 - origin.z as i64;
        dx * dx + dy * dy + dz * dz
    });
    let selected = if positions.len() <= total_notes {
        positions
    } else {
        let last = positions.len() - 1;
        (0..total_notes)
            .map(|i| positions[i * last / (total_notes - 1)])
            .collect()
    };
    let next_tick = crate::state::now_tick().saturating_add(1);
    open_cascades()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(OpenCascade {
            world_name: world.get_name(),
            origin,
            positions: selected,
            big: opened >= OPEN_CASCADE_BIG_THRESHOLD,
            next_tick,
            next_note: 0,
        });
}

pub fn tick_open_sounds(server: &Server, now: u64) {
    let mut cascades = open_cascades().lock().unwrap_or_else(|e| e.into_inner());
    if cascades.is_empty() {
        return;
    }
    let sound = open_cascade_sound();
    let mut i = 0;
    while i < cascades.len() {
        let cascade = &mut cascades[i];
        if now < cascade.next_tick {
            i += 1;
            continue;
        }
        let total = cascade.positions.len();
        let note = cascade.next_note;
        let pos = cascade.positions[note];
        if let Some(world) = server.get_world_by_name(&cascade.world_name) {
            if note == 0 && cascade.big {
                play_named_sound_at_pitched(
                    &world,
                    cascade.origin,
                    "block.note_block.bass",
                    OPEN_CASCADE_BASS_VOLUME,
                    OPEN_CASCADE_BASS_PITCH,
                );
            }
            play_named_sound_at_pitched(
                &world,
                pos,
                &sound,
                open_note_volume(note, total),
                OPEN_CASCADE_PITCHES[note],
            );
            world.spawn_particle(
                Particle::Note,
                (pos.x as f64 + 0.5, pos.y as f64 + 1.1, pos.z as f64 + 0.5),
                (0.0, 0.0, 0.0),
                0.0,
                1,
            );
            if note + 1 == total {
                play_named_sound_at_pitched(
                    &world,
                    pos,
                    "block.amethyst_block.chime",
                    OPEN_CASCADE_CHIME_VOLUME,
                    OPEN_CASCADE_CHIME_PITCH,
                );
            }
        }
        cascade.next_note += 1;
        if cascade.next_note >= total {
            cascades.swap_remove(i);
        } else {
            cascade.next_tick = now.saturating_add(1);
            i += 1;
        }
    }
}

pub fn play_open_strong(world: &World, pos: Pos3) {
    play_named_sound(world, pos, "block.moss.break");
}

pub fn play_bomb(world: &World, pos: Pos3) {
    play_named_sound_at(
        world,
        pos,
        "crystalized:effect.shard.detonation",
        CUSTOM_BOMB_VOLUME,
    );
    play_named_sound_at(world, pos, "entity.generic.explode", BOMB_EXPLOSION_VOLUME);
    world.spawn_particle(
        Particle::RaidOmen,
        (pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5),
        (0.7, 0.7, 0.7),
        0.05,
        64,
    );
}

/// Peaceful-minigame gamerules: no mob spawns, TNT or mob griefing. Applied
/// to every loaded world and to the infinite world on creation.
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
