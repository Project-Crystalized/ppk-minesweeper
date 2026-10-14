use std::{
    collections::HashMap,
    sync::{
        mpsc::{Receiver, Sender},
        Mutex,
    },
};

use minesweeper_lib::*;
use noise::OpenSimplex;
use valence::scoreboard::*;
use valence::{interact_block::InteractBlockEvent, prelude::*, protocol::Sound};

pub type BoardChunk = [[(CellContent, CellState); 16]; 16];

#[derive(Component)]
pub struct PlayerBoard {
    pub simplex: OpenSimplex,
    pub board: HashMap<ChunkPos, BoardChunk>,
    pub sender: Sender<(Entity, OpenSimplex, ChunkPos, Option<BoardChunk>)>,
}

#[derive(Resource)]
pub struct MapState {
    pub receiver: Mutex<Receiver<(ChunkPos, Entity, UnloadedChunk, BoardChunk)>>,
    pub clonable_sender: Sender<(Entity, OpenSimplex, ChunkPos, Option<BoardChunk>)>,
}

pub struct ChunkGenerator {
    pub sender: Sender<(ChunkPos, Entity, UnloadedChunk, BoardChunk)>,
    pub receiver: Receiver<(Entity, OpenSimplex, ChunkPos, Option<BoardChunk>)>,
}

#[derive(Event)]
pub struct ClickedCellEvent {
    entity: Entity,
    is_bomb: bool,
}

#[derive(Event)]
pub struct PlacedFlagEvent(Entity);

#[derive(Component, Default)]
pub struct PlayerSession {
    pub top_streak: usize,
    pub current_streak: usize,
    pub total_fields_opened: usize,
    pub total_bombs_clicked: usize,
}

pub fn show_session_scorebrd(
    players: Query<(&PlayerSession, &VisibleEntityLayers)>,
    mut objectives: Query<(&mut ObjectiveScores, &EntityLayerId)>,
    server: Res<Server>,
) {
    if server.current_tick() % 5 == 0 {
        for (p_session, p_layers) in players.iter() {
            let mut obj = objectives
                .iter_mut()
                .find(|(_, &layerid)| p_layers.0.iter().any(|&l| l == *layerid))
                .unwrap();
            obj.0.insert("Top Streak", p_session.top_streak as i32);
            obj.0
                .insert("Current Streak", p_session.current_streak as i32);
            obj.0
                .insert("Total Fields", p_session.total_fields_opened as i32);
            obj.0
                .insert("Total Bombs", p_session.total_bombs_clicked as i32);
        }
    }
}

pub fn update_session_stats(
    mut players: Query<&mut PlayerSession>,
    mut cell_clicks: EventReader<ClickedCellEvent>,
) {
    for cell_click in cell_clicks.read() {
        let mut session = players.get_mut(cell_click.entity).unwrap();
        if cell_click.is_bomb {
            session.current_streak = 0;
            session.total_bombs_clicked += 1;
        } else {
            session.total_fields_opened += 1;
            session.current_streak += 1;
            session.top_streak = session.top_streak.max(session.current_streak);
        }
    }
}

pub fn show_action_bar(
    mut players: Query<(&mut Client, &PlayerSession)>,
    mut flag_places: EventReader<PlacedFlagEvent>,
    mut cell_clicks: EventReader<ClickedCellEvent>,
) {
    let mut get_bar = |entity| {
        let (mut client, session) = players.get_mut(entity).unwrap();
        let color = match session.current_streak {
            0..=50 => Color::GRAY,
            0..=150 => Color::GREEN,
            0..=300 => Color::BLUE,
            0..=500 => Color::DARK_BLUE,
            0..=1000 => Color::RED,
            0..=2000 => Color::DARK_RED,
            _ => Color::GOLD,
        };
        let mut text: Text = format!("Current: Streak: {}", session.current_streak).color(color);
        if session.current_streak > 1000 {
            text = "a ".obfuscated() + text.not_obfuscated() + " a".obfuscated();
        }
        client.set_action_bar(text);
    };
    for flag_place in flag_places.read() {
        get_bar(flag_place.0);
    }
    for cell_click in cell_clicks.read() {
        get_bar(cell_click.entity);
    }
}

pub fn broadcast_msgs(
    mut players: Query<(&mut Client, &PlayerSession, &Username)>,
    mut clicks: EventReader<ClickedCellEvent>,
) {
    let mut msgs = vec![];
    for click in clicks.read() {
        let (_, session, username) = players.get_mut(click.entity).unwrap();
        if session.current_streak == 1000 {
            msgs.push((username.0.clone(), 1000));
        }
        if session.current_streak == 500 {
            msgs.push((username.0.clone(), 500));
        }
    }
    for msg in msgs {
        players.for_each_mut(|mut c| {
            c.0.send_chat_message(
                format!("{} has reached a streak of {}.", msg.0, msg.1).color(Color::RED),
            )
        });
    }
}

pub fn left_click(
    mut layers: Query<&mut ChunkLayer>,
    mut clicks: ResMut<Events<DiggingEvent>>,
    mut cell_click: EventWriter<ClickedCellEvent>,
    mut players: Query<(&mut PlayerBoard, &VisibleChunkLayer, Entity, &mut Client)>,
    mut locks: Local<Vec<(BlockPos, u8)>>,
) {
    let mut r = clicks.get_reader();
    let mut click_writer = vec![];
    handle_locks_tick(locks.as_mut());
    for click in r.read(&clicks) {
        let (mut player, layer_id, entity, mut client) = players.get_mut(click.client).unwrap();
        let mut layer = layers.get_mut(layer_id.0).unwrap();

        if check_locks(locks.as_mut(), click.position) {
            continue;
        }

        let (cpos, x, z) = get_poss(click.position);
        let (z, x) = (z as usize, x as usize);

        let board = if let Some(board) = player.board.get_mut(&cpos) {
            board
        } else {
            tracing::error!("clicked a block in a non existant chunk");
            continue;
        };

        match board[z][x].1 {
            CellState::Closed => match board[z][x].0 {
                CellContent::Bomb => {
                    cell_click.send(ClickedCellEvent {
                        entity,
                        is_bomb: true,
                    });
                    board[z][x].1 = CellState::Opened;
                    sound(&mut layer, Sound::EntityGenericExplode, &click.position);
                    layer.set_block(click.position, get_num_color(board[z][x].0));
                    client.send_chat_message("You Clicked a bomb, your streak has reset!");
                    tracing::debug!("game over by clicking a bomb");
                    return;
                }
                CellContent::Empty => {
                    cell_click.send(ClickedCellEvent {
                        entity,
                        is_bomb: false,
                    });
                    sound(&mut layer, Sound::EntityFrogStep, &click.position);
                    board[z][x].1 = CellState::Opened;
                    let block = get_num_color(board[z][x].0);
                    layer.set_block(click.position, block);
                    for x in click.position.x - 1..=click.position.x + 1 {
                        for z in click.position.z - 1..=click.position.z + 1 {
                            // simulate click on adjacent empty fields
                            click_writer.push(DiggingEvent {
                                client: click.client,
                                position: BlockPos::from((x, -49, z)),
                                direction: Direction::Up,
                                state: DiggingState::Start,
                            });
                        }
                    }
                }
                CellContent::Number(_) => {
                    cell_click.send(ClickedCellEvent {
                        entity,
                        is_bomb: false,
                    });
                    sound(&mut layer, Sound::EntityFrogStep, &click.position);
                    board[z][x].1 = CellState::Opened;
                    let b = layer.set_block(click.position, get_num_color(board[z][x].0));
                    if !(b.clone().is_some_and(|b| b.state == BlockState::MOSS_BLOCK)) {
                        tracing::error!(
                            "error: clicking a minesweeper field, replaced block: {:?}",
                            b.map(|b| b.state)
                        );
                    }
                }
            },
            _ => (),
        }
    }
    click_writer.iter().for_each(|c| clicks.send(*c));
}

pub fn right_click(
    mut layers: Query<&mut ChunkLayer>,
    mut clicks: EventReader<InteractBlockEvent>,
    mut players: Query<(&mut PlayerBoard, &VisibleChunkLayer, Entity)>,
    mut place_events: EventWriter<PlacedFlagEvent>,
    mut locks: Local<Vec<(BlockPos, u8)>>,
) {
    handle_locks_tick(locks.as_mut());
    for click in clicks.read() {
        let (mut player, layer_id, entity) = players.get_mut(click.client).unwrap();
        let mut layer = layers.get_mut(layer_id.0).unwrap();

        if check_locks(locks.as_mut(), click.position) {
            continue;
        }

        let (cpos, x, z) = get_poss(click.position);
        let (z, x) = (z as usize, x as usize);

        let board = if let Some(board) = player.board.get_mut(&cpos) {
            board
        } else {
            tracing::error!("clicked a block in a non existant chunk");
            continue;
        };

        match board[z][x].1 {
            CellState::Closed => {
                layer.set_block(click.position, BlockState::RED_WOOL);
                board[z][x].1 = CellState::Flagged;
                sound(&mut layer, Sound::BlockWoolPlace, &click.position);
                place_events.send(PlacedFlagEvent(entity));
            }
            CellState::Flagged => {
                layer.set_block(click.position, BlockState::MOSS_BLOCK);
                board[z][x].1 = CellState::Closed;
                sound(&mut layer, Sound::BlockWoolBreak, &click.position);
            }
            _ => (),
        }
    }
}

fn handle_locks_tick(locks: &mut Vec<(BlockPos, u8)>) {
    locks.retain(|(_, i)| *i > 0);
    locks.iter_mut().for_each(|(_, timer)| {
        *timer -= 1;
    });
}

fn check_locks(locks: &mut Vec<(BlockPos, u8)>, click_pos: BlockPos) -> bool {
    if locks
        .iter_mut()
        .find(|(pos, _)| *pos == click_pos)
        .is_some()
    {
        return true;
    }
    locks.push((click_pos, 2));
    return false;
}

fn get_poss(pos: BlockPos) -> (ChunkPos, u8, u8) {
    let cpos = ChunkPos::new(pos.x.div_euclid(16), pos.z.div_euclid(16));
    for x in 0..16_u8 {
        for z in 0..16_u8 {
            if x as i32 + cpos.x * 16 == pos.x && z as i32 + cpos.z * 16 == pos.z {
                return (cpos, x, z);
            }
        }
    }
    unreachable!("idk how this could happen lol");
}
