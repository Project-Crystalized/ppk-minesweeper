//! Shared in-memory game state.
//!
//! The plugin owns no world or ECS: all active games live in these globals,
//! keyed by an incrementing id. World/Player handles are never stored here;
//! they are re-acquired from event/server handles on every call.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

use crate::core::{Cell, Pos3};

pub type UuidKey = (u64, u64);

pub const MAX_GAMES_PER_PLAYER: usize = 3;
/// Vertical bounds for board placement. There is no horizontal limit: unlike
/// the old server (which had a 250-block world border), Pumpkin worlds are
/// effectively unbounded, so boards can start wherever the player stands.
pub const MIN_BUILD_Y: i32 = -64;
pub const MAX_BUILD_Y: i32 = 320;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameKind {
    TwoD,
    ThreeD,
}

pub struct GameInstance {
    pub kind: GameKind,
    pub dim: usize,
    pub board: Vec<Cell>,
    pub owner: UuidKey,
    pub world_id: String,
    pub comp_time_ticks: u64,
    pub is_over: bool,
    pub is_won: bool,
}

impl GameInstance {
    pub fn bomb_total(&self) -> usize {
        use crate::core::CellContent;
        self.board
            .iter()
            .filter(|c| c.content == CellContent::Bomb)
            .count()
    }

    pub fn flag_count(&self) -> usize {
        use crate::core::CellState;
        self.board
            .iter()
            .filter(|c| c.state == CellState::Flagged)
            .count()
    }

    /// Index of the cell at `pos`, if this game owns it.
    pub fn find_cell(&self, pos: Pos3) -> Option<usize> {
        self.board.iter().position(|c| c.pos == pos)
    }
}

struct State {
    games: HashMap<u32, GameInstance>,
    /// Block position -> game id, for O(1) click routing.
    pos_index: HashMap<(i32, i32, i32), u32>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            games: HashMap::new(),
            pos_index: HashMap::new(),
        })
    })
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// Seed source for board RNGs (mixed with the player uuid at creation).
static RNG_SALT: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);

/// Global tick counter, advanced by the 1-tick scheduler task. Used for
/// short-term cooldowns (flag debounce).
static TICK: AtomicU64 = AtomicU64::new(0);

pub fn now_tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

pub fn tick_advance() -> u64 {
    TICK.fetch_add(1, Ordering::Relaxed) + 1
}

/// Ticks a flag toggle stays locked (anti spam-hold). The old Valence game
/// used an equivalent `flag_lock`.
pub const FLAG_LOCK_TICKS: u64 = 6;

fn flag_locks() -> &'static Mutex<HashMap<(u32, usize), u64>> {
    static LOCKS: OnceLock<Mutex<HashMap<(u32, usize), u64>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn flag_locked(id: u32, idx: usize, now: u64) -> bool {
    flag_locks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(id, idx))
        .is_some_and(|&exp| now < exp)
}

pub fn lock_flag(id: u32, idx: usize, until: u64) {
    flag_locks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((id, idx), until);
}

pub fn prune_flag_locks(now: u64) {
    flag_locks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, exp| *exp > now);
    infinite_flag_locks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, exp| *exp > now);
}

/// Same anti-spam lock for infinite-mode flags (vanilla sends main-hand +
/// off-hand interact packets per physical click).
fn infinite_flag_locks() -> &'static Mutex<HashMap<(String, i32, i32), u64>> {
    static LOCKS: OnceLock<Mutex<HashMap<(String, i32, i32), u64>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn infinite_flag_locked(world_id: &str, x: i32, z: i32, now: u64) -> bool {
    infinite_flag_locks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(world_id.to_string(), x, z))
        .is_some_and(|&exp| now < exp)
}

pub fn lock_infinite_flag(world_id: &str, x: i32, z: i32, until: u64) {
    infinite_flag_locks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((world_id.to_string(), x, z), until);
}

pub fn next_rng_seed(owner: UuidKey) -> u64 {
    let salt = RNG_SALT.fetch_add(0x9E3779B97F4A7C15, Ordering::Relaxed);
    salt ^ owner.0 ^ owner.1
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

/// Insert a game, indexing all its cells. Returns the new game id.
pub fn insert_game(game: GameInstance) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut s = lock();
    for cell in &game.board {
        s.pos_index.insert((cell.pos.x, cell.pos.y, cell.pos.z), id);
    }
    s.games.insert(id, game);
    id
}

pub fn with_game<R>(id: u32, f: impl FnOnce(&GameInstance) -> R) -> Option<R> {
    lock().games.get(&id).map(f)
}

pub fn with_game_mut<R>(id: u32, f: impl FnOnce(&mut GameInstance) -> R) -> Option<R> {
    lock().games.get_mut(&id).map(f)
}

pub fn game_id_at(x: i32, y: i32, z: i32) -> Option<u32> {
    lock().pos_index.get(&(x, y, z)).copied()
}

pub fn player_game_count(owner: UuidKey) -> usize {
    lock().games.values().filter(|g| g.owner == owner).count()
}

pub fn player_games(owner: UuidKey) -> Vec<u32> {
    lock()
        .games
        .iter()
        .filter(|(_, g)| g.owner == owner)
        .map(|(id, _)| *id)
        .collect()
}

/// Remove a game, returning it so the caller can clear its blocks.
pub fn remove_game(id: u32) -> Option<GameInstance> {
    let mut s = lock();
    let game = s.games.remove(&id)?;
    for cell in &game.board {
        s.pos_index.remove(&(cell.pos.x, cell.pos.y, cell.pos.z));
    }
    Some(game)
}

/// Remove all games owned by `owner`, returning them for block cleanup.
pub fn remove_player_games(owner: UuidKey) -> Vec<GameInstance> {
    let ids: Vec<u32> = lock()
        .games
        .iter()
        .filter(|(_, g)| g.owner == owner)
        .map(|(id, _)| *id)
        .collect();
    ids.into_iter().filter_map(remove_game).collect()
}

pub fn all_games_snapshot() -> Vec<(u32, UuidKey, String, u64, usize, usize, bool)> {
    lock()
        .games
        .iter()
        .map(|(id, g)| {
            (
                *id,
                g.owner,
                g.world_id.clone(),
                g.comp_time_ticks,
                g.bomb_total(),
                g.flag_count(),
                g.is_over,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Infinite-mode sessions (in-memory only, per the no-persistence decision)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct InfiniteSession {
    pub current_streak: usize,
    pub top_streak: usize,
    pub total_opened: usize,
    pub total_bombs: usize,
}

fn sessions() -> &'static Mutex<HashMap<UuidKey, InfiniteSession>> {
    static SESSIONS: OnceLock<Mutex<HashMap<UuidKey, InfiniteSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn with_session_mut<R>(owner: UuidKey, f: impl FnOnce(&mut InfiniteSession) -> R) -> R {
    let mut s = sessions().lock().unwrap_or_else(|e| e.into_inner());
    f(s.entry(owner).or_default())
}

/// Shared infinite board state: cells that are no longer `Closed`.
/// Content itself is derived deterministically from coordinates + seed.
#[derive(Default)]
pub struct InfiniteBoard {
    pub cells: HashMap<(i32, i32), crate::core::CellState>,
}

fn boards() -> &'static Mutex<HashMap<String, InfiniteBoard>> {
    static BOARDS: OnceLock<Mutex<HashMap<String, InfiniteBoard>>> = OnceLock::new();
    BOARDS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn with_infinite_board_mut<R>(world_id: &str, f: impl FnOnce(&mut InfiniteBoard) -> R) -> R {
    let mut b = boards().lock().unwrap_or_else(|e| e.into_inner());
    f(b.entry(world_id.to_string()).or_default())
}
