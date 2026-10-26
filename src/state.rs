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
/// Vertical bounds for board placement (worlds are horizontally unbounded).
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
    /// Cached owner display name: boards outlive disconnects, so the
    /// scoreboard and DB records cannot rely on the owner being online.
    pub owner_name: String,
    /// Helpers who opened cells (for credit); unlike the owner they do NOT
    /// bypass `locked`.
    pub helpers: Vec<UuidKey>,
    pub locked: bool,
    /// Set once the finished game has been written to SQLite.
    pub recorded: bool,
    pub world_id: String,
    pub comp_time_ticks: u64,
    /// Seed that produced the board; 0 until the first dig.
    pub seed: u64,
    /// Left-clicks routed to this board (including swallowed ones).
    pub clicks: u32,
    /// Times a flag was toggled on (unflags do not count).
    pub flags_placed: u32,
    pub is_over: bool,
    pub is_won: bool,
}

impl GameInstance {
    pub fn bomb_total(&self) -> usize {
        match self.kind {
            GameKind::TwoD => crate::core::bomb_amount_2d(self.dim),
            GameKind::ThreeD => crate::core::bomb_amount_3d(self.dim),
        }
    }

    pub fn flag_count(&self) -> usize {
        use crate::core::CellState;
        self.board
            .iter()
            .filter(|c| c.state == CellState::Flagged)
            .count()
    }

    /// Cells opened at the end of the game (loss reveal only rewrites blocks,
    /// it does not change cell states, so this stays accurate).
    pub fn opened_count(&self) -> usize {
        use crate::core::CellState;
        self.board
            .iter()
            .filter(|c| c.state == CellState::Opened)
            .count()
    }

    pub fn origin(&self) -> Pos3 {
        self.board.first().map(|c| c.pos).unwrap_or_default()
    }

    pub fn find_cell(&self, pos: Pos3) -> Option<usize> {
        self.board.iter().position(|c| c.pos == pos)
    }

    /// Middle cell, used as the action-bar proximity anchor.
    pub fn center(&self) -> Pos3 {
        self.board
            .get(self.board.len() / 2)
            .map(|c| c.pos)
            .unwrap_or_default()
    }

    /// Owner always; helpers only when the game is unlocked.
    pub fn is_allowed(&self, who: UuidKey) -> bool {
        who == self.owner || !self.locked
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

/// Global tick counter, advanced by the 1-tick scheduler task. Used for
/// short-term cooldowns (flag debounce).
static TICK: AtomicU64 = AtomicU64::new(0);

pub fn now_tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

pub fn tick_advance() -> u64 {
    TICK.fetch_add(1, Ordering::Relaxed) + 1
}

fn join_ticks() -> &'static Mutex<HashMap<UuidKey, u64>> {
    static JOINS: OnceLock<Mutex<HashMap<UuidKey, u64>>> = OnceLock::new();
    JOINS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember when a player joined so their playtime can be added on leave.
pub fn mark_join(owner: UuidKey) {
    join_ticks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(owner, now_tick());
}

/// Ticks spent online since `mark_join`, consumed on leave.
pub fn take_playtime_ticks(owner: UuidKey) -> u64 {
    let start = join_ticks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&owner);
    start
        .map(|start| now_tick().saturating_sub(start))
        .unwrap_or(0)
}

/// Whether a player is currently online (used to pause their game timers
/// while they are disconnected; boards themselves stay alive).
pub fn is_online(owner: UuidKey) -> bool {
    join_ticks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&owner)
}

/// Ticks a flag toggle stays locked (anti spam-hold).
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

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

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

/// Refresh the cached display name on every board owned by `owner`
/// (called on join, so renames show up on scoreboards and later records).
pub fn update_owner_name(owner: UuidKey, name: &str) {
    for game in lock().games.values_mut() {
        if game.owner == owner {
            game.owner_name.clear();
            game.owner_name.push_str(name);
        }
    }
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

pub fn remove_player_games(owner: UuidKey) -> Vec<GameInstance> {
    let ids: Vec<u32> = lock()
        .games
        .iter()
        .filter(|(_, g)| g.owner == owner)
        .map(|(id, _)| *id)
        .collect();
    ids.into_iter().filter_map(remove_game).collect()
}

/// Advance active game timers by one server tick. A disconnected owner's
/// timer is paused: the board stays, but time spent offline does not count.
pub fn advance_game_timers() {
    let mut s = lock();
    for g in s.games.values_mut() {
        if !g.is_over && is_online(g.owner) {
            g.comp_time_ticks = g.comp_time_ticks.saturating_add(1);
        }
    }
}

/// Snapshot for `/ms list` and the tick action bar.
pub struct GameSummary {
    pub id: u32,
    pub owner: UuidKey,
    pub owner_name: String,
    pub world_id: String,
    pub kind: GameKind,
    pub dim: usize,
    pub ticks: u64,
    pub bombs: usize,
    pub flags: usize,
    pub over: bool,
    pub won: bool,
    pub locked: bool,
    pub helpers: usize,
    pub center: Pos3,
}

pub fn all_games_detailed() -> Vec<GameSummary> {
    lock()
        .games
        .iter()
        .map(|(id, g)| GameSummary {
            id: *id,
            owner: g.owner,
            owner_name: g.owner_name.clone(),
            world_id: g.world_id.clone(),
            kind: g.kind,
            dim: g.dim,
            ticks: g.comp_time_ticks,
            bombs: g.bomb_total(),
            flags: g.flag_count(),
            over: g.is_over,
            won: g.is_won,
            locked: g.locked,
            helpers: g.helpers.len(),
            center: g.center(),
        })
        .collect()
}

#[allow(dead_code)]
pub fn add_helper(id: u32, who: UuidKey) {
    if let Some(game) = lock().games.get_mut(&id) {
        if who != game.owner && !game.helpers.contains(&who) {
            game.helpers.push(who);
        }
    }
}

pub fn toggle_lock(id: u32, who: UuidKey) -> Option<bool> {
    let mut s = lock();
    let game = s.games.get_mut(&id)?;
    if game.owner != who {
        return None;
    }
    game.locked = !game.locked;
    Some(game.locked)
}

/// Toggle all games owned by `who`. Returns `(toggled, now_locked)`.
/// `now_locked` is the state after toggling when at least one game exists;
/// all games end up in the same state (lock if any unlocked, else unlock).
pub fn toggle_all_locks(who: UuidKey) -> (usize, bool) {
    let mut s = lock();
    let mut owned: Vec<&mut GameInstance> = s.games.values_mut().filter(|g| g.owner == who).collect();
    if owned.is_empty() {
        return (0, false);
    }
    let target = owned.iter().any(|g| !g.locked);
    for g in &mut owned {
        g.locked = target;
    }
    (owned.len(), target)
}

/// (wool item/block suffix, full block name) in cycle order.
pub const FLAG_STYLE_ORDER: [(&str, &str); 12] = [
    ("red", "minecraft:red_wool"),
    ("orange", "minecraft:orange_wool"),
    ("yellow", "minecraft:yellow_wool"),
    ("cyan", "minecraft:cyan_wool"),
    ("blue", "minecraft:blue_wool"),
    ("lime", "minecraft:lime_wool"),
    ("light_blue", "minecraft:light_blue_wool"),
    ("purple", "minecraft:purple_wool"),
    ("magenta", "minecraft:magenta_wool"),
    ("pink", "minecraft:pink_wool"),
    ("white", "minecraft:white_wool"),
    ("black", "minecraft:black_wool"),
];

fn flag_styles() -> &'static Mutex<HashMap<UuidKey, usize>> {
    static STYLES: OnceLock<Mutex<HashMap<UuidKey, usize>>> = OnceLock::new();
    STYLES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Full block name of `owner`'s current flag style (defaults to red wool).
pub fn flag_block_for(owner: UuidKey) -> &'static str {
    let idx = flag_styles()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&owner)
        .copied()
        .unwrap_or(0);
    FLAG_STYLE_ORDER[idx % FLAG_STYLE_ORDER.len()].1
}

pub fn cycle_flag_style(owner: UuidKey) -> &'static str {
    let mut s = flag_styles().lock().unwrap_or_else(|e| e.into_inner());
    let next = s.get(&owner).copied().unwrap_or(0) + 1;
    s.insert(owner, next % FLAG_STYLE_ORDER.len());
    FLAG_STYLE_ORDER[next % FLAG_STYLE_ORDER.len()].1
}

/// Reset to the default red wool.
pub fn reset_flag_style(owner: UuidKey) -> &'static str {
    flag_styles()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&owner);
    FLAG_STYLE_ORDER[0].1
}

/// Short color name ("red", "orange", ...) for item names and messages.
pub fn flag_style_name(owner: UuidKey) -> &'static str {
    let idx = flag_styles()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&owner)
        .copied()
        .unwrap_or(0);
    FLAG_STYLE_ORDER[idx % FLAG_STYLE_ORDER.len()].0
}

#[derive(Clone, Copy, Default)]
pub struct InfiniteSession {
    pub current_streak: usize,
    pub top_streak: usize,
    pub total_opened: usize,
    /// Bomb clicks (they reset the streak; they are not deaths).
    pub bombs_clicked: usize,
    /// Lifetime time spent in the infinite world, in ticks.
    pub playtime_ticks: u64,
}

fn sessions() -> &'static Mutex<HashMap<UuidKey, InfiniteSession>> {
    static SESSIONS: OnceLock<Mutex<HashMap<UuidKey, InfiniteSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn with_session_mut<R>(owner: UuidKey, f: impl FnOnce(&mut InfiniteSession) -> R) -> R {
    let mut s = sessions().lock().unwrap_or_else(|e| e.into_inner());
    f(s.entry(owner).or_default())
}

pub fn session(owner: UuidKey) -> Option<InfiniteSession> {
    sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&owner)
        .copied()
}/// Shared infinite board state: cells that are no longer `Closed`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CellContent, CellState, Cell};

    fn mk_game(owner: UuidKey, x0: i32, locked: bool) -> GameInstance {
        let cells = vec![
            Cell {
                content: CellContent::Empty,
                state: CellState::Closed,
                pos: Pos3::new(x0, 100, 9001),
            },
            Cell {
                content: CellContent::Empty,
                state: CellState::Closed,
                pos: Pos3::new(x0 + 1, 100, 9001),
            },
        ];
        GameInstance {
            kind: GameKind::TwoD,
            dim: 2,
            board: cells,
            owner,
            owner_name: "owner".to_string(),
            helpers: Vec::new(),
            locked,
            recorded: false,
            world_id: "test_world".to_string(),
            comp_time_ticks: 0,
            seed: 1,
            clicks: 0,
            flags_placed: 0,
            is_over: false,
            is_won: false,
        }
    }

    #[test]
    fn lifecycle_routes_cells_and_cleans_up() {
        let owner = (0xAAA1_0001, 0xBBB1_0001);
        let id = insert_game(mk_game(owner, 50_001, false));
        assert_eq!(game_id_at(50_001, 100, 9001), Some(id));
        assert_eq!(game_id_at(50_002, 100, 9001), Some(id));
        assert_eq!(game_id_at(50_003, 100, 9001), None);
        // Removal unindexes every cell: no ghost clicks afterwards.
        remove_game(id);
        assert_eq!(game_id_at(50_001, 100, 9001), None);
    }

    #[test]
    fn owner_name_refreshes_on_rename() {
        let owner = (0xAAA1_0006, 0xBBB1_0006);
        let id = insert_game(mk_game(owner, 50_101, false));
        assert_eq!(
            with_game(id, |g| g.owner_name.clone()).unwrap(),
            "owner"
        );
        update_owner_name(owner, "renamed");
        assert_eq!(
            with_game(id, |g| g.owner_name.clone()).unwrap(),
            "renamed"
        );
        remove_game(id);
    }

    #[test]
    fn online_tracking_marks_and_clears() {
        let owner = (0xAAA1_0007, 0xBBB1_0007);
        assert!(!is_online(owner));
        mark_join(owner);
        assert!(is_online(owner));
        let _ = take_playtime_ticks(owner);
        assert!(!is_online(owner));
    }

    #[test]
    fn locks_are_owner_only() {
        let owner = (0xAAA1_0002, 0xBBB1_0002);
        let stranger = (0xCC01_0002, 0xDD01_0002);
        let id = insert_game(mk_game(owner, 50_011, false));
        assert!(!with_game(id, |g| g.locked).unwrap());
        assert!(with_game(id, |g| g.is_allowed(stranger)).unwrap());
        assert_eq!(toggle_lock(id, stranger), None);
        assert_eq!(toggle_lock(id, owner), Some(true));
        assert!(!with_game(id, |g| g.is_allowed(stranger)).unwrap());
        assert!(with_game(id, |g| g.is_allowed(owner)).unwrap());
        assert_eq!(toggle_lock(id, owner), Some(false));
        assert_eq!(toggle_lock(0xFFFF_FFFE, owner), None);
        remove_game(id);
    }

    #[test]
    fn toggle_all_converges_to_one_state() {
        let owner = (0xAAA1_0003, 0xBBB1_0003);
        let a = insert_game(mk_game(owner, 50_021, false));
        let b = insert_game(mk_game(owner, 50_031, false));
        toggle_lock(a, owner);
        let (n, locked) = toggle_all_locks(owner);
        assert_eq!((n, locked), (2, true));
        let (n, locked) = toggle_all_locks(owner);
        assert_eq!((n, locked), (2, false));
        assert_eq!(toggle_all_locks((9, 9)), (0, false));
        remove_game(a);
        remove_game(b);
    }

    #[test]
    fn flag_styles_cycle_all_and_wrap() {
        let owner = (0xAAA1_0004, 0xBBB1_0004);
        assert_eq!(flag_block_for(owner), FLAG_STYLE_ORDER[0].1);
        assert_eq!(flag_style_name(owner), FLAG_STYLE_ORDER[0].0);
        for (name, block) in FLAG_STYLE_ORDER.iter().skip(1) {
            assert_eq!(cycle_flag_style(owner), *block);
            assert_eq!(flag_style_name(owner), *name);
        }
        assert_eq!(cycle_flag_style(owner), FLAG_STYLE_ORDER[0].1);
        assert_eq!(flag_style_name(owner), FLAG_STYLE_ORDER[0].0);
    }

    #[test]
    fn flag_style_reset_restores_default() {
        let owner = (0xAAA1_0005, 0xBBB1_0005);
        for _ in 0..4 {
            cycle_flag_style(owner);
        }
        assert_ne!(flag_style_name(owner), FLAG_STYLE_ORDER[0].0);
        assert_eq!(reset_flag_style(owner), FLAG_STYLE_ORDER[0].1);
        assert_eq!(flag_style_name(owner), FLAG_STYLE_ORDER[0].0);
        assert_eq!(flag_block_for(owner), FLAG_STYLE_ORDER[0].1);
    }

    #[test]
    fn flag_debounce_locks_then_expires() {
        let now = now_tick();
        let id = 999_981u32;
        let idx = 3usize;
        assert!(!flag_locked(id, idx, now));
        lock_flag(id, idx, now + FLAG_LOCK_TICKS);
        assert!(flag_locked(id, idx, now));
        // Expiry is strict: at exactly `until` the lock is gone.
        assert!(!flag_locked(id, idx, now + FLAG_LOCK_TICKS));
        prune_flag_locks(now + FLAG_LOCK_TICKS + 1);
        assert!(!flag_locked(id, idx, now));

        let world = "test_flag_world_expire";
        assert!(!infinite_flag_locked(world, 7, 8, now));
        lock_infinite_flag(world, 7, 8, now + FLAG_LOCK_TICKS);
        assert!(infinite_flag_locked(world, 7, 8, now));
        assert!(!infinite_flag_locked(world, 7, 8, now + FLAG_LOCK_TICKS));
    }

    #[test]
    fn infinite_session_and_board_start_empty() {
        let owner = (0xAAA1_0005, 0xBBB1_0005);
        with_session_mut(owner, |s| {
            assert_eq!(
                (s.current_streak, s.top_streak, s.total_opened, s.bombs_clicked, s.playtime_ticks),
                (0, 0, 0, 0, 0)
            );
        });
        with_infinite_board_mut("test_board_empty", |b| {
            assert!(b.cells.is_empty());
        });
    }
}
