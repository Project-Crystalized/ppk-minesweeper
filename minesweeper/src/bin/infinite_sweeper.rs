#![feature(slice_flatten)]
use std::{
    collections::HashMap,
    sync::{mpsc, Mutex},
    thread,
};

use itertools::Itertools;
use mampfer_plugin::PostgresWrapper;
use minesweeper_lib::*;
use noise::{NoiseFn, OpenSimplex};
use rand::{thread_rng, Rng};
use tracing::Level;
use valence::{log::LogPlugin, scoreboard::*};
use valence::{prelude::*, world_border::WorldBorderBundle};
mod infinitms;
use infinitms::*;

const SPAWN_POS: DVec3 = DVec3::new(0.0, 10.0, 0.0);
const BOMB_THRESHOLD: f64 = 0.195;

fn main() {
    App::new()
        .insert_resource(NetworkSettings {
            connection_mode: ConnectionMode::Velocity {
                secret: "temporärsecret".into(),
            },
            address: "0.0.0.0:25571".parse().unwrap(),
            ..Default::default()
        })
        .add_systems(Startup, setup)
        .add_event::<ClickedCellEvent>()
        .add_event::<PlacedFlagEvent>()
        .add_systems(
            Update,
            (
                (init_clients, send_chunks_to_generator, chunk_receiver).chain(),
                lower_render_distance,
                right_click,
                left_click,
                despawn_disconnected_clients,
                optimize_memory,
                broadcast_msgs,
                (update_session_stats, show_action_bar, show_session_scorebrd).chain(),
            ),
        )
        .add_systems(PostUpdate, on_disconnect)
        .add_plugins(DefaultPlugins.set(LogPlugin {
            level: Level::INFO,
            ..Default::default()
        }))
        .add_plugins(mampfer_plugin::MampferPlugin)
        .run();
}

fn init_clients(
    mut clients: Query<
        (
            Entity,
            &UniqueId,
            &mut EntityLayerId,
            &mut VisibleChunkLayer,
            &mut VisibleEntityLayers,
            &mut Position,
            &mut GameMode,
            &mut Client,
        ),
        Added<Client>,
    >,

    mut commands: Commands,
    server: Res<Server>,
    dimensions: Res<DimensionTypeRegistry>,
    biomes: Res<BiomeRegistry>,
    mapstuff: Res<MapState>,
    database: Res<PostgresWrapper>,
) {
    for (
        entity,
        uuid,
        mut player_layer_id,
        mut visible_chunk_layer,
        mut visible_entity_layers,
        mut pos,
        mut game_mode,
        mut client,
    ) in &mut clients
    {
        let layer = LayerBundle::new(ident!("overworld"), &dimensions, &biomes, &server);
        let mut wb = WorldBorderBundle::default();
        wb.lerp.current_diameter = 15000.0;
        wb.lerp.target_diameter = 15000.0;
        let layer_id = commands.spawn((layer, wb)).id();

        player_layer_id.0 = layer_id;
        visible_chunk_layer.0 = layer_id;
        visible_entity_layers.0.insert(layer_id);
        pos.set(SPAWN_POS);
        *game_mode = GameMode::Creative;

        client.set_resource_pack(
            "https://bits-mampfer.eu/tubnet-tourneys/minesweeper_resources.zip",
            "0ff6f1c2f43e03733090d08a44cecadccf2c532a",
            true,
            Some(Text::from("Install the minesweeper textures?")),
        );

        if let Some((top_streak, total_fields, _)) = database.get_infinite_stats(uuid) {
            client.send_chat_message(
                format!(
                    "Your best streak ever is: {top_streak}, and you have opened a total of {total_fields} cells so far."
                )
            );
        }

        let simplex = loop {
            let simplex = OpenSimplex::new(thread_rng().gen());
            if is_board_suitable(simplex) {
                break simplex;
            }
        };

        commands.entity(entity).insert(PlayerBoard {
            simplex,
            board: HashMap::new(),
            sender: mapstuff.clonable_sender.clone(),
        });

        commands.entity(entity).insert(PlayerSession::default());

        commands.spawn(ObjectiveBundle {
            name: Objective::new("session_stats"),
            display: ObjectiveDisplay("Current Session".into_text()),
            layer: EntityLayerId(layer_id),
            ..Default::default()
        });
    }
}

fn is_board_suitable(simplex: OpenSimplex) -> bool {
    (0..=2)
        .cartesian_product(0..=2)
        .all(|(x, z)| simplex.get([x as f64, z as f64]) < BOMB_THRESHOLD)
}

fn setup(mut commands: Commands) {
    let (pos_sender, pos_receiver) = mpsc::channel();
    let (chunk_sender, chunk_receiver) = mpsc::channel();

    let generator = ChunkGenerator {
        sender: chunk_sender,
        receiver: pos_receiver,
    };

    thread::spawn(move || board_generator(generator));

    let state = MapState {
        clonable_sender: pos_sender,
        receiver: Mutex::new(chunk_receiver),
    };

    commands.insert_resource(state);
}

fn on_disconnect(
    players: Query<(&PlayerSession, &UniqueId), With<Despawned>>,
    database: Res<PostgresWrapper>,
) {
    for (player, uuid) in players.iter() {
        database.save_infinite_stats(
            player.top_streak as i32,
            player.total_fields_opened as i64,
            player.total_bombs_clicked as i32,
            uuid,
        )
    }
}

fn lower_render_distance(mut players: Query<&mut ViewDistance, Changed<ViewDistance>>) {
    for mut player in players.iter_mut() {
        tracing::debug!("changed a render dist");
        let current = player.get();
        player.set(10.min(current));
    }
}

// fn debug_memory(server: Res<Server>, layers: Query<&ChunkLayer>, players: Query<&PlayerBoard>) {
//     if server.current_tick() % (20 * 10) == 0 {
//         tracing::info!("layers: {}", layers.iter().len());
//         for (i, layer) in layers.iter().enumerate() {
//             tracing::info!("layer {i} has {} chunks", layer.chunks().count());
//         }
//
//         tracing::info!("players: {}", players.iter().len());
//         for (i, player) in players.iter().enumerate() {
//             tracing::info!(
//                 "player {i} has {} entrys in his hashmap",
//                 player.board.iter().len()
//             );
//         }
//     }
// }

fn optimize_memory(
    server: Res<Server>,
    mut layers: Query<(&mut ChunkLayer, Entity)>,
    mut players: Query<(&mut PlayerBoard, &VisibleChunkLayer)>,
    mut commands: Commands,
) {
    if server.current_tick() % (20 * 10) == 0 {
        layers.for_each_mut(|(mut l, _)| l.retain_chunks(|_, chunk| chunk.viewer_count_mut() > 0));
        for (mut board, chunk_layer) in players.iter_mut() {
            if let Ok((layer, _)) = layers.get(**chunk_layer) {
                board.board.retain(|cpos, cells| {
                    layer.chunk(*cpos).is_some()
                        || cells.flatten().iter().any(|c| c.1 != CellState::Closed)
                });
            }
        }
    }

    if server.current_tick() % (20 * 600) == 0 {
        for (mut player, _) in players.iter_mut() {
            player.board.shrink_to_fit();
        }
        for (mut layer, entity) in layers.iter_mut() {
            layer.shrink_to_fit();
            if !players.iter().any(|(_, layerid)| layerid.0 == entity) {
                tracing::warn!("despawning a layer");
                commands.entity(entity).insert(Despawned);
                // commands.entity(entity).despawn();
            }
        }
    }
}

fn send_chunks_to_generator(
    mut clients: Query<(View, OldView, &mut PlayerBoard, Entity, &VisibleChunkLayer)>,
    layers: Query<&ChunkLayer>,
) {
    for (view, old_view, board, entity, layer_id) in clients.iter_mut() {
        let view = view.get();
        let old_view = old_view.get();
        let layer = layers.get(**layer_id).unwrap();

        let que_pos = |pos: ChunkPos| {
            if layer.chunk(pos).is_none() {
                board
                    .sender
                    .send((entity, board.simplex, pos, board.board.get(&pos).copied()))
                    .unwrap();
            }
        };

        if board.is_added() {
            view.iter().for_each(que_pos);
        } else {
            if old_view != view {
                view.diff(old_view).for_each(que_pos);
            }
        }
    }
}

fn chunk_receiver(
    mut layers: Query<&mut ChunkLayer>,
    state: ResMut<MapState>,
    mut players: Query<(&mut PlayerBoard, &VisibleChunkLayer, Entity)>,
    mut dig_sender: EventWriter<DiggingEvent>,
) {
    let receiver = state.receiver.lock().unwrap();

    for _ in 0..5 {
        if let Some((pos, player, chunk, board)) = receiver.try_iter().next() {
            if let Ok((mut player_board, player_layer_id, entity)) = players.get_mut(player) {
                let mut layer = layers.get_mut(player_layer_id.0).unwrap();
                if layer.chunk(pos).is_some() {
                    tracing::warn!("double generated chunk at {:?}", pos);
                } else {
                    layer.insert_chunk(pos, chunk);
                    if pos.x == 0 && pos.z == 0 {
                        dig_sender.send(DiggingEvent {
                            client: entity,
                            position: BlockPos { x: 0, y: -49, z: 0 },
                            direction: Direction::Up,
                            state: DiggingState::Start,
                        })
                    }
                }

                if player_board.board.contains_key(&pos) {
                    tracing::error!("double inserted into hashamp at {:?}", pos);
                } else {
                    player_board.board.insert(pos, board);
                }
            } else {
                tracing::error!("a chunk got loaded by a disconneted player");
            }
        }
    }
}

fn board_generator(generator: ChunkGenerator) {
    while let Ok((player, simplex, pos, opt_board)) = generator.receiver.recv() {
        // let time = SystemTime::now();
        let mut chunk = UnloadedChunk::with_height(256);

        let board = match opt_board {
            Some(bord) => bord,
            None => {
                let mut board = [[(CellContent::Empty, CellState::Closed); 16]; 16];
                let mut simplex_buffer: [[f64; 18]; 18] = [[0.0; 18]; 18];

                for offset_z in 0..18_usize {
                    for offset_x in 0..18_usize {
                        simplex_buffer[offset_z][offset_x] = simplex.get([
                            offset_x as f64 + pos.x as f64 * 16.0,
                            offset_z as f64 + pos.z as f64 * 16.0,
                        ]);
                    }
                }

                for z in 0..16_usize {
                    for x in 0..16_usize {
                        if simplex_buffer[z + 1][x + 1] > BOMB_THRESHOLD {
                            board[z][x].0 = CellContent::Bomb;
                        } else {
                            let mut bomb_count: u8 = 0;
                            for scan_x in x..=x + 2 {
                                for scan_z in z..=z + 2 {
                                    if simplex_buffer[scan_z][scan_x] > BOMB_THRESHOLD {
                                        bomb_count += 1;
                                    }
                                }
                            }
                            if bomb_count > 0 {
                                board[z][x].0 = CellContent::Number(bomb_count);
                            }
                        }
                    }
                }
                board
            }
        };

        chunk.fill_block_state_section(0, BlockState::MOSS_BLOCK);
        for z in 0..16_usize {
            for x in 0..16_usize {
                // chunk.set_block_state(x as u32, 14, z as u32, BlockState::MOSS_BLOCK);
                chunk.set_block_state(
                    x as u32,
                    15,
                    z as u32,
                    match board[z][x].1 {
                        CellState::Flagged => BlockState::RED_WOOL,
                        CellState::Closed => BlockState::MOSS_BLOCK,
                        CellState::Opened => get_num_color(board[z][x].0),
                    },
                );
            }
        }

        let r = generator.sender.send((pos, player, chunk, board));
        if let Err(e) = r {
            tracing::error!("a chunk couldnt be send {}", e);
        }
        // tracing::info!("generating chunk took: {:?}", time.elapsed().unwrap());
    }
    tracing::warn!("chunk generator shut down");
}

// fn board_generator(generator: ChunkGenerator) {
//     while let Ok((player, simplex, pos, opt_board)) = generator.receiver.recv() {
//         let time = SystemTime::now();
//         let mut chunk = UnloadedChunk::with_height(256);
//
//         let board = match opt_board {
//             Some(bord) => bord,
//             None => {
//                 let mut board: [[(MaybeUninit<CellContent>, CellState); 16]; 16] =
//                     unsafe { [[(MaybeUninit::uninit().assume_init(), CellState::Closed); 16]; 16] };
//                 let mut simplex_buffer: [[f64; 18]; 18] = [[0.0; 18]; 18];
//
//                 for offset_z in 0..18_usize {
//                     for offset_x in 0..18_usize {
//                         simplex_buffer[offset_z][offset_x] = simplex.get([
//                             offset_x as f64 + pos.x as f64 * 16.0,
//                             offset_z as f64 + pos.z as f64 * 16.0,
//                         ]);
//                     }
//                 }
//
//                 let bomb_threshold = 0.195;
//                 for z in 0..16_usize {
//                     for x in 0..16_usize {
//                         if simplex_buffer[z + 1][x + 1] > bomb_threshold {
//                             board[z][x].0.write(CellContent::Bomb);
//                         } else {
//                             let mut bomb_count: u8 = 0;
//                             for scan_x in x..=x + 2 {
//                                 for scan_z in z..=z + 2 {
//                                     if simplex_buffer[scan_z][scan_x] > bomb_threshold {
//                                         bomb_count += 1;
//                                     }
//                                 }
//                             }
//                             if bomb_count > 0 {
//                                 board[z][x].0.write(CellContent::Number(bomb_count));
//                             }
//                         }
//                     }
//                 }
//                 unsafe { mem::transmute::<_, BoardChunk>(board) }
//             }
//         };
//
//         for z in 0..16_usize {
//             for x in 0..16_usize {
//                 chunk.set_block_state(
//                     x as u32,
//                     15,
//                     z as u32,
//                     match board[z][x].1 {
//                         CellState::Flagged => BlockState::RED_WOOL,
//                         CellState::Closed => BlockState::MOSS_BLOCK,
//                         CellState::Opened => get_num_color(board[z][x].0),
//                     },
//                 );
//             }
//         }
//
//         let r = generator.sender.send((pos, player, chunk, board));
//         if let Err(e) = r {
//             tracing::error!("a chunk couldnt be send {}", e);
//         }
//         tracing::info!("generating chunk took: {:?}", time.elapsed().unwrap());
//     }
//     tracing::warn!("chunk generator shut down");
// }
