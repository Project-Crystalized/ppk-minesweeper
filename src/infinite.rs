//! Infinite minesweeper: a dedicated flat world where the terrain itself is
//! the board. Content is derived deterministically from coordinates, opened /
//! flagged state is shared in memory, streaks are per player.

use std::{collections::HashSet, sync::OnceLock};

use pumpkin_plugin_api::{
    Block, ChunkBuffer, Dimension, Player, Server, World,
    worldgen::{ChunkGenerator, GeneratorManager},
};

use crate::{
    core::{
        infinite_is_bomb, infinite_number_at, revealed_block_name, CellContent, CellState, Pos3,
        COVERED_BLOCK, FLAG_BLOCK,
    },
    helpers::{action_bar, block_is, block_name, is_air, play_sound, set_block, tell, to_block_pos, uuid_key, Sound},
    state,
};

pub const INFINITE_WORLD_NAME: &str = "ms_infinite";
pub const BOARD_Y: i32 = 64;
/// Fixed board seed so the world is identical for everyone and across restarts.
pub const BOARD_SEED: u64 = 0x5EED_BEEF_00C0_FFEE;

fn moss_state_id() -> Option<u16> {
    static ID: OnceLock<u16> = OnceLock::new();
    if let Some(id) = ID.get() {
        return Some(*id);
    }
    // Retry until the registry knows the block (world may not be up at load).
    if let Some(id) = Block::from_name("moss_block").map(|b| b.get_default_state().id) {
        let _ = ID.set(id);
        Some(id)
    } else {
        None
    }
}

fn stone_state_id() -> Option<u16> {
    static ID: OnceLock<u16> = OnceLock::new();
    if let Some(id) = ID.get() {
        return Some(*id);
    }
    if let Some(id) = Block::from_name("stone").map(|b| b.get_default_state().id) {
        let _ = ID.set(id);
        Some(id)
    } else {
        None
    }
}

pub struct InfiniteGenerator;

impl ChunkGenerator for InfiniteGenerator {
    fn generate_noise(&self, chunk: &mut ChunkBuffer) {
        let (Some(moss), Some(stone)) = (moss_state_id(), stone_state_id()) else {
            return;
        };
        let min_y = chunk.min_y();
        for x in 0..16u8 {
            for z in 0..16u8 {
                if BOARD_Y > min_y {
                    chunk.fill_range(x, min_y, BOARD_Y - 1, z, stone);
                }
                chunk.set_block(x, BOARD_Y, z, moss);
            }
        }
    }
}

fn generator_slot() -> &'static OnceLock<u32> {
    static ID: OnceLock<u32> = OnceLock::new();
    &ID
}

fn generator_id() -> Option<u32> {
    generator_slot().get().copied()
}

/// Resolve block ids and register the chunk generator. Idempotent.
pub fn register_generator() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let _ = moss_state_id();
        let _ = stone_state_id();
        let id = GeneratorManager::register(InfiniteGenerator);
        let _ = generator_slot().set(id);
    });
}

/// Fetch or create the infinite world and attach our generator.
pub fn ensure_world(server: &Server) -> World {
    let world = match server.get_world_by_name(INFINITE_WORLD_NAME) {
        Some(w) => w,
        None => server.create_world(INFINITE_WORLD_NAME, Dimension::Overworld),
    };
    crate::helpers::apply_peaceful_rules(&world);
    let id = world.get_id();
    tracing::info!("infinite: world id={id} generator={:?}", generator_id());
    let _ = infinite_id_slot().set(id);
    if let Some(gen_id) = generator_id() {
        world.set_chunk_generator(gen_id);
    }
    world
}

fn infinite_id_slot() -> &'static OnceLock<String> {
    static ID: OnceLock<String> = OnceLock::new();
    &ID
}

pub fn is_infinite_world(world: &World) -> bool {
    let id = world.get_id();
    if let Some(known) = infinite_id_slot().get() {
        id == *known
    } else {
        // No teleport happened yet this session (e.g. right after a restart
        // with players already inside): fall back to the well-known name.
        id == INFINITE_WORLD_NAME
    }
}

fn content_at(x: i32, z: i32) -> CellContent {
    if infinite_is_bomb(x, z, BOARD_SEED) {
        CellContent::Bomb
    } else {
        match infinite_number_at(x, z, BOARD_SEED) {
            0 => CellContent::Empty,
            n => CellContent::Number(n),
        }
    }
}

pub fn teleport_here(player: &Player, server: &Server) {
    let world = ensure_world(server);
    // Land on top of whatever is actually there (fresh flat worlds and
    // pre-existing terrain both work).
    let top = world.get_top_block_y(8, 8);
    let ty = (top + 1).max(BOARD_Y + 1);
    tracing::info!("infinite: teleport to 8.5, {ty}, 8.5 (top={top})");
    // Make sure the arrival pad exists if the board plane is exposed.
    let pad = Pos3::new(8, BOARD_Y, 8);
    if is_air(&world, pad) {
        set_block(&world, pad, COVERED_BLOCK);
    }
    player.teleport((8.5, ty as f64, 8.5), None, None, world);
    tell(player, "Welcome to infinite minesweeper! Open cells, avoid TNT.");
}

/// Left-click in the infinite world. Returns true if intercepted.
pub fn handle_break(world: &World, player: &Player, x: i32, y: i32, z: i32) -> bool {
    tracing::info!(
        "infinite: break at {x}, {y}, {z} (board_y={BOARD_Y}, world={})",
        world.get_id()
    );
    if y != BOARD_Y {
        return false;
    }
    let key = uuid_key(player);
    let content = content_at(x, z);

    enum Outcome {
        Bomb,
        Opened(usize),
        Ignored,
    }
    let outcome = state::with_infinite_board_mut(&world.get_id(), |board| {
        let entry = board.cells.get(&(x, z)).copied().unwrap_or(CellState::Closed);
        if entry != CellState::Closed {
            return Outcome::Ignored;
        }
        match content {
            CellContent::Bomb => {
                board.cells.insert((x, z), CellState::Opened);
                set_block(world, Pos3::new(x, y, z), revealed_block_name(content));
                Outcome::Bomb
            }
            CellContent::Empty => {
                // Flood fill over empties, bounded per click.
                let mut opened = 0usize;
                let mut stack = vec![(x, z)];
                let mut seen = HashSet::new();
                while let Some((cx, cz)) = stack.pop() {
                    if !seen.insert((cx, cz)) {
                        continue;
                    }
                    if opened >= 3000 {
                        break;
                    }
                    let st = board.cells.get(&(cx, cz)).copied().unwrap_or(CellState::Closed);
                    if st != CellState::Closed {
                        continue;
                    }
                    board.cells.insert((cx, cz), CellState::Opened);
                    set_block(
                        world,
                        Pos3::new(cx, BOARD_Y, cz),
                        revealed_block_name(content_at(cx, cz)),
                    );
                    opened += 1;
                    if content_at(cx, cz) == CellContent::Empty {
                        for dz in -1..=1 {
                            for dx in -1..=1 {
                                stack.push((cx + dx, cz + dz));
                            }
                        }
                    }
                }
                Outcome::Opened(opened)
            }
            CellContent::Number(_) => {
                board.cells.insert((x, z), CellState::Opened);
                set_block(world, Pos3::new(x, y, z), revealed_block_name(content));
                Outcome::Opened(1)
            }
        }
    });

    match outcome {
        Outcome::Ignored => true, // swallow breaks on already-open cells
        Outcome::Bomb => {
            play_sound(world, Pos3::new(x, y, z), Sound::EntityGenericExplode);
            state::with_session_mut(key, |s| {
                s.current_streak = 0;
                s.total_bombs += 1;
            });
            tell(player, "You clicked a bomb, streak reset!");
            overlay(player, key);
            true
        }
        Outcome::Opened(n) => {
            play_sound(world, Pos3::new(x, y, z), Sound::EntityFrogStep);
            state::with_session_mut(key, |s| {
                s.total_opened += n;
                s.current_streak += n;
                s.top_streak = s.top_streak.max(s.current_streak);
            });
            overlay(player, key);
            true
        }
    }
}

/// Right-click flag toggle in the infinite world. Returns true if intercepted.
pub fn handle_interact(world: &World, _player: &Player, x: i32, y: i32, z: i32) -> bool {
    tracing::info!(
        "infinite: interact at {x}, {y}, {z} (board_y={BOARD_Y}, world={})",
        world.get_id()
    );
    if y != BOARD_Y {
        return false;
    }
    let now = state::now_tick();
    let world_id = world.get_id();
    if state::infinite_flag_locked(&world_id, x, z, now) {
        return true; // off-hand duplicate / spam: swallow, keep cancelled
    }
    state::lock_infinite_flag(&world_id, x, z, now + state::FLAG_LOCK_TICKS);
    // Flags live and die by the visible block: wool unflags, moss flags,
    // anything already revealed is left alone.
    let pos = Pos3::new(x, y, z);
    match block_name(world, to_block_pos(pos)).as_str() {
        b if block_is(b, FLAG_BLOCK) => {
            state::with_infinite_board_mut(&world_id, |board| {
                board.cells.insert((x, z), CellState::Closed);
            });
            set_block(world, pos, COVERED_BLOCK);
            play_sound(world, pos, Sound::BlockWoolBreak);
        }
        b if block_is(b, COVERED_BLOCK) => {
            state::with_infinite_board_mut(&world_id, |board| {
                board.cells.insert((x, z), CellState::Flagged);
            });
            set_block(world, pos, FLAG_BLOCK);
            play_sound(world, pos, Sound::BlockWoolPlace);
        }
        _ => {}
    }
    true
}

fn overlay(player: &Player, key: crate::state::UuidKey) {
    state::with_session_mut(key, |s| {
        action_bar(
            player,
            &format!(
                "Streak: {}  Best: {}  Opened: {}",
                s.current_streak, s.top_streak, s.total_opened
            ),
        );
    });
}
