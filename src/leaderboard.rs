//! In-world leaderboards: one big multi-line text display entity per board
//! at hardcoded spots, refreshed from SQLite. Stateless by design: each
//! board is tagged with a custom name. On the first refresh after load
//! every text display within a radius of the board's spot is wiped
//! (leftovers, legacy boards, stacks — tags may not survive restarts) and a
//! fresh display is spawned; later refreshes only re-style/re-text the
//! tagged keeper. That heals server restarts without leaving ghosts,
//! mirroring how the old server rebuilt its armor-stand boards on a timer.

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
    /// `db::KIND_2D` / `db::KIND_3D` (the board's dimensionality).
    game_kind: i64,
    dim: usize,
    /// What the lines rank: fastest win times or most games finished.
    kind: BoardKind,
    /// Fixed text orientation (see `Facing`).
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
/// (+Z); these are 180°/90° rotations about Y applied on top.
#[derive(Clone, Copy, PartialEq)]
enum Facing {
    North,
    East,
}

impl Facing {
    /// Left-rotation quaternion for the display transformation.
    fn quaternion(self) -> (f32, f32, f32, f32) {
        match self {
            // 180° about Y.
            Facing::North => (0.0, 1.0, 0.0, 0.0),
            // +90° about Y takes +Z (south) to +X (east).
            Facing::East => (0.0, 0.7071, 0.0, 0.7071),
        }
    }
}

/// Hardcoded boards, all in one row at z=29 (16-block x spacing):
/// fastest-wins and most-games pairs for the two preset specs.
/// Line 0 is the header at the TOP; entries stack downward from it.
/// Edit the numbers to move a board.
const BOARDS: [Board; 4] = [
    Board {
        tag: "ms_lb_2d50",
        title: "2D 50x50 All-Time Best",
        game_kind: crate::db::KIND_2D,
        dim: 50,
        kind: BoardKind::FastestWins,
        facing: Facing::North,
        x: 4.0,
        y: 2.0,
        z: 29.0,
    },
    Board {
        tag: "ms_lb_3d7",
        title: "3D 7x7x7 All-Time Best",
        game_kind: crate::db::KIND_3D,
        dim: 7,
        kind: BoardKind::FastestWins,
        facing: Facing::North,
        x: -12.0,
        y: 2.0,
        z: 29.0,
    },
    Board {
        tag: "ms_lb_2d50_games",
        title: "2D 50x50 Most Games Finished",
        game_kind: crate::db::KIND_2D,
        dim: 50,
        kind: BoardKind::MostGames,
        facing: Facing::East,
        x: -26.0,
        y: 2.0,
        z: 9.0,
    },
    Board {
        tag: "ms_lb_3d7_games",
        title: "3D 7x7x7 Most Games Finished",
        game_kind: crate::db::KIND_3D,
        dim: 7,
        kind: BoardKind::MostGames,
        facing: Facing::East,
        x: -27.0,
        y: 2.0,
        z: -17.0,
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

/// Rendered text block for one board: gold header + top 10 lines,
/// newline-separated into a single display. Placeholder lines fill the
/// empty tail so the board keeps its size before anyone plays.
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

/// First-refresh trackers: on the very first refresh after load each board
/// ignores its existing displays and spawns completely fresh ones. This
/// self-heals any stale metadata (invisible-text corruption from older
/// sessions) every restart.
fn fresh_cycle() -> &'static Mutex<HashSet<&'static str>> {
    static FRESH: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    FRESH.get_or_init(|| Mutex::new(HashSet::new()))
}

fn style_new_display(display: &TextDisplayEntity, facing: Facing) {
    display.set_alignment(TextAlignment::Center);
    // Fixed orientation: the board never rotates to face the player.
    display.get_display().set_billboard(BillboardMode::Fixed);
    // Orientation flip on top of the display's default (south) plane.
    let (x, y, z, w) = facing.quaternion();
    display.get_display().set_transformation(
        TransformationBuilder::new().left_rotation(x, y, z, w).build(),
    );
    // Full-light override: the boards sit low (y=2) where the sampled light
    // is near zero, rendering the text dark. Vanilla packs block/sky light
    // as two 4-bit nibbles; 255 sets both to 15 regardless of nibble order.
    display.get_display().set_brightness(255);
}

/// Squared wipe radius around a board's spot: at boot, every text display
/// inside it is removed (leftovers/legacy/stacked boards). The two boards
/// are 16 blocks apart, so a 6-block radius never crosses between them.
const WIPE_DIST2: f64 = 36.0;

fn dist2(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)
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

/// Reconcile one board: exactly one tagged display must exist at our coords.
fn refresh_board(world: &World, board: &Board) {
    let pos = (board.x, board.y, board.z);
    let text = board_text(board);
    let forced_fresh = fresh_cycle()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(board.tag);
    if forced_fresh {
        // Boot: wipe every text display near the spot (leftovers, legacy
        // boards, stacks — tags may not survive restarts) and spawn fresh.
        wipe_area(world, pos, &[]);
        spawn_board_display(world, board, pos, &text);
        return;
    }
    // Live refresh: tag-only matching — spawns always carry the tag, so
    // keeper identification is by name alone. Then a stray wipe: leftovers
    // that surfaced with a late chunk load (entities persist inside chunk
    // saves, so they're invisible until their chunk streams in) get removed
    // here, while every tagged keeper — ours and the other boards' — is
    // protected by name.
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
                // Re-style every refresh so orientation changes converge.
                style_new_display(&display, board.facing);
                display.set_text(TextComponent::from_legacy_string(&text));
            } else {
                // Tagged entity is not a text display (foreign tag
                // collision): drop it and respawn.
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

/// Refresh all boards. Idempotent and fail-soft (missing world/DB just logs).
pub fn refresh(server: &Server) {
    let Some(world) = board_world(server) else {
        return;
    };
    for board in &BOARDS {
        refresh_board(&world, board);
    }
}
