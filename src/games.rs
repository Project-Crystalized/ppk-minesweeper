//! Boxed game modes (2D + 3D): creation, clicking, flagging, clearing.

use pumpkin_plugin_api::{Player, World};

use crate::{
    core::{
        self, adjacent_2d, adjacent_3d, bomb_amount_2d, bomb_amount_3d, generate_board_2d,
        generate_board_3d, is_won, revealed_block_name, CellContent, CellState, Pos3, TinyRng,
        AIR_BLOCK, COVERED_BLOCK, FLAG_BLOCK,
    },
    helpers::{self, is_air, play_sound, set_block, tell, Sound},
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
        "2d30, 2d50, 3d5, 3d7, or /ms 2d <size> / 3d <size> (2+)"
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

/// Find the first fully-free origin for the board: above head height so the
/// player can walk underneath, and offset sideways so no cell spawns inside
/// the player standing at `feet`.
fn find_origin(world: &World, feet: Pos3, spec: NewGameSpec) -> Option<Pos3> {
    let offsets = cell_offsets(spec);
    for dy in 2..=13 {
        // +2/+2 in x/z keeps the player's own footprint clear of cells.
        let origin = Pos3::new(feet.x + 2, feet.y + dy, feet.z + 2);
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
                "No free space in the 12 blocks above your feet ({}, {}, {}). Go somewhere with open sky.",
                feet.x, feet.y, feet.z
            ),
        );
        return;
    };

    let mut rng = TinyRng::new(state::next_rng_seed(owner));
    let (kind, dim, board) = cells_of(spec, origin, &mut rng);

    for cell in &board {
        set_block(&world, cell.pos, COVERED_BLOCK);
    }

    let id = state::insert_game(GameInstance {
        kind,
        dim,
        board,
        owner,
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

pub fn clear_player_games(player: &Player) {
    let owner = helpers::uuid_key(player);
    let world = player.get_world();
    let mut n = 0;
    for game in state::remove_player_games(owner) {
        clear_blocks(&world, &game);
        n += 1;
    }
    // Also clear games whose world differs (best effort via server lookup is
    // handled by the caller passing the right player; here we just report).
    tell(player, &format!("Cleared {n} game(s)."));
}

fn clear_blocks(world: &World, game: &GameInstance) {
    for cell in &game.board {
        set_block(world, cell.pos, AIR_BLOCK);
    }
}

pub fn clear_game_in_world(world: &World, id: u32) {
    if let Some(game) = state::remove_game(id) {
        clear_blocks(world, &game);
    }
}

fn reveal_all(world: &World, game: &GameInstance) {
    for cell in &game.board {
        set_block(world, cell.pos, revealed_block_name(cell.content));
    }
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
        set_block(world, cell.pos, revealed_block_name(cell.content));
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
        set_block(world, cell.pos, revealed_block_name(cell.content));
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

/// Left-click (block break) on a board cell. Returns true if the click
/// belonged to a game (caller should cancel the break event).
pub fn click_left(world: &World, player: &Player, id: u32, idx: usize) -> bool {
    let owner = helpers::uuid_key(player);
    // Owner clicking after game over despawns the game.
    let over_owned = state::with_game(id, |g| g.is_over && g.owner == owner).unwrap_or(false);
    if over_owned {
        let t0 = state::now_tick();
        clear_game_in_world(world, id);
        tracing::info!(
            "clear: game #{id} took {} ticks",
            state::now_tick().saturating_sub(t0)
        );
        tell(player, "Game cleared. Start a new one with /ms.");
        return true;
    }
    state::with_game_mut(id, |game| {
        if game.is_over {
            return false;
        }
        // First-click protection (mirrors the old regenerate_if_not_empty):
        // never let the opening click be a bomb or a number.
        if core::all_closed(&game.board) && game.board[idx].content != CellContent::Empty {
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
        match game.board[idx].content {
            CellContent::Bomb => {
                let pos = game.board[idx].pos;
                play_sound(world, pos, Sound::EntityGenericExplode);
                reveal_all(world, game);
                game.is_over = true;
                game.is_won = false;
                true
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
                true
            }
            CellContent::Number(_) => {
                let pos = game.board[idx].pos;
                game.board[idx].state = CellState::Opened;
                set_block(world, pos, revealed_block_name(game.board[idx].content));
                play_sound(world, pos, Sound::EntityFrogStep);
                if is_won(&game.board) {
                    play_sound(world, pos, Sound::ItemGoatHornSound1);
                    game.is_over = true;
                    game.is_won = true;
                }
                true
            }
        }
    })
    .unwrap_or(false)
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
/// Returns true when the click was consumed (toggled or spam-swallowed).
pub fn click_right(world: &World, id: u32, idx: usize) -> bool {
    state::with_game_mut(id, |game| {
        if game.is_over {
            return false;
        }
        let now = state::now_tick();
        if state::flag_locked(id, idx, now) {
            return true;
        }
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
            CellState::Opened => return false,
        };
        state::lock_flag(id, idx, now + FLAG_LOCK_TICKS);
        set_block(world, pos, if flagged { FLAG_BLOCK } else { COVERED_BLOCK });
        play_sound(
            world,
            pos,
            if flagged {
                Sound::BlockWoolPlace
            } else {
                Sound::BlockWoolBreak
            },
        );
        true
    })
    .unwrap_or(false)
}

pub fn status_line(id: u32) -> Option<String> {
    state::with_game(id, |g| {
        format!(
            "Time: {}s  Bombs: {}  Flags: {}  {}",
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
            }
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
    let snap = state::all_games_snapshot();
    let mut lines = Vec::new();
    for (id, owner, _world, ticks, bombs, flags, over) in snap {
        if let Some(f) = owner_filter {
            if f != owner {
                continue;
            }
        }
        lines.push(format!(
            "#{id} {}s bombs={bombs} flags={flags} {}",
            ticks / 20,
            if over { "over" } else { "active" }
        ));
    }
    if lines.is_empty() {
        "No games.".to_string()
    } else {
        lines.join(" | ")
    }
}
