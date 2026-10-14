use itertools::Itertools;
use minesweeper_lib::*;
use rand::Rng;
use valence::{prelude::*, protocol::Sound};

pub mod items;
use mampfer_plugin::PostgresWrapper;

use crate::BORDER_DISTANCE;

pub struct MineSweeperGame2d<const DIM: usize> {
    board: [[Cell; DIM]; DIM],
    player: (Entity, UniqueId),
    is_build: bool,
    should_despawn: bool,
    is_over: bool,
    is_won: bool,
    flag_lock: u8,
    comp_time: usize,
    players: Vec<UniqueId>,
}

impl<const DIM: usize> MineSweeperGame2d<DIM> {
    const BOMB_AMT: usize = DIM.pow(2).div_euclid(6);
    pub fn new(pos: &Position, player: (Entity, UniqueId)) -> MineSweeperGame2d<DIM> {
        MineSweeperGame2d {
            board: Self::generate_board(Self::BOMB_AMT, BlockPos::from(**pos)),
            player,
            is_build: false,
            should_despawn: false,
            is_over: false,
            is_won: false,
            flag_lock: 0,
            comp_time: 0,
            players: vec![],
        }
    }
    fn generate_board(bomb_amt: usize, pos: BlockPos) -> [[Cell; DIM]; DIM] {
        let mut base = [[Cell::default(); DIM]; DIM];
        let mut rng = rand::thread_rng();
        // place all bombs
        while Self::count_bombs_total(&base) < bomb_amt {
            for cell in base.flatten_mut() {
                if rng.gen_ratio(1, 100) {
                    cell.content = CellContent::Bomb;
                }
            }
        }
        // fill in numbers and positions
        for (x, y) in (0..DIM).cartesian_product(0..DIM) {
            base[y][x].pos = pos.offset(x as i32, 0, y as i32);
            if base[y][x].content == CellContent::Bomb {
                continue;
            } else if MineSweeperGame2d::<DIM>::count_bombs(&base, (x, y)) >= 1 {
                base[y][x].content =
                    CellContent::Number(MineSweeperGame2d::<DIM>::count_bombs(&base, (x, y)));
            }
        }
        return base;
    }

    fn count_bombs(board: &[[Cell; DIM]; DIM], (x, y): (usize, usize)) -> u8 {
        let mut bomb_count: u8 = 0;
        for scan_x in x as isize - 1..=x as isize + 1 {
            for scan_y in y as isize - 1..=y as isize + 1 {
                if board.get(scan_y as usize).is_some_and(|y| {
                    y.get(scan_x as usize)
                        .is_some_and(|c| c.content == CellContent::Bomb)
                }) {
                    bomb_count += 1;
                }
            }
        }
        return bomb_count;
    }

    fn count_bombs_total(board: &[[Cell; DIM]; DIM]) -> usize {
        board
            .flatten()
            .iter()
            .filter(|c| c.content == CellContent::Bomb)
            .count()
    }

    fn get_adjacent_cells(&self, (x, y): (usize, usize)) -> Vec<(usize, usize)> {
        let mut array: Vec<(usize, usize)> = vec![];
        for scan_x in x as isize - 1..=x as isize + 1 {
            for scan_y in y as isize - 1..=y as isize + 1 {
                if self
                    .board
                    .get(scan_y as usize)
                    .is_some_and(|y| y.get(scan_x as usize).is_some())
                {
                    array.push((scan_x as usize, scan_y as usize));
                }
            }
        }
        return array;
    }

    fn save_helpers(&mut self, player: (Entity, UniqueId)) {
        if !self.players.contains(&player.1) {
            self.players.push(player.1);
        }
    }

    fn regenerate_if_not_empty(&mut self, (x, y): (usize, usize)) -> bool {
        if self
            .board
            .flatten()
            .iter()
            .all(|e| e.state == CellState::Closed)
        {
            tracing::debug!(
                "minesweeper: a bomb or number was the first clicked cell. Generating new Board."
            );
            while self.board[y][x].content != CellContent::Empty {
                self.board = Self::generate_board(Self::BOMB_AMT, self.board[0][0].pos);
            }
            return true;
        }
        return false;
    }
    fn get_corners(&self) -> [BlockPos; 4] {
        let mut array = [BlockPos::new(0, 0, 0); 4];
        array[0] = self.board[0][0].pos;
        array[1] = self.board[0][DIM - 1].pos;
        array[2] = self.board[DIM - 1][0].pos;
        array[3] = self.board[DIM - 1][DIM - 1].pos;
        return array;
    }
}

impl<const DIM: usize> MinesweeperGame for MineSweeperGame2d<DIM> {
    fn build_blocks(&mut self, layer: &mut ChunkLayer) -> Result<(), BuildGameError> {
        if (0..DIM).cartesian_product(0..DIM).any(|(x, y)| {
            layer
                .block(self.board[y][x].pos)
                .is_some_and(|b| !b.state.is_air())
        }) {
            return Err(BuildGameError::BlocksInTheWay);
        }
        if self.get_corners().iter().any(|block_pos| {
            all_directions().any(|dir| {
                let pos1 = block_pos.get_in_direction(dir);
                let pos2 = block_pos.get_in_direction(dir).get_in_direction(dir);
                let pos3 = block_pos.get_in_direction(dir).get_in_direction(dir);
                layer.block(pos1).is_some_and(|b| !b.state.is_air())
                    || layer.block(pos2).is_some_and(|b| !b.state.is_air())
                    || layer.block(pos3).is_some_and(|b| !b.state.is_air())
            })
        }) {
            return Err(BuildGameError::BlocksTooClose);
        }
        if self.get_corners().iter().any(|&BlockPos { x, y, z }| {
            let d = BORDER_DISTANCE as i32 / 2;
            x > d || x < -d || z > d || z < -d || y > 300 || y < -70
        }) {
            return Err(BuildGameError::InBorder);
        }
        // check outside border
        for (x, y) in (0..DIM).cartesian_product(0..DIM) {
            let block = BlockState::MOSS_BLOCK;
            // let block = get_num_color(self.board[y][x].content);
            layer.set_block(self.board[y][x].pos, block);
        }
        self.is_build = true;
        return Ok(());
    }
    fn tick(&mut self, _layer: &mut ChunkLayer) {
        if !self.is_over {
            self.comp_time += 1;
        }
        if self.flag_lock > 0 {
            self.flag_lock -= 1;
        }
    }
    fn click_right(
        &mut self,
        click_pos: &BlockPos,
        _player: Entity,
        layer: &mut ChunkLayer,
        flag: BlockState,
    ) {
        if self.flag_lock != 0 || self.is_over {
            return;
        }
        for (x, y) in (0..DIM).cartesian_product(0..DIM) {
            let pos = self.board[y][x].pos;
            if pos == *click_pos {
                self.flag_lock = 2;
                match self.board[y][x].state {
                    CellState::Closed => {
                        layer.set_block(pos, flag);
                        self.board[y][x].state = CellState::Flagged;
                        sound(layer, Sound::BlockWoolPlace, click_pos);
                    }
                    CellState::Flagged => {
                        layer.set_block(pos, BlockState::MOSS_BLOCK);
                        self.board[y][x].state = CellState::Closed;
                        sound(layer, Sound::BlockWoolBreak, click_pos);
                    }
                    _ => (),
                }
            }
        }
    }
    fn click_left(
        &mut self,
        click_pos: &BlockPos,
        player: (Entity, UniqueId),
        layer: &mut ChunkLayer,
        click_writer: &mut Vec<DiggingEvent>,
    ) {
        if self.is_over && self.flag_lock < 1 && player == self.player {
            self.should_despawn = true;
            return;
        }
        for (x, y) in (0..DIM).cartesian_product(0..DIM) {
            if self.board[y][x].pos == *click_pos {
                match self.board[y][x].state {
                    CellState::Closed => match self.board[y][x].content {
                        CellContent::Bomb => {
                            if self.regenerate_if_not_empty((x, y)) {
                                self.click_left(click_pos, player, layer, click_writer);
                                return;
                            }
                            sound(layer, Sound::EntityGenericExplode, click_pos);
                            for x in 0..DIM {
                                for y in 0..DIM {
                                    let block = get_num_color(self.board[y][x].content);
                                    layer.set_block(self.board[y][x].pos, block);
                                }
                            }
                            tracing::debug!("game over by clicking a bomb");
                            self.is_over = true;
                            self.flag_lock = 2;
                            return;
                        }
                        CellContent::Empty => {
                            sound(layer, Sound::EntityFrogStep, click_pos);
                            self.board[y][x].state = CellState::Opened;
                            let block = get_num_color(self.board[y][x].content);
                            layer.set_block(self.board[y][x].pos, block);
                            for adj in self.get_adjacent_cells((x, y)) {
                                let cell = self.board[adj.1][adj.0];
                                if cell.state != CellState::Opened {
                                    self.board[adj.1][adj.0].state = CellState::Closed;
                                    // simulate click on adjacent empty fields
                                    click_writer.push(DiggingEvent {
                                        client: player.0,
                                        position: cell.pos,
                                        direction: Direction::Up,
                                        state: DiggingState::Start,
                                    })
                                }
                            }
                            self.save_helpers(player);
                        }
                        CellContent::Number(_) => {
                            if self.regenerate_if_not_empty((x, y)) {
                                self.click_left(click_pos, player, layer, click_writer);
                                return;
                            }
                            sound(layer, Sound::EntityFrogStep, click_pos);
                            self.board[y][x].state = CellState::Opened;
                            let b = layer
                                .set_block(*click_pos, get_num_color(self.board[y][x].content));
                            if !(b.clone().is_some_and(|b| b.state == BlockState::MOSS_BLOCK)) {
                                tracing::error!(
                                    "error: clicking a minesweeper field, replaced block: {:?}",
                                    b.map(|b| b.state)
                                );
                            }
                            self.save_helpers(player);
                        }
                    },
                    _ => (),
                }
                if !self.is_over
                    && (0..DIM).cartesian_product(0..DIM).all(|(x, y)| {
                        self.board[y][x].state == CellState::Opened
                            || self.board[y][x].content == CellContent::Bomb
                    })
                {
                    sound(
                        layer,
                        Sound::ItemGoatHornSound1,
                        &self.board[DIM / 2][DIM / 2].pos,
                    );
                    self.flag_lock = 2;
                    self.is_over = true;
                    self.is_won = true;
                }
            }
        }
    }
    fn reset(&self, layer: &mut ChunkLayer, pgsql: &mut PostgresWrapper) {
        for x in 0..DIM {
            for y in 0..DIM {
                layer.set_block(self.board[y][x].pos, BlockState::AIR);
            }
        }

        tracing::debug!("saving entry to database");
        if self.is_won {
            pgsql.insert_minesweeper(DIM as i32, 2, self.comp_time as i32, &self.players);
        }
    }
    fn get_player(&self) -> (Entity, UniqueId) {
        self.player
    }
    fn should_despawn(&self) -> bool {
        self.should_despawn
    }
    fn get_action_state(&self) -> ActionState {
        let p = self.board[DIM / 2][DIM / 2].pos;
        let center = Position(DVec3::new(p.x as f64, p.y as f64, p.z as f64));
        let flags: usize = self
            .board
            .flatten()
            .iter()
            .filter(|e| e.state == CellState::Flagged)
            .count();
        ActionState {
            center,
            bombs: Self::count_bombs_total(&self.board),
            timer: self.comp_time,
            flags,
        }
    }
}
