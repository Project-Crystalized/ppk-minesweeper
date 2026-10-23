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
        infinite_is_bomb, infinite_number_at, CellContent, CellState, Pos3,
        COVERED_BLOCK,
    },
    helpers::{block_is, block_name, is_air, play_sound, set_block, set_tile, tell, to_block_pos, uuid_key, Sound},
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
    hydrate_session(player);
    tell(player, "Welcome to infinite minesweeper! Open cells, avoid TNT.");
}

/// Left-click in the infinite world. Returns true if intercepted.
pub fn handle_break(
    world: &World,
    server: &Server,
    player: &Player,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
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
                set_tile(world, Pos3::new(x, y, z), content);
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
                    set_tile(world, Pos3::new(cx, BOARD_Y, cz), content_at(cx, cz));
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
                set_tile(world, Pos3::new(x, y, z), content);
                Outcome::Opened(1)
            }
        }
    });

    match outcome {
        Outcome::Ignored => true, // swallow breaks on already-open/flagged cells
        Outcome::Bomb => {
            play_sound(world, Pos3::new(x, y, z), Sound::EntityGenericExplode);
            state::with_session_mut(key, |s| {
                s.current_streak = 0;
                s.bombs_clicked += 1;
            });
            persist_session(key, player);
            tell(player, "You clicked a bomb, streak reset!");
            overlay(player, key);
            true
        }
        Outcome::Opened(n) => {
            play_sound(world, Pos3::new(x, y, z), Sound::EntityFrogStep);
            let milestone = state::with_session_mut(key, |s| {
                s.total_opened += n;
                s.current_streak += n;
                s.top_streak = s.top_streak.max(s.current_streak);
                s.current_streak
            });
            persist_session(key, player);
            overlay(player, key);
            // Milestone broadcasts mirror the old server (500 / 1000 streaks).
            if milestone == 500 || milestone == 1000 {
                let name = player.get_name();
                for p in server.get_all_players() {
                    tell(
                        &p,
                        &format!("{name} has reached a streak of {milestone}."),
                    );
                }
            }
            true
        }
    }
}

/// Write-through lifetime totals (one row per player). Fail-soft. Only the
/// durable numbers go out (best/opened/bombs/playtime); the live streak
/// restarts at 0 after every login (see hydrate_session).
fn persist_session(key: crate::state::UuidKey, player: &Player) {
    let (top, opened, bombs, playtime) = state::with_session_mut(key, |s| {
        (
            s.top_streak,
            s.total_opened,
            s.bombs_clicked,
            s.playtime_ticks,
        )
    });
    crate::db::save_infinite(key, Some(&player.get_name()), top, opened, bombs, playtime);
}

/// Persist a player's lifetime infinite totals (leave + periodic flush).
/// Players who never entered the infinite world have no session and are not
/// given a zero row.
pub fn persist_session_for(key: crate::state::UuidKey, player: &Player) {
    if state::session(key).is_some() {
        persist_session(key, player);
    }
}

/// Restore durable totals after a restart; the live streak always starts at 0.
pub fn hydrate_session(player: &Player) {
    let key = uuid_key(player);
    if let Some(row) = crate::db::load_infinite(key) {
        state::with_session_mut(key, |s| {
            s.current_streak = 0;
            s.top_streak = row.top;
            s.total_opened = row.opened;
            s.bombs_clicked = row.bombs_clicked;
            s.playtime_ticks = row.playtime;
        });
    }
}

/// Right-click flag toggle in the infinite world. Returns true if intercepted.
pub fn handle_interact(world: &World, player: &Player, x: i32, y: i32, z: i32) -> bool {
    if y != BOARD_Y {
        return false;
    }
    let now = state::now_tick();
    let world_id = world.get_id();
    if state::infinite_flag_locked(&world_id, x, z, now) {
        return true; // off-hand duplicate / spam: swallow, keep cancelled
    }
    state::lock_infinite_flag(&world_id, x, z, now + state::FLAG_LOCK_TICKS);
    // Flags live and die by the visible block: any known flag wool unflags,
    // moss flags with the player's own color, anything revealed is ignored.
    let pos = Pos3::new(x, y, z);
    let name = block_name(world, to_block_pos(pos));
    if is_flag_wool(&name) {
        state::with_infinite_board_mut(&world_id, |board| {
            board.cells.insert((x, z), CellState::Closed);
        });
        set_block(world, pos, COVERED_BLOCK);
        play_sound(world, pos, Sound::BlockWoolBreak);
    } else if block_is(&name, COVERED_BLOCK) {
        state::with_infinite_board_mut(&world_id, |board| {
            board.cells.insert((x, z), CellState::Flagged);
        });
        let flag_block = state::flag_block_for(uuid_key(player));
        set_block(world, pos, flag_block);
        play_sound(world, pos, Sound::BlockWoolPlace);
    }
    true
}

fn is_flag_wool(name: &str) -> bool {
    crate::state::FLAG_STYLE_ORDER
        .iter()
        .any(|(_, block)| block_is(name, block))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_matches_hash_helpers() {
        for x in -4..=4 {
            for z in -4..=4 {
                match content_at(x, z) {
                    CellContent::Bomb => assert!(infinite_is_bomb(x, z, BOARD_SEED)),
                    CellContent::Empty => {
                        assert!(!infinite_is_bomb(x, z, BOARD_SEED));
                        assert_eq!(infinite_number_at(x, z, BOARD_SEED), 0);
                    }
                    CellContent::Number(n) => {
                        assert!(!infinite_is_bomb(x, z, BOARD_SEED));
                        assert_eq!(infinite_number_at(x, z, BOARD_SEED), n);
                        assert!(n >= 1 && n <= 8, "2D neighbor max is 8, got {n}");
                    }
                }
            }
        }
    }

    #[test]
    fn flag_wool_recognizes_every_style() {
        for (_, block) in crate::state::FLAG_STYLE_ORDER {
            assert!(is_flag_wool(block), "{block}");
            // Reads may come back without the namespace.
            let short = block.rsplit(':').next().unwrap();
            assert!(is_flag_wool(short), "{short}");
        }
        assert!(!is_flag_wool("minecraft:moss_block"));
        assert!(!is_flag_wool("minecraft:stone"));
        assert!(!is_flag_wool("minecraft:tnt"));
    }
}

fn overlay(player: &Player, key: crate::state::UuidKey) {
    use pumpkin_plugin_api::text::TextComponent;
    state::with_session_mut(key, |s| {
        // Color tiers mirror the old server (gray -> green -> blue ->
        // dark-blue -> red -> dark-red -> gold), obfuscated past 1000.
        let code = match s.current_streak {
            0..=50 => "§7",
            51..=150 => "§a",
            151..=300 => "§9",
            301..=500 => "§1",
            501..=1000 => "§c",
            1001..=2000 => "§4",
            _ => "§6",
        };
        let body = if s.current_streak > 1000 {
            format!(
                "§ka {code}Streak: {}  Best: {}  Opened: {}§r §ka§r",
                s.current_streak, s.top_streak, s.total_opened
            )
        } else {
            format!(
                "{code}Streak: {}  Best: {}  Opened: {}",
                s.current_streak, s.top_streak, s.total_opened
            )
        };
        crate::helpers::action_bar_component(player, TextComponent::from_legacy_string(&body));
    });
}
