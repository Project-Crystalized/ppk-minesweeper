//! In-world leaderboards: one big multi-line text display entity per board
//! at hardcoded spots, refreshed from SQLite. Stateless by design: each
//! board is tagged with a custom name, so refresh finds and reuses its
//! display (or spawns it). That also heals server restarts without leaving
//! ghosts, mirroring how the old server rebuilt its armor-stand boards on
//! a timer.

use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

use pumpkin_plugin_api::{
    text::TextComponent, BillboardMode, EntityType, Server, TextAlignment, TextDisplayEntity,
    World,
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
    mode: &'static str,
    dim: usize,
    x: f64,
    y: f64,
    z: f64,
}

/// Hardcoded boards: 2D 50x50 at (4, 2, 27), 3D 7x7 at (-12, 2, 27).
/// Line 0 is the header at the TOP; entries stack downward from it.
/// Edit the numbers to move a board.
const BOARDS: [Board; 2] = [
    Board {
        tag: "ms_lb_2d50",
        title: "2D 50x50 All-Time Best",
        mode: "2D",
        dim: 50,
        x: 4.0,
        y: 2.0,
        z: 27.0,
    },
    Board {
        tag: "ms_lb_3d7",
        title: "3D 7x7x7 All-Time Best",
        mode: "3D",
        dim: 7,
        x: -12.0,
        y: 2.0,
        z: 27.0,
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
        tracing::info!(
            "leaderboard: no world named '{BOARD_WORLD_NAME}', using '{}' instead",
            world.get_id()
        );
        return Some(world);
    }
    warn_once("no-world", "leaderboard: no loaded world to place displays in");
    None
}

/// Rendered text block for one board: gold header + fastest wins or
/// placeholders, newline-separated into a single display.
fn board_text(board: &Board) -> String {
    let mut lines = vec![format!("§6§l{}", board.title)];
    let top = crate::db::top_spec_wins(board.mode, board.dim, 10);
    for (i, w) in top.iter().enumerate() {
        lines.push(format!("§f{}. {} - {}s", i + 1, w.owner_name, w.secs));
    }
    while lines.len() < LINES_PER_BOARD {
        lines.push("§7—. ---".to_string());
    }
    lines.truncate(LINES_PER_BOARD);
    lines.join("\n")
}

fn style_new_display(display: &TextDisplayEntity) {
    display.set_alignment(TextAlignment::Center);
    // Fixed orientation: the board never rotates to face the player.
    display.get_display().set_billboard(BillboardMode::Fixed);
}

/// Squared adoption radius: untagged text displays within a block of our
/// hardcoded coords count as ours (their tags may not survive restarts).
const ADOPT_DIST2: f64 = 1.0;

fn dist2(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)
}

/// Reconcile one board: exactly one display must exist at our coords.
/// Matching is by tag first, then by position: custom names may not survive
/// a restart on all hosts, in which case the persisted set comes back
/// untagged and tag-only matching would spawn a fresh set next to it every
/// restart (the stacking bug). An untagged text display within a block of
/// our hardcoded coords is ours; anything else is left alone.
fn refresh_board(world: &World, board: &Board) {
    let legacy_prefix = format!("{}_", board.tag);
    let pos = (board.x, board.y, board.z);
    let text = board_text(board);

    // keeper index + whether it already carries our tag
    let mut keeper: Option<usize> = None;
    let mut keeper_tagged = false;
    // all candidates to collapse (tagged ours + untagged lookalikes)
    let mut dups: Vec<usize> = Vec::new();
    let entities = world.get_entities();
    let mut known: Vec<(Option<String>, pumpkin_plugin_api::Entity)> = Vec::new();
    for entity in entities {
        let name = entity.get_custom_name().map(|c| c.get_text());
        let tagged = name.as_deref() == Some(board.tag);
        let legacy = name
            .as_deref()
            .is_some_and(|n| n.starts_with(&legacy_prefix));
        let lookalike = name.is_none()
            && TextDisplayEntity::from_entity(&entity).is_some()
            && dist2(entity.get_position(), pos) <= ADOPT_DIST2;
        if tagged || legacy || lookalike {
            known.push((name, entity));
        }
    }
    // Prefer the tagged keeper; otherwise adopt the closest lookalike.
    let mut best_dist = f64::INFINITY;
    for (k, (name, entity)) in known.iter().enumerate() {
        if name.as_deref() == Some(board.tag) {
            if keeper.is_none() {
                keeper = Some(k);
                keeper_tagged = true;
            } else {
                dups.push(k);
            }
            continue;
        }
        if keeper_tagged {
            // A tagged keeper owns the spot: everything else goes.
            dups.push(k);
            continue;
        }
        if name.is_some() {
            // Legacy per-line display: always removed, never adopted.
            dups.push(k);
            continue;
        }
        // No tagged keeper: adopt the closest untagged lookalike.
        let (ex, ey, ez) = entity.get_position();
        let d = (ex - pos.0).powi(2) + (ey - pos.1).powi(2) + (ez - pos.2).powi(2);
        if d < best_dist {
            if let Some(prev) = keeper {
                dups.push(prev);
            }
            keeper = Some(k);
            best_dist = d;
        } else {
            dups.push(k);
        }
    }
    if let Some(k) = keeper {
        let entity = &known[k].1;
        if let Some(display) = TextDisplayEntity::from_entity(entity) {
            // Style every refresh (not just at spawn) so orientation
            // changes converge on already-placed displays.
            style_new_display(&display);
            display.set_text(TextComponent::from_legacy_string(&text));
            // Retag adopted lookalikes so later cycles match by tag, and
            // reposition every refresh so layout changes migrate live
            // displays instead of mixing old and new positions.
            if known[k].0.as_deref() != Some(board.tag) {
                entity.set_custom_name(TextComponent::text(board.tag));
                entity.set_custom_name_visible(false);
            }
            let w = entity.get_world();
            entity.teleport(pos, w);
        } else {
            // Tagged entity is not a text display (foreign tag collision):
            // drop it and respawn below.
            entity.remove();
            spawn_board_display(world, board, pos, &text);
        }
    } else {
        spawn_board_display(world, board, pos, &text);
    }
}

fn spawn_board_display(world: &World, board: &Board, pos: (f64, f64, f64), text: &str) {
    let entity = world.spawn_entity(EntityType::TextDisplay, pos);
    entity.set_custom_name(TextComponent::text(board.tag));
    entity.set_custom_name_visible(false);
    if let Some(display) = TextDisplayEntity::from_entity(&entity) {
        style_new_display(&display);
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
