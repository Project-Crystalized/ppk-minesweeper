#![feature(slice_flatten)]

use mampfer_plugin::PostgresWrapper;

use mampfer_plugin::postgres_wrapper::LeaderBoardType;
use valence::anvil::parsing::DimensionFolder;
use valence::anvil::{AnvilLevel, ChunkLoadEvent, ChunkLoadStatus};
use valence::entity::armor_stand::ArmorStandEntity;
use valence::interact_block::InteractBlockEvent;
use valence::world_border::WorldBorderBundle;
use valence::{interact_item::InteractItemEvent, inventory::HeldItem, prelude::*};

mod ms2d;
use ms2d::items::StartItemType;
use ms2d::MineSweeperGame2d;

use minesweeper_lib::MinesweeperGamePlugin;
use minesweeper_lib::{create_leaderbaords, MinesweeperGameContainer};

pub const BORDER_DISTANCE: i32 = 250;

fn main() {
    App::new()
        .insert_resource(NetworkSettings {
            connection_mode: ConnectionMode::Velocity {
                secret: "temporärsecret".into(),
            },
            address: "0.0.0.0:25567".parse().unwrap(),
            ..Default::default()
        })
        .add_systems(Startup, setup)
        .insert_resource(LeaderBoardType { dim: 2, size: 50 })
        .add_systems(
            Update,
            (
                init_clients,
                despawn_disconnected_clients,
                item_use_listener,
                update_leaderboards,
                // handle_chunk_loads,
                set_leaderboard_types,
            ),
        )
        .add_plugins(MinesweeperGamePlugin)
        .run();
}

fn setup(
    mut commands: Commands,
    server: Res<Server>,
    dimensions: Res<DimensionTypeRegistry>,
    biomes: Res<BiomeRegistry>,
    mut database: ResMut<PostgresWrapper>,
    selected_type: Res<LeaderBoardType>,
) {
    let mut world = DimensionFolder::new("./world", &biomes);
    let mut layer = LayerBundle::new(ident!("overworld"), &dimensions, &biomes, &server);

    for z in -11..11 {
        for x in -11..11 {
            let pos = ChunkPos { x, z };
            match world.get_chunk(pos) {
                Err(err) => {
                    tracing::error!("error loading chunk at {:?}, {err}", pos);
                    layer.chunk.insert_chunk(pos, UnloadedChunk::new());
                }
                Ok(None) => {
                    tracing::error!("there was no chunk to load at {:?}", pos);
                    layer.chunk.insert_chunk(pos, UnloadedChunk::new());
                }
                Ok(Some(chunk)) => {
                    layer.chunk.insert_chunk(pos, chunk.chunk);
                }
            }
        }
    }

    // for z in -BORDER_DISTANCE..BORDER_DISTANCE {
    //     for x in -BORDER_DISTANCE..BORDER_DISTANCE {
    //         if rand::random::<bool>() {
    //             layer.chunk.set_block((x, -61, z), BlockState::MOSS_BLOCK);
    //         }
    //     }
    // }

    let mut wb = WorldBorderBundle::default();
    wb.lerp.current_diameter = BORDER_DISTANCE as f64;
    wb.lerp.target_diameter = BORDER_DISTANCE as f64;
    let layer_id = commands.spawn((layer, wb)).id();
    create_leaderbaords(&selected_type, &mut database, &mut commands, layer_id);
}

fn set_leaderboard_types(
    mut clicks: EventReader<InteractBlockEvent>,
    mut selected_type: ResMut<LeaderBoardType>,
    layer: Query<&ChunkLayer>,
) {
    let layer = layer.single();
    for click in clicks.read() {
        if let Some(BlockRef {
            state: BlockState::STONE_BUTTON,
            ..
        }) = layer.block(click.position)
        {
            selected_type.next();
        }
    }
}

fn update_leaderboards(
    mut commands: Commands,
    old_armorstands: Query<Entity, With<ArmorStandEntity>>,
    layer: Query<Entity, With<EntityLayer>>,
    mut database: ResMut<PostgresWrapper>,
    mut time: Local<usize>,
    selected_type: Res<LeaderBoardType>,
) {
    *time += 1;
    if *time % 200 == 0 || selected_type.is_changed() {
        old_armorstands.for_each(|a| {
            commands.entity(a).insert(Despawned);
        });
        create_leaderbaords(&selected_type, &mut database, &mut commands, layer.single());
    }
}

fn init_clients(
    mut clients: Query<
        (
            &mut EntityLayerId,
            &mut VisibleChunkLayer,
            &mut VisibleEntityLayers,
            &mut Position,
            &mut GameMode,
            &mut Client,
            &mut Inventory,
            &UniqueId,
            &Username,
        ),
        Added<Client>,
    >,
    layers: Query<Entity, (With<ChunkLayer>, With<EntityLayer>)>,
    database: ResMut<PostgresWrapper>,
) {
    for (
        mut layer_id,
        mut visible_chunk_layer,
        mut visible_entity_layers,
        mut pos,
        mut game_mode,
        mut client,
        mut inv,
        uuid,
        username,
    ) in &mut clients
    {
        tracing::info!("{} logged on", username);
        let layer = layers.single();

        layer_id.0 = layer;
        visible_chunk_layer.0 = layer;
        visible_entity_layers.0.insert(layer);
        pos.set([20.5, 3.0, 9.5]);
        *game_mode = GameMode::Creative;

        for e in StartItemType::all_types() {
            let slot_num = inv.first_empty_slot_in(36..45).unwrap();
            inv.set_slot(slot_num, StartItemType::create_start_item(e));
        }

        if let Some((dim, comp_time, helpers)) = database.get_best_game(uuid, 2) {
            client.send_chat_message(
                format!(
                    "Your best minesweeper game took: {} seconds, it was a {dim}x{dim} game. ",
                    comp_time / 20
                )
                .not_obfuscated()
                    + format!("The people participated are: {helpers}"),
            );
        } else {
            tracing::info!("a new player logged on: {}", username);
            client.send_chat_message(
                "Welcome to Minesweeper, looks like your new ".bold()
                    + "Click Here"
                        .on_click_open_url(
                            "https://www.google.com/search?q=how+to+play+minesweeper",
                        )
                        .color(Color::AQUA)
                        .bold()
                    + " for a explanation of minesweeper!"
                        .clear_click_event()
                        .clear_color()
                        .bold(),
            );
        }

        client.set_resource_pack(
            "https://bits-mampfer.eu/tubnet-tourneys/minesweeper_resources.zip",
            "0ff6f1c2f43e03733090d08a44cecadccf2c532a",
            true,
            Some(Text::from("Install the minesweeper textures?")),
        );
    }
}

fn item_use_listener(
    mut item_interacts: EventReader<InteractItemEvent>,
    mut players: Query<(&Inventory, &HeldItem, &Position, &UniqueId, &mut Client)>,
    mut commands: Commands,
    games: Query<&MinesweeperGameContainer>,
) {
    for interaction in item_interacts.read() {
        let (inv, held_item, pos, uuid, mut client) = players.get_mut(interaction.client).unwrap();
        let held_item = inv.slot(held_item.slot());
        let players_game_count = games
            .iter()
            .filter(|game| game.get_player().0 == interaction.client)
            .count();
        if players_game_count >= 3 {
            client.send_chat_message("you cant have more than 3 games active".color(Color::RED));
            return;
        }

        if let Some(item_type) = StartItemType::get_start_item_type(held_item) {
            match item_type {
                StartItemType::Minesweeper20x20 => {
                    let msg = MineSweeperGame2d::<20>::new(pos, (interaction.client, *uuid));
                    commands.spawn(MinesweeperGameContainer(Box::new(msg)));
                }
                StartItemType::Minesweeper30x30 => {
                    let msg = MineSweeperGame2d::<30>::new(pos, (interaction.client, *uuid));
                    commands.spawn(MinesweeperGameContainer(Box::new(msg)));
                }
                StartItemType::Minesweeper50x50 => {
                    let msg = MineSweeperGame2d::<50>::new(pos, (interaction.client, *uuid));
                    commands.spawn(MinesweeperGameContainer(Box::new(msg)));
                }
            }
        }
    }
}

fn handle_chunk_loads(
    mut events: EventReader<ChunkLoadEvent>,
    mut layers: Query<&mut ChunkLayer, With<AnvilLevel>>,
) {
    let mut layer = layers.single_mut();

    for event in events.read() {
        match &event.status {
            ChunkLoadStatus::Success { .. } => {
                // The chunk was inserted into the world. Nothing for us to do.
            }
            ChunkLoadStatus::Empty => {
                // There's no chunk here so let's insert an empty chunk. If we were doing
                // terrain generation we would prepare that here.
                layer.insert_chunk(event.pos, UnloadedChunk::new());
            }
            ChunkLoadStatus::Failed(e) => {
                // Something went wrong.
                let errmsg = format!(
                    "failed to load chunk at ({}, {}): {e:#}",
                    event.pos.x, event.pos.z
                );

                eprintln!("{errmsg}");
                layer.send_chat_message(errmsg.color(Color::RED));

                layer.insert_chunk(event.pos, UnloadedChunk::new());
            }
        }
    }
}
