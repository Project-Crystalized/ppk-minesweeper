//! In-world leaderboards: one multi-line text display per hardcoded spot,
//! refreshed from SQLite. The first refresh after load wipes and respawns;
//! later refreshes reuse one keeper and remove duplicates.

use std::{
    collections::HashSet,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use pumpkin_plugin_api::{
    BillboardMode, Entity, EntityType, Server, TextAlignment, TextDisplayEntity,
    TransformationBuilder, World, text::TextComponent,
};

/// Preferred world; falls back to the first loaded world when the server
/// uses a different name for its default.
const BOARD_WORLD_NAME: &str = "world";
/// Header + top 10 lines in the single text block.
const LINES_PER_BOARD: usize = 11;
/// Refresh cadence in ticks (1200 = 60s); initial spawn happens on the
/// second tick after load (see `tick.rs`).
pub const REFRESH_TICKS: u64 = 1200;

/// Leaderboard view toggles. Both default to the requested server-start
/// view: bigger boards and solo games only.
static SOLO_ONLY: AtomicBool = AtomicBool::new(true);
static BIGGER_SIZES: AtomicBool = AtomicBool::new(true);
static REFRESH_REQUESTED: AtomicBool = AtomicBool::new(false);
static LAST_REFRESH_TICK: AtomicU64 = AtomicU64::new(0);
/// Last accepted button press, or `u64::MAX` before the first one.
static LAST_BUTTON_TICK: AtomicU64 = AtomicU64::new(u64::MAX);

/// Button clicks are coalesced: a request waits until this many ticks have
/// passed since the last refresh, so holding/spamming a button cannot run the
/// six leaderboard queries every tick.
const MIN_REFRESH_INTERVAL_TICKS: u64 = 10;
/// Server-wide button lock. 20 ticks is one second at the normal tick rate.
/// This swallows duplicate main-hand/off-hand interactions and stops rapid
/// presses from flipping the view repeatedly.
const BUTTON_LOCK_TICKS: u64 = 20;

pub fn solo_only() -> bool {
    SOLO_ONLY.load(Ordering::Relaxed)
}

pub fn bigger_sizes() -> bool {
    BIGGER_SIZES.load(Ordering::Relaxed)
}

pub fn toggle_solo_only() -> bool {
    // `fetch_xor` returns the previous value; the new value is its inverse.
    !SOLO_ONLY.fetch_xor(true, Ordering::Relaxed)
}

pub fn toggle_bigger_sizes() -> bool {
    !BIGGER_SIZES.fetch_xor(true, Ordering::Relaxed)
}

/// Ask the tick task to refresh on its next pass. Used by button clicks so
/// the blocking interact event returns without doing six DB queries.
pub fn request_refresh() {
    // Release publishes the toggle stores above to the tick thread.
    REFRESH_REQUESTED.store(true, Ordering::Release);
}

pub fn take_refresh_request() -> bool {
    if !REFRESH_REQUESTED.load(Ordering::Acquire) {
        return false;
    }
    let now = crate::state::now_tick();
    if now.saturating_sub(LAST_REFRESH_TICK.load(Ordering::Relaxed)) < MIN_REFRESH_INTERVAL_TICKS {
        return false;
    }
    REFRESH_REQUESTED.swap(false, Ordering::AcqRel)
}

fn button_lock_ready(last: u64, now: u64) -> bool {
    last == u64::MAX || now.saturating_sub(last) >= BUTTON_LOCK_TICKS
}

/// Global one-second lock for leaderboard buttons. Returns false for the
/// second half of a double interaction, rapid spam, or a second player clicking
/// while the lock is still active.
pub fn button_click_allowed() -> bool {
    let now = crate::state::now_tick();
    LAST_BUTTON_TICK
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            button_lock_ready(last, now).then_some(now)
        })
        .is_ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Wooden,
    Stone,
}

/// Classify a block name as a wooden or stone button. Names may arrive with
/// or without the `minecraft:` namespace.
pub fn button_kind(block_name: &str) -> Option<ButtonKind> {
    let name = block_name.rsplit(':').next().unwrap_or(block_name);
    if name == "stone_button" || name == "polished_blackstone_button" {
        Some(ButtonKind::Stone)
    } else if name.ends_with("_button") {
        Some(ButtonKind::Wooden)
    } else {
        None
    }
}

struct Board {
    tag: &'static str,
    kind: BoardKind,
    facing: Facing,
    x: f64,
    y: f64,
    z: f64,
}

/// Snapshot of the two view toggles, taken once per refresh so every board in
/// the same pass agrees on mode and size.
#[derive(Clone, Copy)]
struct BoardView {
    solo: bool,
    bigger: bool,
}

impl BoardView {
    fn current() -> Self {
        Self {
            solo: solo_only(),
            bigger: bigger_sizes(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BoardKind {
    InfiniteStreak,
    InfiniteNet,
    FastestWins {
        game_kind: i64,
        small_dim: usize,
        big_dim: usize,
    },
    MostWins {
        game_kind: i64,
        small_dim: usize,
        big_dim: usize,
    },
}

impl BoardKind {
    fn spec(self, bigger: bool) -> Option<(i64, usize)> {
        match self {
            BoardKind::FastestWins {
                game_kind,
                small_dim,
                big_dim,
            }
            | BoardKind::MostWins {
                game_kind,
                small_dim,
                big_dim,
            } => Some((game_kind, if bigger { big_dim } else { small_dim })),
            BoardKind::InfiniteStreak | BoardKind::InfiniteNet => None,
        }
    }
}

fn mode_label(solo: bool) -> &'static str {
    if solo { "Solo" } else { "Team" }
}

fn size_label(kind: i64, dim: usize) -> String {
    if kind == crate::db::KIND_3D {
        format!("3D {dim}x{dim}x{dim}")
    } else {
        format!("2D {dim}x{dim}")
    }
}

impl Board {
    fn title(&self, view: BoardView) -> String {
        match self.kind {
            BoardKind::InfiniteStreak => "Infinite - Highest Streak".to_string(),
            BoardKind::InfiniteNet => "Infinite - Net Cells Opened".to_string(),
            BoardKind::FastestWins { .. } => {
                let (kind, dim) = self.kind.spec(view.bigger).expect("spec board");
                format!(
                    "{} {} - Best Time",
                    size_label(kind, dim),
                    mode_label(view.solo)
                )
            }
            BoardKind::MostWins { .. } => {
                let (kind, dim) = self.kind.spec(view.bigger).expect("spec board");
                format!(
                    "{} {} - Most Wins",
                    size_label(kind, dim),
                    mode_label(view.solo)
                )
            }
        }
    }
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
/// south), north wall at z=1079 (facing north). Each column is one mode:
/// infinite, 2D, then 3D. The stone button swaps the 2D/3D sizes; the wooden
/// button swaps solo/team for the 2D/3D boards. Infinite is all-player and has
/// no team or size dimension, so the buttons do not affect it.
/// Edit the numbers to move a board.
const BOARDS: [Board; 6] = [
    Board {
        tag: "ms_lb_inf_streak",
        kind: BoardKind::InfiniteStreak,
        facing: Facing::South,
        x: 1091.0,
        y: 75.0,
        z: 1024.0,
    },
    Board {
        tag: "ms_lb_2d_best",
        kind: BoardKind::FastestWins {
            game_kind: crate::db::KIND_2D,
            small_dim: 20,
            big_dim: 40,
        },
        facing: Facing::South,
        x: 1104.0,
        y: 75.0,
        z: 1024.0,
    },
    Board {
        tag: "ms_lb_3d_best",
        kind: BoardKind::FastestWins {
            game_kind: crate::db::KIND_3D,
            small_dim: 5,
            big_dim: 7,
        },
        facing: Facing::South,
        x: 1117.0,
        y: 75.0,
        z: 1024.0,
    },
    Board {
        tag: "ms_lb_inf_cells",
        kind: BoardKind::InfiniteNet,
        facing: Facing::North,
        x: 1091.0,
        y: 75.0,
        z: 1079.0,
    },
    Board {
        tag: "ms_lb_2d_games",
        kind: BoardKind::MostWins {
            game_kind: crate::db::KIND_2D,
            small_dim: 20,
            big_dim: 40,
        },
        facing: Facing::North,
        x: 1104.0,
        y: 75.0,
        z: 1079.0,
    },
    Board {
        tag: "ms_lb_3d_games",
        kind: BoardKind::MostWins {
            game_kind: crate::db::KIND_3D,
            small_dim: 5,
            big_dim: 7,
        },
        facing: Facing::North,
        x: 1117.0,
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
    warn_once(
        "no-world",
        "leaderboard: no loaded world to place displays in",
    );
    None
}

/// Rendered text block for one board: gold header plus top 10 lines, with
/// placeholders filling the tail so the display keeps its size.
fn board_text(board: &Board, view: BoardView) -> String {
    let mut lines = vec![format!("§6§l{}", board.title(view))];
    match board.kind {
        BoardKind::InfiniteStreak => {
            for (i, t) in crate::db::top_infinite(10).iter().enumerate() {
                lines.push(format!("§f{}. {} - {}", i + 1, t.name, t.top));
            }
        }
        BoardKind::InfiniteNet => {
            for (i, t) in crate::db::top_infinite_net(10).iter().enumerate() {
                lines.push(format!("§f{}. {} - {}", i + 1, t.name, t.net()));
            }
        }
        BoardKind::FastestWins { game_kind, .. } => {
            let dim = board.kind.spec(view.bigger).expect("spec board").1;
            for (i, w) in crate::db::top_spec_wins(game_kind, dim, view.solo, 10)
                .iter()
                .enumerate()
            {
                lines.push(format!("§f{}. {} - {}s", i + 1, w.name, w.secs));
            }
        }
        BoardKind::MostWins { game_kind, .. } => {
            let dim = board.kind.spec(view.bigger).expect("spec board").1;
            for (i, w) in crate::db::top_spec_win_counts(game_kind, dim, view.solo, 10)
                .iter()
                .enumerate()
            {
                let label = if w.wins == 1 { "win" } else { "wins" };
                lines.push(format!("§f{}. {} - {} {}", i + 1, w.name, w.wins, label));
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
        TransformationBuilder::new()
            .left_rotation(x, y, z, w)
            .build(),
    );
    // Full-light override: the sampled light can be near zero (covered spots),
    // so set both light nibbles to 15.
    display.get_display().set_brightness(255);
}

/// Squared wipe radius around a board's spot: at boot, every text display
/// inside it is removed (leftovers/legacy/stacked boards). Boards in a row
/// are 13 blocks apart, so a 6-block radius never crosses between them.
const WIPE_DIST2: f64 = 36.0;

/// Squared keeper-search radius (2 blocks).
const KEEPER_DIST2: f64 = 4.0;

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
        if dist2(entity.get_position(), pos) > KEEPER_DIST2 {
            entity.remove();
        }
    }
}

/// Find this board's closest tagged text display, breaking ties by entity ID.
fn find_keeper(world: &World, tag: &str, pos: (f64, f64, f64)) -> Option<Entity> {
    world
        .get_entities()
        .into_iter()
        .filter(|entity| TextDisplayEntity::from_entity(entity).is_some())
        .filter(|entity| {
            entity
                .get_custom_name()
                .map(|c| c.get_text().to_string())
                .as_deref()
                == Some(tag)
        })
        .filter(|entity| dist2(entity.get_position(), pos) <= KEEPER_DIST2)
        .min_by(|a, b| {
            dist2(a.get_position(), pos)
                .total_cmp(&dist2(b.get_position(), pos))
                .then_with(|| a.get_id().cmp(&b.get_id()))
        })
}

/// Remove text displays within [`WIPE_DIST2`] of `pos`, except `keep_id`.
/// `None` removes all of them. Unloaded chunks are handled on a later refresh.
fn wipe_area(world: &World, pos: (f64, f64, f64), keep_id: Option<u32>) -> usize {
    let mut removed = 0;
    for entity in world.get_entities() {
        if TextDisplayEntity::from_entity(&entity).is_none()
            || dist2(entity.get_position(), pos) > WIPE_DIST2
        {
            continue;
        }
        if Some(entity.get_id()) == keep_id {
            continue;
        }
        entity.remove();
        removed += 1;
    }
    removed
}

fn refresh_board(world: &World, board: &Board, view: BoardView) {
    let pos = (board.x, board.y, board.z);
    let text = board_text(board, view);
    purge_offsite_tagged(world, board.tag, pos);
    let forced_fresh = fresh_cycle()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(board.tag);
    if forced_fresh {
        wipe_area(world, pos, None);
        spawn_board_display(world, board, pos, &text);
        return;
    }
    let keeper = find_keeper(world, board.tag, pos);
    wipe_area(world, pos, keeper.as_ref().map(|entity| entity.get_id()));
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
    LAST_REFRESH_TICK.store(crate::state::now_tick(), Ordering::Relaxed);
    let view = BoardView::current();
    for board in &BOARDS {
        refresh_board(&world, board, view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boards_cover_the_six_requested_spots() {
        assert_eq!(BOARDS.len(), 6);
        assert_eq!(
            BOARDS
                .iter()
                .filter(|b| b.kind == BoardKind::InfiniteStreak)
                .count(),
            1
        );
        assert_eq!(
            BOARDS
                .iter()
                .filter(|b| b.kind == BoardKind::InfiniteNet)
                .count(),
            1
        );
        assert_eq!(
            BOARDS
                .iter()
                .filter(|b| matches!(b.kind, BoardKind::FastestWins { .. }))
                .count(),
            2
        );
        assert_eq!(
            BOARDS
                .iter()
                .filter(|b| matches!(b.kind, BoardKind::MostWins { .. }))
                .count(),
            2
        );
        for kind in [crate::db::KIND_2D, crate::db::KIND_3D] {
            assert_eq!(
                BOARDS
                    .iter()
                    .filter(|b| matches!(
                        b.kind,
                        BoardKind::FastestWins { game_kind, .. } if game_kind == kind
                    ))
                    .count(),
                1
            );
            assert_eq!(
                BOARDS
                    .iter()
                    .filter(|b| matches!(
                        b.kind,
                        BoardKind::MostWins { game_kind, .. } if game_kind == kind
                    ))
                    .count(),
                1
            );
        }
        let mut spots: Vec<_> = BOARDS
            .iter()
            .map(|b| (b.x.to_bits(), b.z.to_bits()))
            .collect();
        spots.sort_unstable();
        spots.dedup();
        assert_eq!(spots.len(), 6);
    }

    #[test]
    fn size_toggle_switches_both_specs() {
        for board in BOARDS {
            match board.kind {
                BoardKind::FastestWins {
                    game_kind,
                    small_dim,
                    big_dim,
                }
                | BoardKind::MostWins {
                    game_kind,
                    small_dim,
                    big_dim,
                } => {
                    assert_eq!(board.kind.spec(false), Some((game_kind, small_dim)));
                    assert_eq!(board.kind.spec(true), Some((game_kind, big_dim)));
                }
                BoardKind::InfiniteStreak | BoardKind::InfiniteNet => {
                    assert_eq!(board.kind.spec(false), None);
                    assert_eq!(board.kind.spec(true), None);
                }
            }
        }
    }

    #[test]
    fn infinite_titles_are_explicit() {
        let streak = BOARDS
            .iter()
            .find(|b| b.kind == BoardKind::InfiniteStreak)
            .unwrap();
        let view = BoardView {
            solo: true,
            bigger: true,
        };
        assert_eq!(streak.title(view), "Infinite - Highest Streak");
        let net = BOARDS
            .iter()
            .find(|b| b.kind == BoardKind::InfiniteNet)
            .unwrap();
        assert_eq!(net.title(view), "Infinite - Net Cells Opened");
    }

    #[test]
    fn titles_use_the_refresh_snapshot() {
        let board = BOARDS
            .iter()
            .find(|b| {
                matches!(
                    b.kind,
                    BoardKind::FastestWins { game_kind, .. } if game_kind == crate::db::KIND_2D
                )
            })
            .unwrap();
        assert_eq!(
            board.title(BoardView {
                solo: false,
                bigger: false,
            }),
            "2D 20x20 Team - Best Time"
        );
        assert_eq!(
            board.title(BoardView {
                solo: true,
                bigger: true,
            }),
            "2D 40x40 Solo - Best Time"
        );
    }

    #[test]
    fn button_lock_is_one_second() {
        assert!(button_lock_ready(u64::MAX, 0));
        assert!(button_lock_ready(0, BUTTON_LOCK_TICKS));
        assert!(!button_lock_ready(0, BUTTON_LOCK_TICKS - 1));
    }

    #[test]
    fn button_click_locks_globally() {
        assert!(button_click_allowed());
        assert!(!button_click_allowed());
    }

    #[test]
    fn button_names_are_classified() {
        for name in [
            "oak_button",
            "minecraft:spruce_button",
            "bamboo_button",
            "crimson_button",
            "pale_oak_button",
        ] {
            assert_eq!(button_kind(name), Some(ButtonKind::Wooden), "{name}");
        }
        for name in ["stone_button", "minecraft:polished_blackstone_button"] {
            assert_eq!(button_kind(name), Some(ButtonKind::Stone), "{name}");
        }
        for name in ["oak_planks", "stone", "moss_block"] {
            assert_eq!(button_kind(name), None, "{name}");
        }
    }
}
