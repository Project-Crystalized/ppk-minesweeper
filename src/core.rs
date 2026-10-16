//! Pure minesweeper logic with no Pumpkin/Valence dependencies.
//! Everything here is unit-testable on the host.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CellContent {
    #[default]
    Empty,
    Bomb,
    Number(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CellState {
    #[default]
    Closed,
    Opened,
    Flagged,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct Pos3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos3 {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn offset(&self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
        }
    }
}

/// Tiny deterministic RNG (splitmix64) so board generation works on
/// wasm32-wasip2 with no external crates.
pub struct TinyRng(u64);

impl TinyRng {
    pub fn new(seed: u64) -> Self {
        // Avoid the degenerate zero state.
        Self(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut z = self.0.wrapping_add(0x9E3779B97F4A7C15);
        self.0 = z;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}

/// Deterministic hash for infinite-mode cells: `(x, z, seed) -> u64`.
pub fn hash2(x: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed.wrapping_add(0x9E3779B97F4A7C15);
    h ^= (x as u64).wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= (z as u64).wrapping_mul(0x94D049BB133111EB);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D049BB133111EB);
    h ^ (h >> 31)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cell {
    pub content: CellContent,
    pub state: CellState,
    pub pos: Pos3,
}

// ---------------------------------------------------------------------------
// 2D boards
// ---------------------------------------------------------------------------

pub fn bomb_amount_2d(dim: usize) -> usize {
    dim.pow(2).div_euclid(6).max(1)
}

pub fn generate_board_2d(dim: usize, bomb_amt: usize, origin: Pos3, rng: &mut TinyRng) -> Vec<Cell> {
    let mut board = vec![Cell::default(); dim * dim];
    place_bombs_exact(&mut board, bomb_amt, rng);
    for y in 0..dim {
        for x in 0..dim {
            let idx = y * dim + x;
            board[idx].pos = origin.offset(x as i32, 0, y as i32);
            if board[idx].content == CellContent::Bomb {
                continue;
            }
            let n = count_bombs_2d(&board, dim, x, y);
            if n >= 1 {
                board[idx].content = CellContent::Number(n);
            }
        }
    }
    board
}

/// Place exactly `bomb_amt` bombs at distinct random cells (Fisher-Yates).
fn place_bombs_exact(board: &mut [Cell], bomb_amt: usize, rng: &mut TinyRng) {
    let total = board.len();
    let bomb_amt = bomb_amt.min(total);
    let mut indices: Vec<usize> = (0..total).collect();
    for i in 0..bomb_amt {
        let j = i + (rng.next_u64() as usize % (total - i));
        indices.swap(i, j);
        board[indices[i]].content = CellContent::Bomb;
    }
}

fn content_2d(board: &[Cell], dim: usize, x: isize, y: isize) -> Option<CellContent> {
    if x < 0 || y < 0 || x >= dim as isize || y >= dim as isize {
        return None;
    }
    Some(board[y as usize * dim + x as usize].content)
}

pub fn count_bombs_2d(board: &[Cell], dim: usize, x: usize, y: usize) -> u8 {
    let mut n: u8 = 0;
    for sy in y as isize - 1..=y as isize + 1 {
        for sx in x as isize - 1..=x as isize + 1 {
            if sx == x as isize && sy == y as isize {
                continue;
            }
            if content_2d(board, dim, sx, sy) == Some(CellContent::Bomb) {
                n += 1;
            }
        }
    }
    n
}

pub fn adjacent_2d(dim: usize, x: usize, y: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(8);
    for sy in y as isize - 1..=y as isize + 1 {
        for sx in x as isize - 1..=x as isize + 1 {
            if sx < 0 || sy < 0 || sx >= dim as isize || sy >= dim as isize {
                continue;
            }
            if sx == x as isize && sy == y as isize {
                continue;
            }
            out.push((sx as usize, sy as usize));
        }
    }
    out
}

/// True when no cell has been opened yet (first-click protection applies).
pub fn all_closed(board: &[Cell]) -> bool {
    board.iter().all(|c| c.state == CellState::Closed)
}

/// Win check: every non-bomb cell opened.
pub fn is_won(board: &[Cell]) -> bool {
    board
        .iter()
        .all(|c| c.state == CellState::Opened || c.content == CellContent::Bomb)
}

// ---------------------------------------------------------------------------
// 3D boards
// ---------------------------------------------------------------------------

pub fn bomb_amount_3d(dim: usize) -> usize {
    dim.pow(3).div_euclid(8).max(1)
}

fn content_3d(board: &[Cell], dim: usize, x: isize, y: isize, z: isize) -> Option<CellContent> {
    if x < 0 || y < 0 || z < 0 || x >= dim as isize || y >= dim as isize || z >= dim as isize {
        return None;
    }
    Some(board[(z as usize * dim + y as usize) * dim + x as usize].content)
}

pub fn count_bombs_3d(board: &[Cell], dim: usize, x: usize, y: usize, z: usize) -> u8 {
    let mut n: u8 = 0;
    for sz in z as isize - 1..=z as isize + 1 {
        for sy in y as isize - 1..=y as isize + 1 {
            for sx in x as isize - 1..=x as isize + 1 {
                if sx == x as isize && sy == y as isize && sz == z as isize {
                    continue;
                }
                if content_3d(board, dim, sx, sy, sz) == Some(CellContent::Bomb) {
                    n = n.saturating_add(1);
                }
            }
        }
    }
    n
}

pub fn generate_board_3d(
    dim: usize,
    bomb_amt: usize,
    origin: Pos3,
    rng: &mut TinyRng,
) -> Vec<Cell> {
    let mut board = vec![Cell::default(); dim * dim * dim];
    place_bombs_exact(&mut board, bomb_amt, rng);
    for z in 0..dim {
        for y in 0..dim {
            for x in 0..dim {
                let idx = (z * dim + y) * dim + x;
                // Same 3-block spacing as the old 3D mode.
                board[idx].pos = origin.offset(x as i32 * 3, z as i32 * 3, y as i32 * 3);
                if board[idx].content == CellContent::Bomb {
                    continue;
                }
                let n = count_bombs_3d(&board, dim, x, y, z);
                if n >= 1 {
                    board[idx].content = CellContent::Number(n);
                }
            }
        }
    }
    board
}

pub fn adjacent_3d(dim: usize, x: usize, y: usize, z: usize) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::with_capacity(26);
    for sz in z as isize - 1..=z as isize + 1 {
        for sy in y as isize - 1..=y as isize + 1 {
            for sx in x as isize - 1..=x as isize + 1 {
                if sx < 0 || sy < 0 || sz < 0 {
                    continue;
                }
                if sx >= dim as isize || sy >= dim as isize || sz >= dim as isize {
                    continue;
                }
                if sx == x as isize && sy == y as isize && sz == z as isize {
                    continue;
                }
                out.push((sx as usize, sy as usize, sz as usize));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Block palette (vanilla names for Pumpkin's set_block_by_name)
// ---------------------------------------------------------------------------

pub const COVERED_BLOCK: &str = "minecraft:moss_block";
/// Default flag block (player red). Per-player colors live in `state.rs`
/// (`flag_block_for`); this stays as the fallback/documented default.
#[allow(dead_code)]
pub const FLAG_BLOCK: &str = "minecraft:red_wool";
pub const AIR_BLOCK: &str = "minecraft:air";

/// Maps revealed content to a vanilla block, mirroring the old Valence
/// glazed-terracotta palette. Extended past 12 (3D boards can reach 26)
/// with concrete blocks instead of panicking like the old code did.
pub fn revealed_block_name(content: CellContent) -> &'static str {
    match content {
        CellContent::Empty => "minecraft:stone",
        CellContent::Bomb => "minecraft:tnt",
        CellContent::Number(n) => match n {
            1 => "minecraft:blue_glazed_terracotta",
            2 => "minecraft:green_glazed_terracotta",
            3 => "minecraft:red_glazed_terracotta",
            4 => "minecraft:black_glazed_terracotta",
            5 => "minecraft:orange_glazed_terracotta",
            6 => "minecraft:light_blue_glazed_terracotta",
            7 => "minecraft:purple_glazed_terracotta",
            8 => "minecraft:gray_glazed_terracotta",
            9 => "minecraft:white_glazed_terracotta",
            10 => "minecraft:light_gray_terracotta",
            11 => "minecraft:brown_glazed_terracotta",
            12 => "minecraft:yellow_glazed_terracotta",
            13 => "minecraft:white_concrete",
            14 => "minecraft:light_gray_concrete",
            15 => "minecraft:gray_concrete",
            16 => "minecraft:black_concrete",
            17 => "minecraft:brown_concrete",
            18 => "minecraft:red_concrete",
            19 => "minecraft:orange_concrete",
            20 => "minecraft:yellow_concrete",
            21 => "minecraft:lime_concrete",
            22 => "minecraft:green_concrete",
            23 => "minecraft:cyan_concrete",
            24 => "minecraft:light_blue_concrete",
            25 => "minecraft:blue_concrete",
            _ => "minecraft:purple_concrete",
        },
    }
}

// ---------------------------------------------------------------------------
// Infinite mode helpers (hash-based, no noise crate)
// ---------------------------------------------------------------------------

/// Bomb density ~19.5%, matching the old BOMB_THRESHOLD = 0.195.
pub fn infinite_is_bomb(x: i32, z: i32, seed: u64) -> bool {
    hash2(x, z, seed) % 1000 < 195
}

pub fn infinite_number_at(x: i32, z: i32, seed: u64) -> u8 {
    let mut n: u8 = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            if infinite_is_bomb(x + dx, z + dz, seed) {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_2d_has_exact_bombs_and_positions() {
        let mut rng = TinyRng::new(42);
        let dim = 20;
        let board = generate_board_2d(dim, bomb_amount_2d(dim), Pos3::new(10, 64, 10), &mut rng);
        assert_eq!(
            board
                .iter()
                .filter(|c| c.content == CellContent::Bomb)
                .count(),
            bomb_amount_2d(dim)
        );
        assert_eq!(board[0].pos, Pos3::new(10, 64, 10));
        assert_eq!(board[dim - 1].pos, Pos3::new(10 + 19, 64, 10));
    }

    #[test]
    fn numbers_match_neighbors() {
        let mut rng = TinyRng::new(1);
        let dim = 9;
        let board = generate_board_2d(dim, 10, Pos3::new(0, 64, 0), &mut rng);
        for y in 0..dim {
            for x in 0..dim {
                let c = &board[y * dim + x];
                match c.content {
                    CellContent::Number(n) => assert_eq!(n, count_bombs_2d(&board, dim, x, y)),
                    CellContent::Empty => assert_eq!(count_bombs_2d(&board, dim, x, y), 0),
                    CellContent::Bomb => {}
                }
            }
        }
    }

    #[test]
    fn board_3d_counts_fit_palette() {
        let mut rng = TinyRng::new(7);
        let dim = 5;
        let board = generate_board_3d(dim, bomb_amount_3d(dim), Pos3::new(0, 70, 0), &mut rng);
        for c in &board {
            // Must not panic: every content maps to a block name.
            let _ = revealed_block_name(c.content);
        }
    }

    #[test]
    fn infinite_density_sane() {
        let bombs = (0..100)
            .flat_map(|x| (0..100).map(move |z| infinite_is_bomb(x, z, 12345)))
            .filter(|b| *b)
            .count();
        assert!((1000..3000).contains(&bombs), "bombs={bombs}");
    }

    #[test]
    fn neighborhoods_exclude_self() {
        // 2D center: exactly 8 neighbors, never self.
        let center = adjacent_2d(5, 2, 2);
        assert_eq!(center.len(), 8);
        assert!(!center.contains(&(2, 2)));
        // Corner / edge clipping still holds.
        assert_eq!(adjacent_2d(5, 0, 0).len(), 3);
        assert_eq!(adjacent_2d(5, 1, 0).len(), 5);
        assert_eq!(adjacent_2d(2, 0, 0).len(), 3);
        // 3D center: exactly 26 neighbors, never self.
        let center3 = adjacent_3d(5, 2, 2, 2);
        assert_eq!(center3.len(), 26);
        assert!(!center3.contains(&(2, 2, 2)));
        assert_eq!(adjacent_3d(5, 0, 0, 0).len(), 7);
    }

    #[test]
    fn bomb_counts_exclude_center_cell() {
        // Lone bomb in a 3x3: orthogonal + diagonal neighbors see 1...
        let mut board = vec![Cell::default(); 9];
        board[1 * 3 + 1].content = CellContent::Bomb;
        assert_eq!(count_bombs_2d(&board, 3, 0, 0), 1);
        assert_eq!(count_bombs_2d(&board, 3, 1, 0), 1);
        // ...but the bomb cell itself counts 0 neighbors.
        assert_eq!(count_bombs_2d(&board, 3, 1, 1), 0);

        let mut board3 = vec![Cell::default(); 27];
        board3[(1 * 3 + 1) * 3 + 1].content = CellContent::Bomb;
        assert_eq!(count_bombs_3d(&board3, 3, 0, 0, 0), 1);
        assert_eq!(count_bombs_3d(&board3, 3, 1, 1, 1), 0);
    }

    #[test]
    fn generated_numbers_match_neighbor_counts() {
        // Property over several seeds/sizes: every Number cell equals a fresh
        // recount, so generation and counting can never drift apart.
        for (seed, dim) in [(11, 4), (12, 6), (13, 9)] {
            let mut rng = TinyRng::new(seed);
            let board = generate_board_2d(dim, bomb_amount_2d(dim), Pos3::new(0, 64, 0), &mut rng);
            for y in 0..dim {
                for x in 0..dim {
                    let c = &board[y * dim + x];
                    match c.content {
                        CellContent::Number(n) => assert_eq!(n, count_bombs_2d(&board, dim, x, y)),
                        CellContent::Empty => assert_eq!(count_bombs_2d(&board, dim, x, y), 0),
                        CellContent::Bomb => {}
                    }
                }
            }
        }
    }

    #[test]
    fn board_3d_positions_unique_and_spaced() {
        let mut rng = TinyRng::new(9);
        let dim = 4;
        let origin = Pos3::new(10, 64, -7);
        let board = generate_board_3d(dim, 5, origin, &mut rng);
        assert_eq!(board.len(), dim * dim * dim);
        let mut seen = std::collections::HashSet::new();
        for c in &board {
            assert_eq!((c.pos.x - origin.x) % 3, 0, "x not on 3-grid");
            assert_eq!((c.pos.y - origin.y) % 3, 0, "y not on 3-grid");
            assert_eq!((c.pos.z - origin.z) % 3, 0, "z not on 3-grid");
            assert!(seen.insert((c.pos.x, c.pos.y, c.pos.z)), "dup pos");
        }
    }

    #[test]
    fn win_and_closed_transitions() {
        let mut board = vec![Cell::default(); 4];
        board[0].content = CellContent::Bomb;
        assert!(all_closed(&board));
        assert!(!is_won(&board));
        // Flagging the bomb still counts as "not opened, but a bomb" -> win
        // once everything else is open.
        board[0].state = CellState::Flagged;
        for c in board.iter_mut().skip(1) {
            c.state = CellState::Opened;
        }
        assert!(!all_closed(&board));
        assert!(is_won(&board));
    }

    #[test]
    fn palette_covers_big_3d_numbers() {
        assert_eq!(revealed_block_name(CellContent::Empty), "minecraft:stone");
        assert_eq!(revealed_block_name(CellContent::Bomb), "minecraft:tnt");
        for n in 0..=30u8 {
            let name = revealed_block_name(CellContent::Number(n));
            assert!(!name.is_empty(), "no block for {n}");
        }
    }

    #[test]
    fn rng_and_hash_are_deterministic() {
        let mut a = TinyRng::new(1234);
        let mut b = TinyRng::new(1234);
        for _ in 0..5 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        // Seed 0 is remapped away from the degenerate state.
        let mut z = TinyRng::new(0);
        assert_ne!(z.next_u64(), 0);
        assert_eq!(hash2(5, -7, 99), hash2(5, -7, 99));
        assert_ne!(hash2(1, 0, 99), hash2(0, 1, 99));
    }

    #[test]
    fn infinite_numbers_match_manual_neighbor_count() {
        for x in -3..=3 {
            for z in -3..=3 {
                let mut manual: u8 = 0;
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dz == 0 {
                            continue;
                        }
                        manual += infinite_is_bomb(x + dx, z + dz, 777) as u8;
                    }
                }
                assert_eq!(infinite_number_at(x, z, 777), manual, "at {x},{z}");
            }
        }
    }
}
