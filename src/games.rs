//! Boxed game modes (2D + 3D): creation, clicking, flagging, clearing.

use pumpkin_plugin_api::{Player, World};

use crate::{
    core::{
        self, adjacent_2d, adjacent_3d, bomb_amount_2d, bomb_amount_3d, generate_board_2d,
        generate_board_3d, is_won, CellContent, CellState, Pos3, TinyRng,
        AIR_BLOCK, COVERED_BLOCK,
    },
    helpers::{self, is_air, play_sound, set_block, set_tile, tell, Sound},
    state::{
        self, GameInstance, GameKind, MAX_BUILD_Y, MAX_GAMES_PER_PLAYER, MIN_BUILD_Y,
        FLAG_LOCK_TICKS,
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NewGameSpec {
    pub kind: GameKind,
    pub dim: usize,
}

impl NewGameSpec {
    pub const TWO_D_20: Self = Self {
        kind: GameKind::TwoD,
        dim: 20,
    };
    pub const TWO_D_30: Self = Self {
        kind: GameKind::TwoD,
        dim: 30,
    };
    pub const TWO_D_50: Self = Self {
        kind: GameKind::TwoD,
        dim: 50,
    };
    pub const THREE_D_5: Self = Self {
        kind: GameKind::ThreeD,
        dim: 5,
    };
    pub const THREE_D_7: Self = Self {
        kind: GameKind::ThreeD,
        dim: 7,
    };
    pub const THREE_D_10: Self = Self {
        kind: GameKind::ThreeD,
        dim: 10,
    };

    /// Custom sizes from `/ms 2d|3d <size>`: any dimension >= 2.
    /// No upper cap by design; huge boards just take proportionally longer
    /// to build/clear (one world write per cell).
    pub fn custom(kind: GameKind, dim: usize) -> Option<Self> {
        (dim >= 2).then_some(Self { kind, dim })
    }

    pub fn label(&self) -> String {
        match self.kind {
            GameKind::TwoD => format!("2D {}x{}", self.dim, self.dim),
            GameKind::ThreeD => format!("3D {}x{}x{}", self.dim, self.dim, self.dim),
        }
    }

    pub fn all_labels() -> &'static str {
        "/ms 2d <size> / 3d <size> (2+)"
    }
}

/// Offsets (relative to the origin cell) of every board cell for `spec`.
fn cell_offsets(spec: NewGameSpec) -> Vec<(i32, i32, i32)> {
    let dim = spec.dim;
    match spec.kind {
        GameKind::TwoD => (0..dim)
            .flat_map(|x| (0..dim).map(move |z| (x as i32, 0, z as i32)))
            .collect(),
        GameKind::ThreeD => (0..dim)
            .flat_map(|x| {
                (0..dim).flat_map(move |y| {
                    (0..dim).map(move |z| (x as i32 * 3, z as i32 * 3, y as i32 * 3))
                })
            })
            .collect(),
    }
}

fn within_limits(p: Pos3) -> bool {
    p.y >= MIN_BUILD_Y && p.y <= MAX_BUILD_Y
}

/// Descending lane-stop candidates below the feet: -1, -2 ... `down_to`
/// (\(\ge\) empty when the world floor is nearer than 10 blocks). Kept as a
/// pure function so the descending-range subtlety has a regression test.
fn below_candidates(down_to: i32) -> Vec<i32> {
    if down_to > -1 {
        Vec::new()
    } else {
        (down_to..=-1).rev().collect()
    }
}

/// Find the first fully-free origin for the board: strictly below the
/// player's feet (creative flight UX — hover up, the board floats under
/// you), scanning down at most 10 blocks. No upward fallback: standing on
/// land with no open air below refuses to spawn. Offset sideways so no
/// cell ever spawns inside the player at `feet`.
fn find_origin(world: &World, feet: Pos3, spec: NewGameSpec) -> Option<Pos3> {
    let offsets = cell_offsets(spec);
    // Deepest downward candidate, clamped at the build-limit floor so a
    // player standing near world bottom can't scan below it.
    let floor_y = (feet.y - 10).max(MIN_BUILD_Y);
    let down_to = floor_y - feet.y;
    // Iterate -1, -2, ... down_to. Note the range must be built ascending
    // (down_to..=-1) before reversing: an inverted (-1..=down_to) is
    // already empty and would silently skip the whole scan.
    for dy in below_candidates(down_to) {
        // -2/-2 in x/z anchors the board under your feet: the closest cell
        // column sits 2 blocks behind your position, the rest extends into
        // +x/+z. In flight the board's near corner is directly below you.
        let origin = Pos3::new(feet.x - 2, feet.y + dy, feet.z - 2);
        if !within_limits(origin) {
            continue;
        }
        let mut free = true;
        for (ox, oy, oz) in &offsets {
            let p = origin.offset(*ox, *oy, *oz);
            if !within_limits(p) || !is_air(world, p) {
                free = false;
                break;
            }
        }
        if free {
            return Some(origin);
        }
    }
    None
}

fn cells_of(spec: NewGameSpec, origin: Pos3, rng: &mut TinyRng) -> (GameKind, usize, Vec<core::Cell>) {
    let dim = spec.dim;
    match spec.kind {
        GameKind::TwoD => (
            GameKind::TwoD,
            dim,
            generate_board_2d(dim, bomb_amount_2d(dim), origin, rng),
        ),
        GameKind::ThreeD => (
            GameKind::ThreeD,
            dim,
            generate_board_3d(dim, bomb_amount_3d(dim), origin, rng),
        ),
    }
}



pub fn create_game(player: &Player, spec: NewGameSpec) {
    let owner = helpers::uuid_key(player);
    if state::player_game_count(owner) >= MAX_GAMES_PER_PLAYER {
        tell(player, "You can't have more than 3 games active (/ms clear).");
        return;
    }
    let world = player.get_world();
    let world_id = world.get_id();
    let (px, py, pz) = player.get_position();
    let feet = Pos3::new(px.floor() as i32, py.floor() as i32, pz.floor() as i32);

    let Some(origin) = find_origin(&world, feet, spec) else {
        tell(
            player,
            &format!(
                "No free space within 10 blocks below your feet ({}, {}, {}). Fly up a bit; boards only spawn in open air below you.",
                feet.x, feet.y, feet.z
            ),
        );
        return;
    };

    let mut rng = TinyRng::new(state::next_rng_seed(owner));
    let (kind, dim, board) = cells_of(spec, origin, &mut rng);

    let build_start = std::time::Instant::now();
    let mut bulk = helpers::BulkWriter::new(&world);
    for cell in &board {
        bulk.set(cell.pos, COVERED_BLOCK);
    }
    tracing::info!(
        "build: {} {} cells in {:.1}ms",
        spec.label(),
        board.len(),
        build_start.elapsed().as_secs_f64() * 1000.0
    );

    let id = state::insert_game(GameInstance {
        kind,
        dim,
        board,
        owner,
        helpers: Vec::new(),
        locked: false,
        recorded: false,
        world_id,
        comp_time_ticks: 0,
        is_over: false,
        is_won: false,
    });
    let o = origin;
    tracing::info!(
        "{} game #{} for {} at {}, {}, {}",
        spec.label(),
        id,
        player.get_name(),
        o.x,
        o.y,
        o.z
    );
    tell(
        player,
        &format!(
            "{} game #{} started at {}, {}, {}. Left-click to open, right-click to flag.",
            spec.label(),
            id,
            o.x,
            o.y,
            o.z
        ),
    );
}

pub fn clear_player_games(player: &Player, server: &pumpkin_plugin_api::Server) {
    let owner = helpers::uuid_key(player);
    let mut n = 0;
    for game in state::remove_player_games(owner) {
        // Each game may live in a different world: resolve it by id so no
        // blocks leak and we never wipe the player's current world by mistake.
        match server.get_world_by_name(&game.world_id) {
            Some(world) => clear_blocks(&world, &game),
            None => clear_blocks(&player.get_world(), &game),
        }
        n += 1;
    }
    tell(player, &format!("Cleared {n} game(s)."));
}

/// Write AIR over a game's cells. Bulk (chunk-level) + timing-logged; shared
/// by punch-clear, `/ms clear` and the leave-event cleanup.
pub fn clear_blocks(world: &World, game: &GameInstance) {
    let start = std::time::Instant::now();
    let cells = game.board.len();
    let mut bulk = helpers::BulkWriter::new(world);
    for cell in &game.board {
        bulk.set(cell.pos, AIR_BLOCK);
    }
    tracing::info!(
        "clear-writes: {} cells in {:.1}ms",
        cells,
        start.elapsed().as_secs_f64() * 1000.0
    );
}

pub fn clear_game_in_world(world: &World, id: u32) {
    if let Some(game) = state::remove_game(id) {
        clear_blocks(world, &game);
    }
}

fn reveal_all(world: &World, game: &GameInstance) {
    // Timing diagnostic: the loss reveal is our largest single write burst
    // (dim² / dim³ tiles in one click) and had no instrumentation before.
    let start = std::time::Instant::now();
    let cells = game.board.len();
    let mut bulk = helpers::BulkWriter::new(world);
    for cell in &game.board {
        // Digit tiles need their `facing` block state; resolve the variant
        // ids up front so the bulk loop stays inside the cheap chunk API.
        let (name, facing) = crate::core::revealed_tile(cell.content);
        let id = match facing {
            Some(face) => helpers::state_id_for(name, Some(face)),
            None => helpers::state_id_for(name, None),
        };
        match id {
            Some(id) => bulk.set_state(cell.pos, id),
            None => {
                let _ = set_tile(world, cell.pos, cell.content);
            }
        }
    }
    tracing::info!(
        "reveal: {} cells in {:.1}ms",
        cells,
        start.elapsed().as_secs_f64() * 1000.0
    );
}

fn open_cell_2d(world: &World, game: &mut GameInstance, start: (usize, usize)) {
    // Iterative flood fill over empty cells (mirrors old DiggingEvent chains).
    let dim = game.dim;
    let mut stack = vec![start];
    let mut visited = vec![false; dim * dim];
    while let Some((x, y)) = stack.pop() {
        let idx = y * dim + x;
        if visited[idx] {
            continue;
        }
        visited[idx] = true;
        let cell = &mut game.board[idx];
        if cell.state != CellState::Closed {
            continue;
        }
        cell.state = CellState::Opened;
        set_tile(world, cell.pos, cell.content);
        if cell.content == CellContent::Empty {
            for (ax, ay) in adjacent_2d(dim, x, y) {
                let aidx = ay * dim + ax;
                if !visited[aidx] && game.board[aidx].state == CellState::Closed {
                    stack.push((ax, ay));
                }
            }
        }
    }
}

fn open_cell_3d(world: &World, game: &mut GameInstance, start: (usize, usize, usize)) {
    let dim = game.dim;
    let at = |x: usize, y: usize, z: usize| (z * dim + y) * dim + x;
    let mut stack = vec![start];
    let mut visited = vec![false; dim * dim * dim];
    while let Some((x, y, z)) = stack.pop() {
        let idx = at(x, y, z);
        if visited[idx] {
            continue;
        }
        visited[idx] = true;
        let cell = &mut game.board[idx];
        if cell.state != CellState::Closed {
            continue;
        }
        cell.state = CellState::Opened;
        set_tile(world, cell.pos, cell.content);
        if cell.content == CellContent::Empty {
            for (ax, ay, az) in adjacent_3d(dim, x, y, z) {
                let aidx = at(ax, ay, az);
                if !visited[aidx] && game.board[aidx].state == CellState::Closed {
                    stack.push((ax, ay, az));
                }
            }
        }
    }
}

/// Pure click-guard decision shared by `click_left`/`click_right`, kept
/// World-free so unit tests can cover the exact bug classes (flagged cells
/// exploding, locked games leaking clicks) without a live server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickDecision {
    Proceed,
    Ignore,
    DenyLocked,
}

/// Left-click guard: only `Closed` cells of unlocked (or owned) live games
/// react. Out-of-bounds indices are ignored defensively.
pub fn decide_left(
    game: &GameInstance,
    who: crate::state::UuidKey,
    idx: usize,
) -> ClickDecision {
    if game.is_over {
        return ClickDecision::Ignore;
    }
    if !game.is_allowed(who) {
        return ClickDecision::DenyLocked;
    }
    match game.board.get(idx) {
        Some(cell) if cell.state == CellState::Closed => ClickDecision::Proceed,
        _ => ClickDecision::Ignore,
    }
}

/// Right-click guard: `Closed` (flag) and `Flagged` (unflag) cells react,
/// `Opened` cells are swallowed.
pub fn decide_right(
    game: &GameInstance,
    who: crate::state::UuidKey,
    idx: usize,
) -> ClickDecision {
    if game.is_over {
        return ClickDecision::Ignore;
    }
    if !game.is_allowed(who) {
        return ClickDecision::DenyLocked;
    }
    match game.board.get(idx) {
        Some(cell) if cell.state != CellState::Opened => ClickDecision::Proceed,
        _ => ClickDecision::Ignore,
    }
}

/// Pure first-click condition: the board is untouched and the clicked cell is
/// not empty, so the board must be regenerated before opening.
pub fn needs_first_click_reshuffle(board: &[core::Cell], idx: usize) -> bool {
    core::all_closed(board)
        && board
            .get(idx)
            .is_some_and(|c| c.content != CellContent::Empty)
}
/// Left-click (block break) on a board cell. Always returns true: the click
/// belonged to a game, so the caller must cancel the break event even when
/// the click itself was a no-op (flagged cell, already open, locked game).
pub fn click_left(
    world: &World,
    server: &pumpkin_plugin_api::Server,
    player: &Player,
    id: u32,
    idx: usize,
) -> bool {
    let owner = helpers::uuid_key(player);
    // Owner clicking after game over despawns the game.
    let over_owned = state::with_game(id, |g| g.is_over && g.owner == owner).unwrap_or(false);
    if over_owned {
        let start = std::time::Instant::now();
        clear_game_in_world(world, id);
        tracing::info!(
            "clear: game #{id} removal handled in {:.1}ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        tell(player, "Game cleared. Start a new one with /ms.");
        return true;
    }
    // Lock / state guards run inside one mutation so the checks see a
    // consistent board. Every early exit still returns true (swallow click).
    let outcome = state::with_game_mut(id, |game| {
        match decide_left(game, owner, idx) {
            ClickDecision::Ignore => return Outcome::Swallowed,
            ClickDecision::DenyLocked => return Outcome::Locked,
            ClickDecision::Proceed => {}
        }
        // First-click protection (mirrors the old regenerate_if_not_empty):
        // never let the opening click be a bomb or a number.
        if needs_first_click_reshuffle(&game.board, idx) {
            let origin = game.board.first().map(|c| c.pos).unwrap_or_default();
            let mut seed = state::next_rng_seed(game.owner);
            for _ in 0..64 {
                seed = seed.wrapping_add(0x9E3779B97F4A7C15);
                let mut rng = TinyRng::new(seed);
                let board = match game.kind {
                    GameKind::TwoD => {
                        generate_board_2d(game.dim, bomb_amount_2d(game.dim), origin, &mut rng)
                    }
                    GameKind::ThreeD => {
                        generate_board_3d(game.dim, bomb_amount_3d(game.dim), origin, &mut rng)
                    }
                };
                if board[idx].content == CellContent::Empty {
                    game.board = board;
                    break;
                }
            }
        }
        if owner != game.owner && !game.helpers.contains(&owner) {
            game.helpers.push(owner);
        }
        match game.board[idx].content {
            CellContent::Bomb => {
                let pos = game.board[idx].pos;
                play_sound(world, pos, Sound::EntityGenericExplode);
                reveal_all(world, game);
                game.is_over = true;
                game.is_won = false;
                Outcome::Played
            }
            CellContent::Empty => {
                let cell_pos = game.board[idx].pos;
                play_sound(world, cell_pos, Sound::EntityFrogStep);
                let (x, y, z) = locate(game, idx);
                match game.kind {
                    GameKind::TwoD => open_cell_2d(world, game, (x, y)),
                    GameKind::ThreeD => open_cell_3d(world, game, (x, y, z)),
                }
                if is_won(&game.board) {
                    play_sound(world, cell_pos, Sound::ItemGoatHornSound1);
                    game.is_over = true;
                    game.is_won = true;
                }
                Outcome::Played
            }
            CellContent::Number(_) => {
                let pos = game.board[idx].pos;
                game.board[idx].state = CellState::Opened;
                set_tile(world, pos, game.board[idx].content);
                play_sound(world, pos, Sound::EntityFrogStep);
                if is_won(&game.board) {
                    play_sound(world, pos, Sound::ItemGoatHornSound1);
                    game.is_over = true;
                    game.is_won = true;
                }
                Outcome::Played
            }
        }
    });
    match outcome {
        Some(Outcome::Locked) => {
            tell(player, "That game is locked by its owner.");
            true
        }
        // Swallowed / played / missing game: always consume so the underlying
        // moss block is never actually broken.
        _ => {
            maybe_record_finished_game(server, player, id);
            true
        }
    }
}

/// Persist a game that just ended (win or loss), exactly once. Retries on
/// later clicks until the write succeeds, so a transient DB failure loses
/// nothing (the `recorded` flag only flips on success).
fn maybe_record_finished_game(server: &pumpkin_plugin_api::Server, clicker: &Player, id: u32) {
    let snap = state::with_game(id, |g| {
        (
            g.kind,
            g.dim,
            g.comp_time_ticks,
            g.is_over,
            g.is_won,
            g.owner,
            g.helpers.clone(),
            g.recorded,
        )
    });
    let Some((kind, dim, ticks, over, won, owner, helpers, recorded)) = snap else {
        return;
    };
    if !over || recorded {
        return;
    }
    let online: Vec<(crate::state::UuidKey, String)> = server
        .get_all_players()
        .iter()
        .map(|p| (helpers::uuid_key(p), p.get_name()))
        .collect();
    let name_of = |u: crate::state::UuidKey| {
        if u == helpers::uuid_key(clicker) {
            return clicker.get_name();
        }
        online
            .iter()
            .find(|(k, _)| *k == u)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "?".to_string())
    };
    let mode = match kind {
        GameKind::TwoD => "2D",
        GameKind::ThreeD => "3D",
    };
    if crate::db::record_box_game(dim, mode, ticks, won, owner, &name_of(owner), &helpers) {
        state::with_game_mut(id, |g| g.recorded = true);
    }
}

enum Outcome {
    Played,
    Swallowed,
    Locked,
}

fn locate(game: &GameInstance, idx: usize) -> (usize, usize, usize) {
    match game.kind {
        GameKind::TwoD => {
            let x = idx % game.dim;
            let y = idx / game.dim;
            (x, y, 0)
        }
        GameKind::ThreeD => {
            let x = idx % game.dim;
            let y = (idx / game.dim) % game.dim;
            let z = idx / (game.dim * game.dim);
            (x, y, z)
        }
    }
}

/// Right-click (interact) flag toggle, applied synchronously.
/// Always returns true when the position belongs to a game: toggled,
/// spam-swallowed, locked, or already-open clicks are all consumed so the
/// vanilla interact never places/uses a block. Uses the clicking player's
/// own flag color (slot-44 cycling, red wool default).
pub fn click_right(world: &World, player: &Player, id: u32, idx: usize) -> bool {
    let who = helpers::uuid_key(player);
    let outcome = state::with_game_mut(id, |game| {
        match decide_right(game, who, idx) {
            ClickDecision::Ignore => return Outcome::Swallowed,
            ClickDecision::DenyLocked => return Outcome::Locked,
            ClickDecision::Proceed => {}
        }
        let now = state::now_tick();
        if state::flag_locked(id, idx, now) {
            return Outcome::Swallowed;
        }
        // `decide_right` already proved `idx` is in bounds and not opened.
        let cell = &mut game.board[idx];
        let (flagged, pos) = match cell.state {
            CellState::Closed => {
                cell.state = CellState::Flagged;
                (true, cell.pos)
            }
            CellState::Flagged => {
                cell.state = CellState::Closed;
                (false, cell.pos)
            }
            CellState::Opened => return Outcome::Swallowed,
        };
        state::lock_flag(id, idx, now + FLAG_LOCK_TICKS);
        let flag_block = state::flag_block_for(who);
        set_block(world, pos, if flagged { flag_block } else { COVERED_BLOCK });
        play_sound(
            world,
            pos,
            if flagged {
                Sound::BlockWoolPlace
            } else {
                Sound::BlockWoolBreak
            },
        );
        Outcome::Played
    });
    match outcome {
        Some(Outcome::Locked) => {
            tell(player, "That game is locked by its owner.");
            true
        }
        _ => true,
    }
}

/// Plain-text status used by the unit tests (`#[cfg(test)]`).
#[allow(dead_code)]
pub fn status_line(id: u32) -> Option<String> {
    state::with_game(id, |g| {
        format!(
            "Time: {}s  Bombs: {}  Flags: {}  {}{}",
            g.comp_time_ticks / 20,
            g.bomb_total(),
            g.flag_count(),
            if g.is_over {
                if g.is_won {
                    "WON! Click your board to clear."
                } else {
                    "BOOM! Click your board to clear."
                }
            } else {
                ""
            },
            if g.locked { "  [Locked]" } else { "" },
        )
    })
}

/// Route a world position to (game id, cell idx), if it is a board cell.
pub fn route(pos: Pos3) -> Option<(u32, usize)> {
    let id = state::game_id_at(pos.x, pos.y, pos.z)?;
    let idx = state::with_game(id, |g| g.find_cell(pos))??;
    Some((id, idx))
}

pub fn describe_all_for(owner_filter: Option<crate::state::UuidKey>) -> String {
    let mut lines = Vec::new();
    for g in state::all_games_detailed() {
        if let Some(f) = owner_filter {
            if f != g.owner {
                continue;
            }
        }
        let kind = match g.kind {
            crate::state::GameKind::TwoD => format!("2D {}x{}", g.dim, g.dim),
            crate::state::GameKind::ThreeD => format!("3D {}x{}x{}", g.dim, g.dim, g.dim),
        };
        lines.push(format!(
            "#{id} {kind} {time}s bombs={bombs} flags={flags} helpers={helpers} {state}{locked}",
            id = g.id,
            time = g.ticks / 20,
            bombs = g.bombs,
            flags = g.flags,
            helpers = g.helpers,
            state = if g.over {
                if g.won { "won" } else { "lost" }
            } else {
                "active"
            },
            locked = if g.locked { " locked" } else { "" },
        ));
    }
    if lines.is_empty() {
        "No games.".to_string()
    } else {
        lines.join(" | ")
    }
}

/// Toggle locks. With `id`: that game only (owner only). Without: all owned.
/// Returns a human-readable report line.
pub fn toggle_lock_report(who: crate::state::UuidKey, id: Option<u32>) -> String {
    match id {
        Some(id) => match state::toggle_lock(id, who) {
            Some(locked) => format!(
                "Game #{id} {}.",
                if locked { "locked" } else { "unlocked" }
            ),
            None => format!("No owned game #{id} to lock."),
        },
        None => {
            let (n, locked) = state::toggle_all_locks(who);
            if n == 0 {
                "You have no games to lock.".to_string()
            } else {
                format!(
                    "{n} game(s) {}.",
                    if locked { "locked" } else { "unlocked" }
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn below_candidates_descend_order() {
        assert_eq!(super::below_candidates(-10), vec![-1, -2, -3, -4, -5, -6, -7, -8, -9, -10]);
        assert_eq!(super::below_candidates(-1), vec![-1]);
        // Floor nearer than 10 blocks: only the layers above it remain;
        // down_to >= 0 means no room below at all.
        assert!(super::below_candidates(0).is_empty());
    }

    use super::*;
    use crate::core::Cell;

    fn test_game(owner: crate::state::UuidKey, x0: i32, locked: bool) -> GameInstance {
        // 2x2 board with distinct contents: bomb / empty / number / flagged.
        let mk = |content: CellContent, state: CellState, dx: i32| Cell {
            content,
            state,
            pos: Pos3::new(x0 + dx, 200, 7001),
        };
        GameInstance {
            kind: GameKind::TwoD,
            dim: 2,
            board: vec![
                mk(CellContent::Bomb, CellState::Closed, 0),
                mk(CellContent::Empty, CellState::Closed, 1),
                mk(CellContent::Number(1), CellState::Flagged, 2),
                mk(CellContent::Number(2), CellState::Opened, 3),
            ],
            owner,
            helpers: Vec::new(),
            locked,
            recorded: false,
            world_id: "test_world".to_string(),
            comp_time_ticks: 0,
            is_over: false,
            is_won: false,
        }
    }

    #[test]
    fn left_guard_blocks_flagged_and_opened_cells() {
        // Regression test for the flagged-bomb explosion bug: only Closed
        // cells may proceed to the content match.
        let owner = (0xBEEF_0001, 0xCAFE_0001);
        let game = test_game(owner, 60_001, false);
        assert_eq!(decide_left(&game, owner, 0), ClickDecision::Proceed);
        assert_eq!(decide_left(&game, owner, 1), ClickDecision::Proceed);
        assert_eq!(decide_left(&game, owner, 2), ClickDecision::Ignore);
        assert_eq!(decide_left(&game, owner, 3), ClickDecision::Ignore);
        assert_eq!(decide_left(&game, owner, 99), ClickDecision::Ignore);
    }

    #[test]
    fn guards_enforce_locks_and_game_over() {
        let owner = (0xBEEF_0002, 0xCAFE_0002);
        let stranger = (0xDEAD_0002, 0xBEEF_0002);
        let open_game = test_game(owner, 60_011, false);
        assert_eq!(decide_left(&open_game, stranger, 0), ClickDecision::Proceed);
        assert_eq!(decide_right(&open_game, stranger, 0), ClickDecision::Proceed);
        // Right-click allows unflagging but never acts on opened cells.
        assert_eq!(decide_right(&open_game, stranger, 2), ClickDecision::Proceed);
        assert_eq!(decide_right(&open_game, stranger, 3), ClickDecision::Ignore);

        let mut locked = test_game(owner, 60_021, true);
        assert_eq!(decide_left(&locked, stranger, 0), ClickDecision::DenyLocked);
        assert_eq!(decide_right(&locked, stranger, 0), ClickDecision::DenyLocked);
        assert_eq!(decide_left(&locked, owner, 0), ClickDecision::Proceed);

        locked.is_over = true;
        assert_eq!(decide_left(&locked, owner, 0), ClickDecision::Ignore);
        assert_eq!(decide_right(&locked, owner, 0), ClickDecision::Ignore);
    }

    #[test]
    fn reshuffle_only_on_untouched_nonempty_click() {
        // Fresh all-closed board: bomb/number want a reshuffle, empty doesn't.
        let mk = |content: CellContent| Cell {
            content,
            state: CellState::Closed,
            pos: Pos3::default(),
        };
        let board = vec![
            mk(CellContent::Bomb),
            mk(CellContent::Empty),
            mk(CellContent::Number(3)),
        ];
        assert!(core::all_closed(&board));
        assert!(needs_first_click_reshuffle(&board, 0)); // bomb
        assert!(needs_first_click_reshuffle(&board, 2)); // number
        assert!(!needs_first_click_reshuffle(&board, 1)); // empty
        assert!(!needs_first_click_reshuffle(&board, 99)); // OOB
        // A single opened cell anywhere ends first-click protection.
        let mut touched = board;
        touched[1].state = CellState::Opened;
        assert!(!needs_first_click_reshuffle(&touched, 0));
    }

    #[test]
    fn locate_roundtrips_generated_boards() {
        let origin = Pos3::new(200, 64, 300);
        let mut rng = TinyRng::new(0x5EED);
        let board2 = generate_board_2d(4, bomb_amount_2d(4), origin, &mut rng);
        let game2 = GameInstance {
            kind: GameKind::TwoD,
            dim: 4,
            board: board2,
            owner: (1, 1),
            helpers: Vec::new(),
            locked: false,
            recorded: false,
            world_id: "w".to_string(),
            comp_time_ticks: 0,
            is_over: false,
            is_won: false,
        };
        for idx in 0..16 {
            let (x, y, z) = locate(&game2, idx);
            assert_eq!(z, 0);
            assert_eq!(
                game2.board[idx].pos,
                origin.offset(x as i32, 0, y as i32),
                "2D mismatch at {idx}"
            );
            assert_eq!(game2.find_cell(game2.board[idx].pos), Some(idx));
        }

        let mut rng = TinyRng::new(0x5EED);
        let board3 = generate_board_3d(3, bomb_amount_3d(3), origin, &mut rng);
        let game3 = GameInstance {
            kind: GameKind::ThreeD,
            dim: 3,
            board: board3,
            owner: (1, 1),
            helpers: Vec::new(),
            locked: false,
            recorded: false,
            world_id: "w".to_string(),
            comp_time_ticks: 0,
            is_over: false,
            is_won: false,
        };
        for idx in 0..27 {
            let (x, y, z) = locate(&game3, idx);
            assert_eq!(
                game3.board[idx].pos,
                origin.offset(x as i32 * 3, z as i32 * 3, y as i32 * 3),
                "3D mismatch at {idx}"
            );
            assert_eq!(game3.find_cell(game3.board[idx].pos), Some(idx));
        }
    }

    #[test]
    fn route_status_and_reports_agree_on_locks() {
        let owner = (0xBEEF_0004, 0xCAFE_0004);
        let id = state::insert_game(test_game(owner, 60_051, true));
        let pos = Pos3::new(60_051, 200, 7001);
        let (rid, idx) = route(pos).expect("inserted game must route");
        assert_eq!((rid, idx), (id, 0));
        assert!(status_line(id).unwrap().contains("[Locked]"));
        let listing = describe_all_for(Some(owner));
        assert!(listing.contains("locked"), "{listing}");
        assert!(describe_all_for(Some((7, 7))).contains("No games."));
        // Stranger gets the no-owned-game message, owner toggles through.
        assert_eq!(
            toggle_lock_report((8, 8), Some(id)),
            format!("No owned game #{id} to lock.")
        );
        assert_eq!(toggle_lock_report(owner, Some(id)), format!("Game #{id} unlocked."));
        assert!(!status_line(id).unwrap().contains("[Locked]"));
        state::remove_game(id);
        assert_eq!(route(pos), None);
    }
}
