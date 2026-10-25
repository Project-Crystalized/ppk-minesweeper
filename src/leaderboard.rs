//! In-world leaderboards: one multi-line text display per hardcoded spot,
//! refreshed from SQLite. Displays are tagged; the first refresh after load
//! wipes the area and respawns fresh, later refreshes reuse the tagged keeper.

use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

use pumpkin_plugin_api::{
    text::TextComponent, BillboardMode, EntityType, Server, TextAlignment, TextDisplayEntity,
    TransformationBuilder, World,
};

/// Preferred world; falls back to the first loaded world when the server
/// uses a different name for its default.
const BOARD_WORLD_NAME: &str = "world";
/// Header + top 10 lines in the single text block.
const LINES_PER_BOARD: usize = 11;
/// Refresh cadence in ticks (1200 = 60s); initial spawn happens on the
/// second tick after load (see `tick.rs`).
pub const REFRESH_TICKS: u64 = 1200;

struct Board {
    tag: &'static str,
    title: &'static str,
    game_kind: i64,
    dim: usize,
    kind: BoardKind,
    facing: Facing,
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum BoardKind {
    FastestWins,
    MostGames,
}

/// Fixed text orientation. The display's default fixed plane faces south
/// (+Z); these are rotations about Y applied on top.
#[derive(Clone, Copy, PartialEq)]
enum Facing {
    South,
    North,
}

impl Facing {
    /// Left-rotation quaternion for the display transformation.
    fn quaternion(self) -> (f32, f32, f32, f32) {
        match self {
            Facing::South => (0.0, 0.0, 0.0, 1.0),
            Facing::North => (0.0, 1.0, 0.0, 0.0),
        }
    }
}

/// Two rows of three spots in the spawn plaza: south wall at z=1024 (facing
/// south), north wall at z=1079 (facing north). Four boards fill four spots;
/// the x=1117 spots are reserved. Edit the numbers to move a board.
const BOARDS: [Board; 4] = [
    Board {
        tag: "ms_lb_2d50",
        title: "2D 50x50 All-Time Best",
        game_kind: crate::db::KIND_2D,
        dim: 50,
        kind: BoardKind::FastestWins,
        facing: Facing::South,
        x: 1091.0,
        y: 75.0,
        z: 1024.0,
    },
    Board {
        tag: "ms_lb_3d7",
        title: "3D 7x7x7 All-Time Best",
        game_kind: crate::db::KIND_3D,
        dim: 7,
        kind: BoardKind::FastestWins,
        facing: Facing::South,
        x: 1104.0,
        y: 75.0,
        z: 1024.0,
    },
    Board {
        tag: "ms_lb_2d50_games",
        title: "2D 50x50 Most Games Finished",
        game_kind: crate::db::KIND_2D,
        dim: 50,
        kind: BoardKind::MostGames,
        facing: Facing::North,
        x: 1091.0,
        y: 75.0,
        z: 1079.0,
    },
    Board {
        tag: "ms_lb_3d7_games",
        title: "3D 7x7x7 Most Games Finished",
        game_kind: crate::db::KIND_3D,
        dim: 7,
        kind: BoardKind::MostGames,
        facing: Facing::North,
        x: 1104.0,
        y: 75.0,
        z: 1079.0,
    },
];

fn warned() -> &'static Mutex<HashSet<String>> {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn warn_once(key: &str, msg: &str) {
    if warned()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.to_string())
    {
        tracing::warn!("{msg}");
    }
}

fn board_world(server: &Server) -> Option<World> {
    if let Some(world) = server.get_world_by_name(BOARD_WORLD_NAME) {
        return Some(world);
    }
    if let Some(world) = server.get_all_worlds().into_iter().next() {
        warn_once(
            "world-fallback",
            &format!(
                "leaderboard: no world named '{BOARD_WORLD_NAME}', using '{}' instead",
                world.get_id()
            ),
        );
        return Some(world);
    }
    warn_once("no-world", "leaderboard: no loaded world to place displays in");
    None
}

/// Rendered text block for one board: gold header plus top 10 lines, with
/// placeholders filling the tail so the display keeps its size.
fn board_text(board: &Board) -> String {
    let mut lines = vec![format!("§6§l{}", board.title)];
    match board.kind {
        BoardKind::FastestWins => {
            for (i, w) in crate::db::top_spec_wins(board.game_kind, board.dim, 10)
                .iter()
                .enumerate()
            {
                lines.push(format!("§f{}. {} - {}s", i + 1, w.owner_name, w.secs));
            }
        }
        BoardKind::MostGames => {
            for (i, w) in crate::db::top_spec_games(board.game_kind, board.dim, 10)
                .iter()
                .enumerate()
            {
                lines.push(format!("§f{}. {} - {} games", i + 1, w.owner_name, w.games));
            }
        }
    }
    while lines.len() < LINES_PER_BOARD {
        lines.push("§7—. ---".to_string());
    }
    lines.truncate(LINES_PER_BOARD);
    lines.join("\n")
}

/// First-refresh trackers: the first refresh after load spawns each board
/// completely fresh, healing stale metadata (invisible-text corruption).
fn fresh_cycle() -> &'static Mutex<HashSet<&'static str>> {
    static FRESH: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    FRESH.get_or_init(|| Mutex::new(HashSet::new()))
}

fn style_new_display(display: &TextDisplayEntity, facing: Facing) {
    display.set_alignment(TextAlignment::Center);
    display.get_display().set_billboard(BillboardMode::Fixed);
    let (x, y, z, w) = facing.quaternion();
    display.get_display().set_transformation(
        TransformationBuilder::new().left_rotation(x, y, z, w).build(),
    );
    // Full-light override: the sampled light can be near zero (covered spots),
    // so set both light nibbles to 15.
    display.get_display().set_brightness(255);
}

/// Squared wipe radius around a board's spot: at boot, every text display
/// inside it is removed (leftovers/legacy/stacked boards). Boards in a row
/// are 13 blocks apart, so a 6-block radius never crosses between them.
const WIPE_DIST2: f64 = 36.0;

fn dist2(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)
}

/// Remove tagged displays left behind at a board's old spot.
fn purge_offsite_tagged(world: &World, tag: &str, pos: (f64, f64, f64)) {
    for entity in world.get_entities() {
        if entity
            .get_custom_name()
            .map(|c| c.get_text().to_string())
            .as_deref()
            != Some(tag)
        {
            continue;
        }
        if dist2(entity.get_position(), pos) > 4.0 {
            entity.remove();
        }
    }
}

/// Remove every text display within [`WIPE_DIST2`] of `pos` whose custom
/// name is not in `keep`. `keep` empty = boot wipe (removes our own stale
/// keepers too); live refreshes pass all board tags so only strays die.
/// Entities in unloaded chunks are invisible here (they arrive with their
/// chunk, so this re-runs every refresh until the area stays clean).
fn wipe_area(world: &World, pos: (f64, f64, f64), keep: &[&'static str]) -> usize {
    let mut removed = 0;
    for entity in world.get_entities() {
        if TextDisplayEntity::from_entity(&entity).is_none()
            || dist2(entity.get_position(), pos) > WIPE_DIST2
        {
            continue;
        }
        let named = entity.get_custom_name().map(|c| c.get_text().to_string());
        if named
            .as_deref()
            .is_some_and(|n| keep.iter().any(|k| *k == n))
        {
            continue;
        }
        entity.remove();
        removed += 1;
    }
    removed
}

fn board_tags() -> Vec<&'static str> {
    BOARDS.iter().map(|b| b.tag).collect()
}

fn refresh_board(world: &World, board: &Board) {
    let pos = (board.x, board.y, board.z);
    let text = board_text(board);
    purge_offsite_tagged(world, board.tag, pos);
    let forced_fresh = fresh_cycle()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(board.tag);
    if forced_fresh {
        wipe_area(world, pos, &[]);
        spawn_board_display(world, board, pos, &text);
        return;
    }
    // Remove strays that surfaced with a late chunk load, then reuse the
    // tagged keeper.
    wipe_area(world, pos, &board_tags());
    let mut keeper = None;
    for entity in world.get_entities() {
        if entity.get_custom_name().map(|c| c.get_text()).as_deref() == Some(board.tag) {
            keeper = Some(entity);
            break;
        }
    }
    match keeper {
        Some(entity) => {
            if let Some(display) = TextDisplayEntity::from_entity(&entity) {
                style_new_display(&display, board.facing);
                display.set_text(TextComponent::from_legacy_string(&text));
            } else {
                // Foreign tag collision: drop it and respawn.
                entity.remove();
                spawn_board_display(world, board, pos, &text);
            }
        }
        None => spawn_board_display(world, board, pos, &text),
    }
}


fn spawn_board_display(world: &World, board: &Board, pos: (f64, f64, f64), text: &str) {
    let entity = world.spawn_entity(EntityType::TextDisplay, pos);
    entity.set_custom_name(TextComponent::text(board.tag));
    entity.set_custom_name_visible(false);
    if let Some(display) = TextDisplayEntity::from_entity(&entity) {
        style_new_display(&display, board.facing);
        display.set_text(TextComponent::from_legacy_string(text));
    } else {
        tracing::warn!("leaderboard: spawned entity did not cast to text display");
        entity.remove();
    }
}

pub fn refresh(server: &Server) {
    let Some(world) = board_world(server) else {
        return;
    };
    for board in &BOARDS {
        refresh_board(&world, board);
    }
}
