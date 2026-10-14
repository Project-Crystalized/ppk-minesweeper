use valence::{
    prelude::*,
    protocol::{sound::SoundCategory, Sound},
};

#[derive(PartialEq, Copy, Clone, Debug, Default)]
pub enum CellContent {
    #[default]
    Empty,
    Bomb,
    Number(u8),
}

#[derive(PartialEq, Copy, Clone, Default)]
pub enum CellState {
    Opened,
    #[default]
    Closed,
    Flagged,
}

#[derive(Default, Copy, Clone, PartialEq)]
pub struct Cell {
    pub content: CellContent,
    pub state: CellState,
    pub pos: BlockPos,
}

pub struct ActionState {
    pub center: Position,
    pub timer: usize,
    pub bombs: usize,
    pub flags: usize,
}

impl ActionState {
    pub fn to_text(&self) -> Text {
        "A ".obfuscated()
            + format!(" Time: {}    ", self.timer / 20)
                .color(Color::AQUA)
                .not_obfuscated()
            + format!("Bombs: {}    ", self.bombs)
                .color(Color::RED)
                .not_obfuscated()
            + format!("Flags: {}    ", self.flags)
                .color(Color::GOLD)
                .not_obfuscated()
            + " A".obfuscated()
    }
}

pub fn sound(layer: &mut ChunkLayer, sound: Sound, loc: &BlockPos) {
    layer.play_sound(
        sound,
        SoundCategory::Ambient,
        DVec3::new(loc.x.into(), loc.y.into(), loc.z.into()),
        20.0,
        1.0,
    );
}

pub fn all_directions() -> impl Iterator<Item = Direction> {
    [
        Direction::Up,
        Direction::Down,
        Direction::West,
        Direction::East,
        Direction::North,
        Direction::South,
    ]
    .into_iter()
}

pub fn get_num_color(cell: CellContent) -> BlockState {
    match cell {
        CellContent::Empty => BlockState::STONE,
        CellContent::Bomb => BlockState::TNT,
        CellContent::Number(n) => match n {
            1 => BlockState::BLUE_GLAZED_TERRACOTTA,
            2 => BlockState::GREEN_GLAZED_TERRACOTTA,
            3 => BlockState::RED_GLAZED_TERRACOTTA,
            4 => BlockState::BLACK_GLAZED_TERRACOTTA,
            5 => BlockState::ORANGE_GLAZED_TERRACOTTA,
            6 => BlockState::LIGHT_BLUE_GLAZED_TERRACOTTA,
            7 => BlockState::PURPLE_GLAZED_TERRACOTTA,
            8 => BlockState::GRAY_GLAZED_TERRACOTTA,
            9 => BlockState::WHITE_GLAZED_TERRACOTTA,
            10 => BlockState::LIGHT_GRAY_TERRACOTTA,
            11 => BlockState::BROWN_GLAZED_TERRACOTTA,
            12 => BlockState::YELLOW_GLAZED_TERRACOTTA,
            _ => {
                tracing::error!("unknown number of bombs: {}", n);
                unimplemented!();
            }
        },
    }
}
