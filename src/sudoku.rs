//! World-based 9x9 Sudoku games: generation, rendering and click handling.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use pumpkin_plugin_api::{
    ItemStack, ItemStackExt, Player, Server, World, particles::Particle, text::TextComponent,
};

use crate::{
    core::{Pos3, TinyRng},
    helpers::{self, BulkWriter, Sound, is_air, play_sound, tell},
    state::{self, MAX_BUILD_Y, MAX_GAMES_PER_PLAYER, MIN_BUILD_Y, UuidKey},
};

pub const SUDOKU_DIM: usize = 9;
pub const SUDOKU_CELLS: usize = SUDOKU_DIM * SUDOKU_DIM;
pub const SUDOKU_EMPTY_BLOCK: &str = "minecraft:stone";

/// Nine contiguous cells. The 3x3 sub-regions are marked with particles
/// instead of air gaps, so the board reads as one 9x9 square.
const FOOTPRINT: i32 = SUDOKU_DIM as i32;
/// Vanilla can emit both main-hand and off-hand interact packets for one
/// physical click; keep a short per-cell debounce like the minesweeper flags.
const CLICK_LOCK_TICKS: u64 = 4;
/// Digits kept on a fresh board. The generator removes digits down to this
/// target while the puzzle stays uniquely solvable; it may stop a little
/// above the target when no further removal would.
const GIVEN_TARGET: usize = 29;
const PARTICLE_INTERVAL_TICKS: u64 = 5;
const PARTICLE_COUNT: i32 = 12;

#[derive(Clone, Copy, Debug)]
pub struct SudokuCell {
    /// Fixed puzzle digit; 0 for cells the player may edit.
    pub given: u8,
    /// Current displayed digit; 0 means empty.
    pub value: u8,
    pub pos: Pos3,
    last_click_tick: u64,
}

pub struct SudokuGame {
    pub owner: UuidKey,
    pub owner_name: String,
    pub world_id: String,
    pub cells: Vec<SudokuCell>,
    pub helpers: Vec<UuidKey>,
    pub locked: bool,
    pub recorded: bool,
    pub comp_time_ticks: u64,
    pub seed: u64,
    pub clicks: u32,
    pub is_over: bool,
    pub is_won: bool,
}

impl SudokuGame {
    pub fn filled_count(&self) -> usize {
        self.cells.iter().filter(|c| c.value != 0).count()
    }

    pub fn has_conflict(&self) -> bool {
        (0..SUDOKU_CELLS).any(|idx| has_conflict(&self.cells, idx))
    }

    pub fn is_complete_valid(&self) -> bool {
        self.cells.iter().all(|c| c.value != 0) && !self.has_conflict()
    }

    pub fn origin(&self) -> Pos3 {
        self.cells.first().map(|c| c.pos).unwrap_or_default()
    }

    pub fn center(&self) -> Pos3 {
        self.cells
            .get(SUDOKU_CELLS / 2)
            .map(|c| c.pos)
            .unwrap_or_default()
    }

    pub fn find_cell(&self, pos: Pos3) -> Option<usize> {
        self.cells.iter().position(|c| c.pos == pos)
    }

    pub fn is_allowed(&self, who: UuidKey) -> bool {
        who == self.owner || !self.locked
    }
}

#[derive(Clone)]
pub struct Summary {
    pub id: u32,
    pub owner: UuidKey,
    pub owner_name: String,
    pub world_id: String,
    pub ticks: u64,
    pub filled: usize,
    pub total: usize,
    pub over: bool,
    pub won: bool,
    pub locked: bool,
    pub helpers: usize,
    pub center: Pos3,
}

struct State {
    games: HashMap<u32, SudokuGame>,
    pos_index: HashMap<String, HashMap<(i32, i32, i32), u32>>,
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

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn insert_game(game: SudokuGame) -> u32 {
    let id = state::next_id();
    let mut s = lock();
    let index = s.pos_index.entry(game.world_id.clone()).or_default();
    for cell in &game.cells {
        index.insert((cell.pos.x, cell.pos.y, cell.pos.z), id);
    }
    s.games.insert(id, game);
    id
}

pub fn with_game<R>(id: u32, f: impl FnOnce(&SudokuGame) -> R) -> Option<R> {
    lock().games.get(&id).map(f)
}

pub fn with_game_mut<R>(id: u32, f: impl FnOnce(&mut SudokuGame) -> R) -> Option<R> {
    lock().games.get_mut(&id).map(f)
}

pub fn route(world_id: &str, pos: Pos3) -> Option<(u32, usize)> {
    let id = lock()
        .pos_index
        .get(world_id)?
        .get(&(pos.x, pos.y, pos.z))
        .copied()?;
    let idx = with_game(id, |g| g.find_cell(pos))??;
    Some((id, idx))
}

fn remove_game(id: u32) -> Option<SudokuGame> {
    let mut s = lock();
    let game = s.games.remove(&id)?;
    let empty = if let Some(index) = s.pos_index.get_mut(&game.world_id) {
        for cell in &game.cells {
            let key = (cell.pos.x, cell.pos.y, cell.pos.z);
            if index.get(&key) == Some(&id) {
                index.remove(&key);
            }
        }
        index.is_empty()
    } else {
        false
    };
    if empty {
        s.pos_index.remove(&game.world_id);
    }
    Some(game)
}

pub fn player_game_count(owner: UuidKey) -> usize {
    lock().games.values().filter(|g| g.owner == owner).count()
}

pub fn update_owner_name(owner: UuidKey, name: &str) {
    let mut s = lock();
    for game in s.games.values_mut().filter(|g| g.owner == owner) {
        game.owner_name = name.to_string();
    }
}

pub fn player_games(owner: UuidKey) -> Vec<u32> {
    lock()
        .games
        .iter()
        .filter(|(_, g)| g.owner == owner)
        .map(|(id, _)| *id)
        .collect()
}

pub fn any_locked(owner: UuidKey) -> bool {
    lock().games.values().any(|g| g.owner == owner && g.locked)
}

pub fn any_unlocked(owner: UuidKey) -> bool {
    lock().games.values().any(|g| g.owner == owner && !g.locked)
}

pub fn set_all_locks(owner: UuidKey, locked: bool) -> usize {
    let mut s = lock();
    let mut n = 0;
    for game in s.games.values_mut().filter(|g| g.owner == owner) {
        game.locked = locked;
        n += 1;
    }
    n
}

pub fn toggle_lock(id: u32, owner: UuidKey) -> Option<bool> {
    let mut s = lock();
    let game = s.games.get_mut(&id)?;
    if game.owner != owner {
        return None;
    }
    game.locked = !game.locked;
    Some(game.locked)
}

pub fn advance_game_timers() {
    let mut s = lock();
    for game in s.games.values_mut() {
        if !game.is_over && state::is_online(game.owner) {
            game.comp_time_ticks = game.comp_time_ticks.saturating_add(1);
        }
    }
}

fn summary(id: u32, game: &SudokuGame) -> Summary {
    Summary {
        id,
        owner: game.owner,
        owner_name: game.owner_name.clone(),
        world_id: game.world_id.clone(),
        ticks: game.comp_time_ticks,
        filled: game.filled_count(),
        total: SUDOKU_CELLS,
        over: game.is_over,
        won: game.is_won,
        locked: game.locked,
        helpers: game.helpers.len(),
        center: game.center(),
    }
}

pub fn all_summaries() -> Vec<Summary> {
    lock()
        .games
        .iter()
        .map(|(id, game)| summary(*id, game))
        .collect()
}

pub fn nearest_candidate(world_id: &str, pos: (f64, f64, f64)) -> Option<(f64, Summary)> {
    all_summaries()
        .into_iter()
        .filter(|g| g.world_id == world_id)
        .map(|g| {
            let dx = g.center.x as f64 - pos.0;
            let dy = g.center.y as f64 - pos.1;
            let dz = g.center.z as f64 - pos.2;
            (dx * dx + dy * dy + dz * dz, g)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)))
}

pub fn descriptions(owner_filter: Option<UuidKey>) -> Vec<String> {
    all_summaries()
        .into_iter()
        .filter(|g| owner_filter.is_none_or(|owner| owner == g.owner))
        .map(|g| {
            format!(
                "#{} Sudoku 9x9 {time}s filled={filled}/{} helpers={helpers} {state}{locked}",
                g.id,
                g.total,
                time = g.ticks / 20,
                filled = g.filled,
                helpers = g.helpers,
                state = if g.over {
                    if g.won { "won" } else { "lost" }
                } else {
                    "active"
                },
                locked = if g.locked { " locked" } else { "" },
            )
        })
        .collect()
}

fn shuffled_digits(rng: &mut TinyRng) -> [u8; 9] {
    let mut nums = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    for i in (1..nums.len()).rev() {
        let j = (rng.next_u64() as usize) % (i + 1);
        nums.swap(i, j);
    }
    nums
}

/// `(row, column, box)` for a cell index.
fn cell_units(idx: usize) -> (usize, usize, usize) {
    let row = idx / SUDOKU_DIM;
    let col = idx % SUDOKU_DIM;
    (row, col, (row / 3) * 3 + col / 3)
}

/// Set bit for digit `n` (1-9).
const fn digit_bit(n: u8) -> u16 {
    1u16 << (n - 1)
}

const ALL_DIGITS: u16 = 0x1FF;

/// Count the complete conflict-free grids reachable from `puzzle`
/// (0 = empty cell), stopping once `cap` solutions are found. Bitmask
/// backtracking with MRV cell selection; cheap enough (well under a
/// millisecond for playable puzzles) to run once per removed digit while a
/// board is being generated. Returns 0 for conflicting givens.
fn count_solutions(puzzle: &[u8; SUDOKU_CELLS], cap: usize) -> usize {
    let mut buffer = *puzzle;
    let mut row_used = [0u16; 9];
    let mut col_used = [0u16; 9];
    let mut box_used = [0u16; 9];
    for (idx, &value) in buffer.iter().enumerate() {
        if value == 0 {
            continue;
        }
        let bit = digit_bit(value);
        let (row, col, box_idx) = cell_units(idx);
        if row_used[row] & bit != 0 || col_used[col] & bit != 0 || box_used[box_idx] & bit != 0 {
            return 0;
        }
        row_used[row] |= bit;
        col_used[col] |= bit;
        box_used[box_idx] |= bit;
    }
    let mut count = 0usize;
    count_solutions_from(
        &mut buffer,
        &mut row_used,
        &mut col_used,
        &mut box_used,
        cap,
        &mut count,
    );
    count
}

fn count_solutions_from(
    puzzle: &mut [u8; SUDOKU_CELLS],
    row_used: &mut [u16; 9],
    col_used: &mut [u16; 9],
    box_used: &mut [u16; 9],
    cap: usize,
    count: &mut usize,
) {
    if *count >= cap {
        return;
    }
    // Minimum Remaining Values: branch on the empty cell with the fewest
    // legal digits. The one-candidate cell is the common cheap case, so the
    // scan stops as soon as it is found.
    let mut best_pop = usize::MAX;
    let mut best_idx = None;
    let mut best_cands = 0u16;
    for (idx, &cell) in puzzle.iter().enumerate() {
        if cell != 0 {
            continue;
        }
        let (row, col, box_idx) = cell_units(idx);
        let cands = ALL_DIGITS & !row_used[row] & !col_used[col] & !box_used[box_idx];
        let pop = cands.count_ones() as usize;
        if pop == 0 {
            return; // dead end
        }
        if pop < best_pop {
            best_pop = pop;
            best_idx = Some(idx);
            best_cands = cands;
            if pop == 1 {
                break;
            }
        }
    }
    let Some(idx) = best_idx else {
        // No empty cells left: that is one full valid grid.
        *count += 1;
        return;
    };
    let (row, col, box_idx) = cell_units(idx);
    let mut cands = best_cands;
    while cands != 0 && *count < cap {
        let bit = cands & cands.wrapping_neg();
        cands ^= bit;
        // Write the digit into the working grid (MRV relies on it from this
        // depth on), recurse, then take it back out.
        let value = bit.trailing_zeros() as u8 + 1;
        puzzle[idx] = value;
        row_used[row] |= bit;
        col_used[col] |= bit;
        box_used[box_idx] |= bit;
        count_solutions_from(puzzle, row_used, col_used, box_used, cap, count);
        row_used[row] ^= bit;
        col_used[col] ^= bit;
        box_used[box_idx] ^= bit;
        puzzle[idx] = 0;
    }
}

fn can_place(grid: &[u8; SUDOKU_CELLS], row: usize, col: usize, n: u8) -> bool {
    for c in 0..SUDOKU_DIM {
        if grid[row * SUDOKU_DIM + c] == n {
            return false;
        }
    }
    for r in 0..SUDOKU_DIM {
        if grid[r * SUDOKU_DIM + col] == n {
            return false;
        }
    }
    let br = (row / 3) * 3;
    let bc = (col / 3) * 3;
    for r in br..br + 3 {
        for c in bc..bc + 3 {
            if grid[r * SUDOKU_DIM + c] == n {
                return false;
            }
        }
    }
    true
}

fn fill_grid(grid: &mut [u8; SUDOKU_CELLS], idx: usize, rng: &mut TinyRng) -> bool {
    if idx == SUDOKU_CELLS {
        return true;
    }
    if grid[idx] != 0 {
        return fill_grid(grid, idx + 1, rng);
    }
    let row = idx / SUDOKU_DIM;
    let col = idx % SUDOKU_DIM;
    for n in shuffled_digits(rng) {
        if !can_place(grid, row, col, n) {
            continue;
        }
        grid[idx] = n;
        if fill_grid(grid, idx + 1, rng) {
            return true;
        }
        grid[idx] = 0;
    }
    false
}

/// Returns `(complete_solution, given_mask)`. The mask is generated by
/// greedily blanking digits of a full valid grid and undoing any removal
/// that would create a second valid solution, so every puzzle is
/// **uniquely solvable** and the win rule (any conflict-free full grid)
/// exactly matches the intended solution. The real given count may end up
/// above `target` when no further removal stays unique.
pub fn generate_puzzle(rng: &mut TinyRng) -> ([u8; SUDOKU_CELLS], [bool; SUDOKU_CELLS]) {
    generate_puzzle_targeted(rng, GIVEN_TARGET)
}

fn generate_puzzle_targeted(
    rng: &mut TinyRng,
    target: usize,
) -> ([u8; SUDOKU_CELLS], [bool; SUDOKU_CELLS]) {
    let mut solution = [0u8; SUDOKU_CELLS];
    let filled = fill_grid(&mut solution, 0, rng);
    debug_assert!(filled, "backtracking must fill a 9x9 grid");

    let mut given = [true; SUDOKU_CELLS];
    let mut puzzle = solution;

    let mut order: Vec<usize> = (0..SUDOKU_CELLS).collect();
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() as usize) % (i + 1);
        order.swap(i, j);
    }

    let mut remaining = SUDOKU_CELLS;
    for idx in order {
        if remaining <= target {
            break;
        }
        // Blank the cell and undo the removal if it would allow a second
        // valid solution.
        puzzle[idx] = 0;
        if count_solutions(&puzzle, 2) == 1 {
            given[idx] = false;
            remaining -= 1;
        } else {
            puzzle[idx] = solution[idx];
        }
    }
    debug_assert_eq!(count_solutions(&puzzle, 2), 1, "mask lost uniqueness");

    (solution, given)
}

fn cell_pos(origin: Pos3, row: usize, col: usize) -> Pos3 {
    origin.offset(col as i32, 0, row as i32)
}

fn find_origin(world: &World, feet: Pos3) -> Option<Pos3> {
    let floor_y = (feet.y - 10).max(MIN_BUILD_Y);
    let mut y = feet.y - 1;
    while y >= floor_y && y >= MIN_BUILD_Y {
        if y <= MAX_BUILD_Y {
            // Centre the 9x9 footprint under the player.
            let origin = Pos3::new(feet.x - FOOTPRINT / 2, y, feet.z - FOOTPRINT / 2);
            let mut free = true;
            'rows: for dx in 0..FOOTPRINT {
                for dz in 0..FOOTPRINT {
                    if !is_air(world, origin.offset(dx, 0, dz)) {
                        free = false;
                        break 'rows;
                    }
                }
            }
            if free {
                return Some(origin);
            }
        }
        y -= 1;
    }
    None
}

fn seed_now(owner: UuidKey) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ owner.0 ^ owner.1
}

/// Blocks a player places: wool in the same nine hues as the concrete
/// givens. Wool's bold, fuzzy look is chosen so placed digits always read
/// as "player moves" against the smoother concrete given cells.
pub fn placed_block(digit: u8) -> &'static str {
    match digit {
        1 => "minecraft:white_wool",
        2 => "minecraft:yellow_wool",
        3 => "minecraft:orange_wool",
        4 => "minecraft:red_wool",
        5 => "minecraft:magenta_wool",
        6 => "minecraft:blue_wool",
        7 => "minecraft:light_blue_wool",
        8 => "minecraft:lime_wool",
        9 => "minecraft:pink_wool",
        _ => SUDOKU_EMPTY_BLOCK,
    }
}

/// Digits the board came prefilled with. Same hue as `placed_block`, but
/// the plain concrete texture family, so given cells always read as
/// "printed on the board" while every digit a player drops is colored wool.
pub fn given_block(digit: u8) -> &'static str {
    match digit {
        1 => "minecraft:white_concrete",
        2 => "minecraft:yellow_concrete",
        3 => "minecraft:orange_concrete",
        4 => "minecraft:red_concrete",
        5 => "minecraft:magenta_concrete",
        6 => "minecraft:blue_concrete",
        7 => "minecraft:light_blue_concrete",
        8 => "minecraft:lime_concrete",
        9 => "minecraft:pink_concrete",
        _ => SUDOKU_EMPTY_BLOCK,
    }
}

pub fn create_game(player: &Player) {
    let owner = helpers::uuid_key(player);
    let total_games = state::player_game_count(owner) + player_game_count(owner);
    if total_games >= MAX_GAMES_PER_PLAYER {
        tell(
            player,
            "You can't have more than 3 games active (/ms clear).",
        );
        return;
    }

    let world = player.get_world();
    let world_id = world.get_id();
    let (px, py, pz) = player.get_position();
    let feet = Pos3::new(px.floor() as i32, py.floor() as i32, pz.floor() as i32);

    let Some(origin) = find_origin(&world, feet) else {
        tell(
            player,
            &format!(
                "No free 9x9 space within 10 blocks below your feet ({}, {}, {}). Fly up a bit; boards only spawn in open air below you.",
                feet.x, feet.y, feet.z
            ),
        );
        return;
    };

    let seed = seed_now(owner);
    let mut rng = TinyRng::new(seed);
    let (solution, given) = generate_puzzle(&mut rng);

    let mut cells = Vec::with_capacity(SUDOKU_CELLS);
    for row in 0..SUDOKU_DIM {
        for col in 0..SUDOKU_DIM {
            let idx = row * SUDOKU_DIM + col;
            let value = if given[idx] { solution[idx] } else { 0 };
            cells.push(SudokuCell {
                given: value,
                value,
                pos: cell_pos(origin, row, col),
                last_click_tick: 0,
            });
        }
    }

    let mut bulk = BulkWriter::new(&world);
    for cell in &cells {
        bulk.set(
            cell.pos,
            if cell.value == 0 {
                SUDOKU_EMPTY_BLOCK
            } else {
                given_block(cell.value)
            },
        );
    }

    let game = SudokuGame {
        owner,
        owner_name: player.get_name(),
        world_id,
        cells,
        helpers: Vec::new(),
        locked: false,
        recorded: false,
        comp_time_ticks: 0,
        seed,
        clicks: 0,
        is_over: false,
        is_won: false,
    };
    let id = insert_game(game);
    tracing::info!(
        "Sudoku #{} for {} at {}, {}, {}",
        id,
        player.get_name(),
        origin.x,
        origin.y,
        origin.z
    );
    crate::hotbar::sync(player, Some(crate::hotbar::Mode::Sudoku));
    tell(
        player,
        &format!(
            "Sudoku #{} started at {}, {}, {} ({} tiles). While you are near the board your hotbar becomes digits 1-9: select a number, right-click a cell to place it, left-click a cell to clear it. Given cells are fixed. /ms clear removes it.",
            id, origin.x, origin.y, origin.z, SUDOKU_CELLS
        ),
    );
}

fn render_cell(world: &World, pos: Pos3, value: u8) {
    if value == 0 {
        helpers::set_block(world, pos, SUDOKU_EMPTY_BLOCK);
    } else {
        helpers::set_block(world, pos, placed_block(value));
    }
}

fn has_conflict(cells: &[SudokuCell], idx: usize) -> bool {
    let value = cells[idx].value;
    if value == 0 {
        return false;
    }
    let row = idx / SUDOKU_DIM;
    let col = idx % SUDOKU_DIM;
    for c in 0..SUDOKU_DIM {
        let other = row * SUDOKU_DIM + c;
        if other != idx && cells[other].value == value {
            return true;
        }
    }
    for r in 0..SUDOKU_DIM {
        let other = r * SUDOKU_DIM + col;
        if other != idx && cells[other].value == value {
            return true;
        }
    }
    let br = (row / 3) * 3;
    let bc = (col / 3) * 3;
    for r in br..br + 3 {
        for c in bc..bc + 3 {
            let other = r * SUDOKU_DIM + c;
            if other != idx && cells[other].value == value {
                return true;
            }
        }
    }
    false
}

enum Outcome {
    Played,
    Conflict,
    Swallowed,
    Given,
    Locked,
    Finished,
    Won,
}

fn time_text(ticks: u64) -> String {
    let secs = ticks / 20;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

fn hotbar_item(digit: u8) -> ItemStack {
    let stack = ItemStack::of(placed_block(digit), 1);
    stack.set_custom_name(Some(TextComponent::text(&digit.to_string())));
    stack.set_lore(vec![
        TextComponent::text(&format!("Sudoku digit {digit} - colored wool")),
        TextComponent::text("Right-click a cell to place"),
        TextComponent::text("Left-click a cell to clear"),
    ]);
    stack
}

/// Lay out the nine Sudoku digit blocks across hotbar slots 0-8. Called by
/// `crate::hotbar` when the player enters Sudoku mode.
pub(crate) fn apply_digit_hotbar(player: &Player) {
    for digit in 1..=9u8 {
        player.set_inventory_item(digit - 1, Some(hotbar_item(digit)));
    }
}

fn selected_digit(player: &Player) -> Option<u8> {
    let in_sudoku = crate::hotbar::in_mode(player, crate::hotbar::Mode::Sudoku);
    in_sudoku.then(|| player.get_selected_slot().min(8) + 1)
}

pub fn click_left(world: &World, player: &Player, id: u32, idx: usize) -> bool {
    crate::hotbar::sync(player, Some(crate::hotbar::Mode::Sudoku));
    place(world, player, id, idx, 0)
}

pub fn click_right(world: &World, player: &Player, id: u32, idx: usize) -> bool {
    crate::hotbar::sync(player, Some(crate::hotbar::Mode::Sudoku));
    let Some(digit) = selected_digit(player) else {
        tell(player, "Sudoku hotbar is not ready; step away and back.");
        return true;
    };
    place(world, player, id, idx, digit)
}

fn place(world: &World, player: &Player, id: u32, idx: usize, value: u8) -> bool {
    let who = helpers::uuid_key(player);
    let over_owned = with_game(id, |g| g.is_over && g.owner == who).unwrap_or(false);
    if over_owned {
        record_game_end(id, false);
        clear_game_in_world(world, id);
        crate::hotbar::leave(player);
        tell(
            player,
            "Sudoku game cleared. Start another with the Sudoku stick.",
        );
        return true;
    }

    let outcome = with_game_mut(id, |game| {
        if game.is_over {
            return Outcome::Finished;
        }
        if !game.is_allowed(who) {
            return Outcome::Locked;
        }
        let Some(cell) = game.cells.get_mut(idx) else {
            return Outcome::Swallowed;
        };
        if cell.given != 0 {
            return Outcome::Given;
        }
        let now = state::now_tick();
        if now < cell.last_click_tick.saturating_add(CLICK_LOCK_TICKS) {
            return Outcome::Swallowed;
        }
        if cell.value == value {
            return Outcome::Swallowed;
        }
        cell.last_click_tick = now;
        cell.value = value;
        let pos = cell.pos;
        game.clicks = game.clicks.saturating_add(1);

        if who != game.owner && !game.helpers.contains(&who) {
            game.helpers.push(who);
        }
        render_cell(world, pos, value);
        play_sound(
            world,
            pos,
            if value == 0 {
                Sound::BlockWoolBreak
            } else {
                Sound::BlockWoolPlace
            },
        );

        if game.is_complete_valid() {
            game.is_over = true;
            game.is_won = true;
            Outcome::Won
        } else if has_conflict(&game.cells, idx) {
            // No actionbar feedback (the proxy broadcasts actionbar text) -
            // a dull low-pitched thud marks the conflicting placement.
            helpers::play_named_sound_pitched(world, pos, "block.note_block.bass", 0.6);
            Outcome::Conflict
        } else {
            Outcome::Played
        }
    });

    match outcome {
        Some(Outcome::Locked) => {
            tell(player, "That Sudoku game is locked by its owner.");
        }
        Some(Outcome::Finished) => {
            tell(player, "That Sudoku game is already finished.");
        }
        Some(Outcome::Won) => {
            record_game_end(id, false);
            if let Some((ticks, center)) = with_game(id, |g| (g.comp_time_ticks, g.center())) {
                play_sound(world, center, Sound::ItemGoatHornSound1);
                tell(
                    player,
                    &format!(
                        "Sudoku #{id} solved in {}! Click your finished board to clear it.",
                        time_text(ticks)
                    ),
                );
            }
        }
        // Placed/played/conflict/given cells report through the sidebar
        // progress, sounds, and the visible block, not through text.
        _ => {}
    }
    true
}

fn record_game_end(id: u32, allow_unfinished: bool) {
    let snapshot = with_game(id, |g| {
        (
            g.seed,
            g.comp_time_ticks,
            g.is_over,
            g.is_won,
            g.owner,
            g.owner_name.clone(),
            g.helpers.clone(),
            g.recorded,
            g.locked,
            g.clicks,
            g.filled_count(),
            g.origin(),
        )
    });
    #[allow(clippy::type_complexity)]
    let Some((
        seed,
        ticks,
        over,
        won,
        owner,
        owner_name,
        helpers,
        recorded,
        locked,
        clicks,
        filled,
        origin,
    )) = snapshot
    else {
        return;
    };
    if recorded {
        return;
    }
    let outcome = if over {
        if won {
            crate::db::OUTCOME_WIN
        } else {
            crate::db::OUTCOME_LOSS
        }
    } else if allow_unfinished {
        crate::db::OUTCOME_LOSS
    } else {
        return;
    };
    let record = crate::db::GameRecord {
        owner,
        owner_name: Some(owner_name.as_str()),
        helpers: &helpers,
        kind: crate::db::KIND_SUDOKU,
        dim: SUDOKU_DIM,
        seed,
        time_ticks: ticks,
        outcome,
        bombs: 0,
        flags: 0,
        flags_placed: 0,
        opened: filled,
        clicks,
        locked,
        x: origin.x,
        y: origin.y,
        z: origin.z,
    };
    if crate::db::record_game(&record) {
        with_game_mut(id, |g| g.recorded = true);
    }
}

fn clear_blocks(world: &World, game: &SudokuGame) {
    let mut bulk = BulkWriter::new(world);
    for cell in &game.cells {
        bulk.set(cell.pos, crate::core::AIR_BLOCK);
    }
}

pub fn clear_game_in_world(world: &World, id: u32) {
    if let Some(game) = remove_game(id) {
        clear_blocks(world, &game);
    }
}

pub fn clear_player_games(player: &Player, server: &Server) -> usize {
    let owner = helpers::uuid_key(player);
    for id in player_games(owner) {
        record_game_end(id, true);
    }
    let mut n = 0;
    for game in remove_player_games(owner) {
        match server.get_world_by_name(&game.world_id) {
            Some(world) => clear_blocks(&world, &game),
            None => clear_blocks(&player.get_world(), &game),
        }
        n += 1;
    }
    n
}

fn remove_player_games(owner: UuidKey) -> Vec<SudokuGame> {
    let ids: Vec<u32> = lock()
        .games
        .iter()
        .filter(|(_, g)| g.owner == owner)
        .map(|(id, _)| *id)
        .collect();
    ids.into_iter().filter_map(remove_game).collect()
}

/// Admin helper: clear the Sudoku board closest to `pos`. Unfinished games
/// are recorded as losses, like minesweeper boards.
pub fn admin_clear_nearest(world: &World, pos: (f64, f64, f64)) -> Option<(u32, String, String)> {
    let (_, summary) = nearest_candidate(&world.get_id(), pos)?;
    let owner_name = summary.owner_name.clone();
    record_game_end(summary.id, true);
    clear_game_in_world(world, summary.id);
    Some((summary.id, owner_name, "Sudoku".to_string()))
}

fn grid_particle_lines(center: Pos3) -> Vec<((f64, f64, f64), (f64, f64, f64))> {
    let origin = center.offset(-(FOOTPRINT / 2), 0, -(FOOTPRINT / 2));
    let y = origin.y as f64 + 1.02;

    let mut particles = Vec::new();
    let step = 0.2;

    for x in [origin.x as f64 + 3.0, origin.x as f64 + 6.0] {
        let mut z = origin.z as f64;
        while z <= origin.z as f64 + 9.0 {
            particles.push(((x, y, z), (0.0, 0.0, 0.0)));
            z += step;
        }
    }

    for z in [origin.z as f64 + 3.0, origin.z as f64 + 6.0] {
        let mut x = origin.x as f64;
        while x <= origin.x as f64 + 9.0 {
            particles.push(((x, y, z), (0.0, 0.0, 0.0)));
            x += step;
        }
    }

    particles
}

pub fn spawn_grid_particles(server: &Server, now: u64) {
    if now % PARTICLE_INTERVAL_TICKS != 0 {
        return;
    }

    let games = all_summaries();
    if games.is_empty() {
        return;
    }

    for game in games {
        let Some(world) = server.get_world_by_name(&game.world_id) else {
            continue;
        };

        for (pos, offset) in grid_particle_lines(game.center) {
            world.spawn_particle(Particle::EndRod, pos, offset, 0.0, PARTICLE_COUNT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_solution(grid: &[u8; SUDOKU_CELLS]) -> bool {
        if grid.contains(&0) {
            return false;
        }
        for i in 0..SUDOKU_DIM {
            let mut row_seen = [false; 10];
            let mut col_seen = [false; 10];
            let mut box_seen = [false; 10];
            for j in 0..SUDOKU_DIM {
                let row_digit = grid[i * SUDOKU_DIM + j] as usize;
                if row_seen[row_digit] {
                    return false;
                }
                row_seen[row_digit] = true;

                let col_digit = grid[j * SUDOKU_DIM + i] as usize;
                if col_seen[col_digit] {
                    return false;
                }
                col_seen[col_digit] = true;

                let br = (i / 3) * 3;
                let bc = (i % 3) * 3;
                let box_digit = grid[(br + j / 3) * SUDOKU_DIM + bc + j % 3] as usize;
                if box_seen[box_digit] {
                    return false;
                }
                box_seen[box_digit] = true;
            }
        }
        true
    }

    #[test]
    fn generated_puzzles_have_valid_unique_solutions_and_givens() {
        for seed in 1..=12 {
            let mut rng = TinyRng::new(seed);
            let (solution, given) = generate_puzzle(&mut rng);
            assert!(
                valid_solution(&solution),
                "seed {seed} produced an invalid grid"
            );
            let count = given.iter().filter(|&&g| g).count();
            assert!(
                count >= GIVEN_TARGET,
                "seed {seed} gave {count} givens (target {GIVEN_TARGET})"
            );
            for idx in 0..SUDOKU_CELLS {
                if given[idx] {
                    assert_ne!(solution[idx], 0);
                }
            }
            // Uniqueness: the blanked puzzle must have exactly one solution,
            // so the any-conflict-free-grid win rule is the intended one.
            assert_eq!(
                count_solutions(&masked(&solution, &given), 2),
                1,
                "seed {seed} produced a multi-solution puzzle"
            );
        }
    }

    fn masked(solution: &[u8; SUDOKU_CELLS], given: &[bool; SUDOKU_CELLS]) -> [u8; SUDOKU_CELLS] {
        let mut puzzle = *solution;
        for (idx, keep) in given.iter().enumerate() {
            if !keep {
                puzzle[idx] = 0;
            }
        }
        puzzle
    }

    #[test]
    fn count_solutions_counts_up_to_cap_and_rejects_conflicts() {
        // Empty grid: so many solutions the 2-cap always trips.
        assert_eq!(count_solutions(&[0u8; SUDOKU_CELLS], 2), 2);
        assert_eq!(count_solutions(&[0u8; SUDOKU_CELLS], 1), 1);
        assert_eq!(count_solutions(&[0u8; SUDOKU_CELLS], 0), 0);

        // Conflicting givens are not solvable at all.
        let mut bad = [0u8; SUDOKU_CELLS];
        bad[0] = 5;
        bad[1] = 5;
        assert_eq!(count_solutions(&bad, 2), 0);

        // A complete valid grid is its own single solution.
        let mut rng = TinyRng::new(7);
        let (solution, _) = generate_puzzle(&mut rng);
        assert_eq!(count_solutions(&solution, 2), 1);

        // Bumping one filled cell off its solution makes it unsolvable
        // (that digit is already used elsewhere in the row). Blank instead
        // and the puzzle regains >= 1 solution.
        let mut off = solution;
        off[SUDOKU_CELLS - 1] = ((off[SUDOKU_CELLS - 1]) % 9) + 1;
        assert_eq!(count_solutions(&off, 2), 0);
    }

    #[test]
    fn targeted_puzzles_stay_uniquely_solvable() {
        for target in [29usize, 25, 22] {
            let mut rng = TinyRng::new(target as u64 + 100);
            let (solution, given) = generate_puzzle_targeted(&mut rng, target);
            // No per-unit floor anymore: the invariant is uniqueness, and
            // the given count is whatever greedy removal stays unique for.
            assert_eq!(count_solutions(&masked(&solution, &given), 2), 1);
        }
    }

    #[test]
    #[ignore = "informational: prints given-count histograms; run manually"]
    fn generator_stats() {
        for target in [GIVEN_TARGET, 25usize, 22] {
            let mut histogram: std::collections::BTreeMap<usize, usize> = Default::default();
            let mut total = std::time::Duration::ZERO;
            let mut worst = std::time::Duration::ZERO;
            let runs = 200usize;
            for seed in 1..=runs as u64 {
                let start = std::time::Instant::now();
                let mut rng = TinyRng::new(seed.rotate_left(target as u32));
                let (solution, given) = generate_puzzle_targeted(&mut rng, target);
                total += start.elapsed();
                worst = worst.max(start.elapsed());
                *histogram
                    .entry(given.iter().filter(|&&g| g).count())
                    .or_default() += 1;
                assert_eq!(count_solutions(&masked(&solution, &given), 2), 1);
            }
            println!(
                "target={target} hist={histogram:?} avg_ms={:.2} worst_ms={:.2}",
                total.as_secs_f64() * 1000.0 / runs as f64,
                worst.as_secs_f64() * 1000.0,
            );
        }
    }

    #[test]
    fn complete_grid_wins_and_duplicates_conflict() {
        let mut rng = TinyRng::new(42);
        let (solution, given) = generate_puzzle(&mut rng);
        let cells: Vec<SudokuCell> = solution
            .iter()
            .enumerate()
            .map(|(idx, &value)| SudokuCell {
                given: if given[idx] { value } else { 0 },
                value,
                pos: Pos3::new(idx as i32, 0, 0),
                last_click_tick: 0,
            })
            .collect();
        let game = SudokuGame {
            owner: (1, 2),
            owner_name: "test".into(),
            world_id: "test".into(),
            cells,
            helpers: Vec::new(),
            locked: false,
            recorded: false,
            comp_time_ticks: 0,
            seed: 1,
            clicks: 0,
            is_over: false,
            is_won: false,
        };
        assert!(game.is_complete_valid());

        let mut broken = game;
        let original = broken.cells[0].value;
        broken.cells[1].value = original;
        assert!(!broken.is_complete_valid());
    }

    #[test]
    fn route_indexes_cells_and_cleanup() {
        let mut cells = Vec::new();
        for idx in 0..SUDOKU_CELLS {
            cells.push(SudokuCell {
                given: 0,
                value: 0,
                pos: Pos3::new(10_000 + idx as i32, 200, 20_000),
                last_click_tick: 0,
            });
        }
        let game = SudokuGame {
            owner: (99, 100),
            owner_name: "route-test".into(),
            world_id: "sudoku-route-test".into(),
            cells,
            helpers: Vec::new(),
            locked: false,
            recorded: false,
            comp_time_ticks: 0,
            seed: 7,
            clicks: 0,
            is_over: false,
            is_won: false,
        };
        let id = insert_game(game);
        assert_eq!(
            route("sudoku-route-test", Pos3::new(10_000, 200, 20_000)),
            Some((id, 0))
        );
        assert_eq!(
            route("sudoku-route-test", Pos3::new(10_080, 200, 20_000)),
            Some((id, SUDOKU_CELLS - 1))
        );
        assert_eq!(route("other-world", Pos3::new(10_000, 200, 20_000)), None);
        assert!(remove_game(id).is_some());
        assert_eq!(
            route("sudoku-route-test", Pos3::new(10_000, 200, 20_000)),
            None
        );
    }

    #[test]
    fn cell_offsets_are_contiguous() {
        let origin = Pos3::new(10, 64, -3);
        assert_eq!(cell_pos(origin, 0, 0), Pos3::new(10, 64, -3));
        assert_eq!(cell_pos(origin, 0, 2), Pos3::new(12, 64, -3));
        assert_eq!(cell_pos(origin, 0, 3), Pos3::new(13, 64, -3));
        assert_eq!(cell_pos(origin, 3, 0), Pos3::new(10, 64, 0));
        assert_eq!(cell_pos(origin, 8, 8), Pos3::new(18, 64, 5));
    }

    #[test]
    fn particle_lines_are_on_box_boundaries() {
        let center = Pos3::new(10 + 4, 64, -3 + 4);
        let lines = grid_particle_lines(center);
        assert!(!lines.is_empty());
        // Every point sits on top of the board plane.
        for (pos, offset) in &lines {
            assert_eq!(pos.1, 64.0 + 1.02);
            assert_eq!(*offset, (0.0, 0.0, 0.0));
            // Nothing escapes the 9x9 footprint.
            assert!((10.0..=19.0).contains(&pos.0));
            assert!((-3.0..=6.0).contains(&pos.2));
        }
        // All four 3x3 box boundaries are covered.
        assert!(lines.iter().any(|(p, _)| p.0 == 13.0));
        assert!(lines.iter().any(|(p, _)| p.0 == 16.0));
        assert!(lines.iter().any(|(p, _)| p.2 == 0.0));
        assert!(lines.iter().any(|(p, _)| p.2 == 3.0));
        // Points are sampled densely enough to read as lines.
        assert!(lines.len() >= 4 * (9.0 / 0.2) as usize);
    }

    #[test]
    fn placed_palette_has_nine_distinct_wool_blocks() {
        let expected = [
            "minecraft:white_wool",
            "minecraft:yellow_wool",
            "minecraft:orange_wool",
            "minecraft:red_wool",
            "minecraft:magenta_wool",
            "minecraft:blue_wool",
            "minecraft:light_blue_wool",
            "minecraft:lime_wool",
            "minecraft:pink_wool",
        ];
        let blocks: Vec<&str> = (1..=9).map(placed_block).collect();
        assert_eq!(blocks, expected);
        // No duplicate hues and never the empty-cell block.
        for (i, block) in blocks.iter().enumerate() {
            assert_ne!(*block, SUDOKU_EMPTY_BLOCK);
            assert!(!blocks[..i].contains(block));
        }
        // Definitely no black.
        assert!(!blocks.contains(&"minecraft:black_wool"));
    }

    #[test]
    fn given_blocks_are_concrete_twin_of_the_placed_palette() {
        for digit in 1..=9u8 {
            let placed = placed_block(digit).strip_suffix("_wool").unwrap();
            let given = given_block(digit).strip_suffix("_concrete").unwrap();
            assert_eq!(placed, given, "digit {digit} color mismatch");
            assert_ne!(given_block(digit), SUDOKU_EMPTY_BLOCK);
        }
        let given: Vec<&str> = (1..=9).map(given_block).collect();
        for (i, block) in given.iter().enumerate() {
            assert!(!given[..i].contains(block));
        }
    }
}
