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
    generate_board_2d_avoiding(dim, bomb_amt, origin, rng, &[])
}

/// Like [`generate_board_2d`], but the cell at `safe` and its neighbours can
/// never hold a bomb, so a first click there is guaranteed to be `Empty`.
/// Tiny boards (2x2) have no zero-cell at all; there the bomb count wins and
/// the guarantee degrades to "never a bomb".
pub fn generate_board_2d_safe(
    dim: usize,
    bomb_amt: usize,
    origin: Pos3,
    rng: &mut TinyRng,
    safe: usize,
) -> Vec<Cell> {
    let mut avoid = avoid_for_2d(dim, safe);
    if dim * dim - avoid.len() < bomb_amt {
        avoid = vec![safe];
    }
    generate_board_2d_avoiding(dim, bomb_amt, origin, rng, &avoid)
}

fn generate_board_2d_avoiding(
    dim: usize,
    bomb_amt: usize,
    origin: Pos3,
    rng: &mut TinyRng,
    avoid: &[usize],
) -> Vec<Cell> {
    let mut board = vec![Cell::default(); dim * dim];
    place_bombs_exact(&mut board, bomb_amt, rng, avoid);
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

/// Cells that must stay bomb-free for a safe first click: the cell itself
/// plus every neighbour.
fn avoid_for_2d(dim: usize, safe: usize) -> Vec<usize> {
    let (sx, sy) = (safe % dim, safe / dim);
    let mut avoid = vec![safe];
    avoid.extend(adjacent_2d(dim, sx, sy).into_iter().map(|(x, y)| y * dim + x));
    avoid
}

/// Place exactly `bomb_amt` bombs at distinct random cells (Fisher-Yates),
/// never on an index in `avoid`.
fn place_bombs_exact(board: &mut [Cell], bomb_amt: usize, rng: &mut TinyRng, avoid: &[usize]) {
    let mut indices: Vec<usize> = (0..board.len()).filter(|i| !avoid.contains(i)).collect();
    let bomb_amt = bomb_amt.min(indices.len());
    for i in 0..bomb_amt {
        let j = i + (rng.next_u64() as usize % (indices.len() - i));
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
    generate_board_3d_avoiding(dim, bomb_amt, origin, rng, &[])
}

/// Like [`generate_board_3d`], but the cell at `safe` and its 26 neighbours
/// can never hold a bomb, so a first click there is guaranteed to be `Empty`.
/// 2x2x2 has no zero-cell at all; there the bomb count wins and the guarantee
/// degrades to "never a bomb".
pub fn generate_board_3d_safe(
    dim: usize,
    bomb_amt: usize,
    origin: Pos3,
    rng: &mut TinyRng,
    safe: usize,
) -> Vec<Cell> {
    let mut avoid = avoid_for_3d(dim, safe);
    if dim * dim * dim - avoid.len() < bomb_amt {
        avoid = vec![safe];
    }
    generate_board_3d_avoiding(dim, bomb_amt, origin, rng, &avoid)
}

fn generate_board_3d_avoiding(
    dim: usize,
    bomb_amt: usize,
    origin: Pos3,
    rng: &mut TinyRng,
    avoid: &[usize],
) -> Vec<Cell> {
    let mut board = vec![Cell::default(); dim * dim * dim];
    place_bombs_exact(&mut board, bomb_amt, rng, avoid);
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

/// Cells that must stay bomb-free for a safe first click: the cell itself
/// plus every neighbour.
fn avoid_for_3d(dim: usize, safe: usize) -> Vec<usize> {
    let (x, y, z) = (safe % dim, (safe / dim) % dim, safe / (dim * dim));
    let mut avoid = vec![safe];
    avoid.extend(
        adjacent_3d(dim, x, y, z)
            .into_iter()
            .map(|(x, y, z)| (z * dim + y) * dim + x),
    );
    avoid
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

/// Maps revealed content to a vanilla block plus an optional `facing`
/// property value for Java-RP's minesweeper tile art.
///
/// Java-RP retextures cyan/lime/magenta glazed terracotta so every facing
/// renders a different tile (digits 1..=12). Numbers 13+ fall back to
/// concrete colors like before (3D boards can reach 26; those digits have
/// no tile art and are practically unreachable).
pub fn revealed_tile(content: CellContent) -> (&'static str, Option<&'static str>) {
    match content {
        CellContent::Empty => ("minecraft:stone", None),
        CellContent::Bomb => ("minecraft:tnt", None),
        CellContent::Number(n) => match n {
            1 => ("minecraft:cyan_glazed_terracotta", Some("east")),
            2 => ("minecraft:cyan_glazed_terracotta", Some("north")),
            3 => ("minecraft:cyan_glazed_terracotta", Some("south")),
            4 => ("minecraft:cyan_glazed_terracotta", Some("west")),
            5 => ("minecraft:lime_glazed_terracotta", Some("east")),
            6 => ("minecraft:lime_glazed_terracotta", Some("north")),
            7 => ("minecraft:lime_glazed_terracotta", Some("south")),
            8 => ("minecraft:lime_glazed_terracotta", Some("west")),
            9 => ("minecraft:magenta_glazed_terracotta", Some("east")),
            10 => ("minecraft:magenta_glazed_terracotta", Some("north")),
            11 => ("minecraft:magenta_glazed_terracotta", Some("south")),
            12 => ("minecraft:magenta_glazed_terracotta", Some("west")),
            13 => ("minecraft:white_concrete", None),
            14 => ("minecraft:light_gray_concrete", None),
            15 => ("minecraft:gray_concrete", None),
            16 => ("minecraft:black_concrete", None),
            17 => ("minecraft:brown_concrete", None),
            18 => ("minecraft:red_concrete", None),
            19 => ("minecraft:orange_concrete", None),
            20 => ("minecraft:yellow_concrete", None),
            21 => ("minecraft:lime_concrete", None),
            22 => ("minecraft:green_concrete", None),
            23 => ("minecraft:cyan_concrete", None),
            24 => ("minecraft:light_blue_concrete", None),
            25 => ("minecraft:blue_concrete", None),
            _ => ("minecraft:purple_concrete", None),
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
            // Must not panic: every content maps to a block (and facing).
            let _ = revealed_tile(c.content);
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
        assert!(board.iter().all(|c| c.state == CellState::Closed));
        assert!(!is_won(&board));
        // Flagging the bomb still counts as "not opened, but a bomb" -> win
        // once everything else is open.
        board[0].state = CellState::Flagged;
        for c in board.iter_mut().skip(1) {
            c.state = CellState::Opened;
        }
        assert!(board.iter().any(|c| c.state != CellState::Closed));
        assert!(is_won(&board));
    }

    #[test]
    fn palette_covers_big_3d_numbers() {
        assert_eq!(revealed_tile(CellContent::Empty), ("minecraft:stone", None));
        assert_eq!(revealed_tile(CellContent::Bomb), ("minecraft:tnt", None));
        for n in 0..=30u8 {
            let (name, facing) = revealed_tile(CellContent::Number(n));
            assert!(!name.is_empty(), "no block for {n}");
            if (1..=12).contains(&n) {
                assert!(facing.is_some(), "tile {n} must carry a facing");
            } else {
                assert!(facing.is_none(), "tile {n} must not carry a facing");
            }
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

    #[test]
    fn safe_generators_never_start_on_a_bomb() {
        for dim in [2usize, 3, 5, 7] {
            for safe in 0..dim * dim * dim {
                let mut rng = TinyRng::new(7);
                let board = generate_board_3d_safe(
                    dim,
                    bomb_amount_3d(dim),
                    Pos3::new(0, 64, 0),
                    &mut rng,
                    safe,
                );
                assert_ne!(
                    board[safe].content,
                    CellContent::Bomb,
                    "3D dim={dim} safe={safe}"
                );
                assert_eq!(
                    board.iter().filter(|c| c.content == CellContent::Bomb).count(),
                    bomb_amount_3d(dim),
                    "3D dim={dim} bomb count changed"
                );
            }
        }
        for dim in [2usize, 3, 10, 20] {
            for safe in 0..dim * dim {
                let mut rng = TinyRng::new(11);
                let board = generate_board_2d_safe(
                    dim,
                    bomb_amount_2d(dim),
                    Pos3::new(0, 64, 0),
                    &mut rng,
                    safe,
                );
                assert_ne!(
                    board[safe].content,
                    CellContent::Bomb,
                    "2D dim={dim} safe={safe}"
                );
                assert_eq!(
                    board.iter().filter(|c| c.content == CellContent::Bomb).count(),
                    bomb_amount_2d(dim),
                    "2D dim={dim} bomb count changed"
                );
            }
        }
    }

    #[test]
    fn safe_generators_guarantee_empty_when_mathematically_possible() {
        // An Empty start needs every bomb outside the clicked cell plus its
        // neighbourhood; skip indices where that is impossible (tiny boards).
        for dim in [5usize, 7] {
            let amt = bomb_amount_3d(dim);
            for safe in 0..dim * dim * dim {
                let (x, y, z) = (safe % dim, (safe / dim) % dim, safe / (dim * dim));
                if dim.pow(3) - 1 - adjacent_3d(dim, x, y, z).len() < amt {
                    continue;
                }
                let mut rng = TinyRng::new(safe as u64 + 1);
                let board =
                    generate_board_3d_safe(dim, amt, Pos3::new(0, 64, 0), &mut rng, safe);
                assert_eq!(
                    board[safe].content,
                    CellContent::Empty,
                    "3D dim={dim} safe={safe}"
                );
            }
        }
        for dim in [10usize, 20] {
            let amt = bomb_amount_2d(dim);
            for safe in 0..dim * dim {
                let (x, y) = (safe % dim, safe / dim);
                if dim.pow(2) - 1 - adjacent_2d(dim, x, y).len() < amt {
                    continue;
                }
                let mut rng = TinyRng::new(safe as u64 + 1);
                let board =
                    generate_board_2d_safe(dim, amt, Pos3::new(0, 64, 0), &mut rng, safe);
                assert_eq!(
                    board[safe].content,
                    CellContent::Empty,
                    "2D dim={dim} safe={safe}"
                );
            }
        }
    }
}
