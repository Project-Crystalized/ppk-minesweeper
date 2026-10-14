use std::{
    fmt::Display,
    ops::{Deref, DerefMut},
};

use super::ActionState;

use mampfer_plugin::postgres_wrapper::PostgresWrapper;
use tracing::Level;
use valence::{
    interact_block::InteractBlockEvent, interact_item::InteractItemEvent, inventory::HeldItem,
    log::LogPlugin, nbt::compound, prelude::*,
};

pub struct MinesweeperGamePlugin;

impl Plugin for MinesweeperGamePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                add_states,
                tick_games,
                despawn_games,
                on_block_click,
                on_block_break,
                build_spawned_games,
                flag_item_update,
                change_player_state,
                show_action_bar,
                lock_item_update,
            ),
        )
        .add_plugins(DefaultPlugins.set(LogPlugin {
            level: Level::INFO,
            ..Default::default()
        }))
        .add_plugins(mampfer_plugin::MampferPlugin);
    }
}

fn change_player_state(
    mut item_events: EventReader<InteractItemEvent>,
    mut clients: Query<(&HeldItem, &mut FlagType, &mut GameLock)>,
) {
    for item_event in item_events.read() {
        let (held_item, mut flag, mut lock) = clients.get_mut(item_event.client).unwrap();
        if held_item.slot() == 44 {
            flag.set_next();
        } else if held_item.slot() == 43 {
            lock.0 = !lock.0;
        }
    }
}

fn flag_item_update(mut flag_updates: Query<(&mut Inventory, &FlagType), Changed<FlagType>>) {
    for (mut inv, flag) in flag_updates.iter_mut() {
        let item = ItemStack::new(
            flag.0.to_kind().to_item_kind(),
            1,
            Some(compound! {
                "display" => compound! {
                "Name" => "\" Switch flag color\"",
                }
            }),
        );
        inv.set_slot(44, item);
    }
}

fn lock_item_update(mut lock_updates: Query<(&mut Inventory, &GameLock), Changed<GameLock>>) {
    for (mut inv, lock) in lock_updates.iter_mut() {
        let item = if lock.0 {
            ItemStack::new(
                ItemKind::LeatherHorseArmor,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Game is Locked\"",
                    }
                }),
            )
        } else {
            ItemStack::new(
                ItemKind::IronHorseArmor,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Game is Unlocked\"",
                    }
                }),
            )
        };
        inv.set_slot(43, item);
    }
}

fn show_action_bar(
    games: Query<&MinesweeperGameContainer>,
    mut clients: Query<(&mut Client, &Position)>,
    server: Res<Server>,
) {
    if server.current_tick() % 10 == 0 {
        let game_states: Vec<ActionState> = games.iter().map(|g| g.0.get_action_state()).collect();
        for (mut client, pos) in clients.iter_mut() {
            if let Some(smallest) = game_states
                .iter()
                .min_by_key(|g| g.center.distance(**pos) as isize)
            {
                client.set_action_bar(smallest.to_text());
            }
        }
    }
}

fn build_spawned_games(
    mut layer: Query<&mut ChunkLayer>,
    mut new_games: Query<(Entity, &mut MinesweeperGameContainer), Added<MinesweeperGameContainer>>,
    mut clients: Query<&mut Client>,
    mut commands: Commands,
) {
    for mut new_game in new_games.iter_mut() {
        if let Err(err) = new_game.1.build_blocks(layer.single_mut().as_mut()) {
            match clients.get_mut(new_game.1.get_player().0) {
                Ok(mut player) => {
                    player.send_chat_message(format!("Couldnt start game: {err}"));
                    commands.entity(new_game.0).despawn();
                }
                Err(err) => tracing::warn!("A games, player couldnt be found: {}", err),
            }
        }
    }
}

fn on_block_click(
    mut block_interacts: EventReader<InteractBlockEvent>,
    mut games: Query<&mut MinesweeperGameContainer>,
    mut layer: Query<&mut ChunkLayer>,
    flags: Query<&FlagType>,
    lock: Query<&GameLock>,
) {
    for interaction in block_interacts.read() {
        let flagtype = flags.get(interaction.client).unwrap();
        games.for_each_mut(|ms| {
            let lock = lock.get(ms.get_player().0).unwrap();
            if ms.get_player().0 == interaction.client || !lock.0 {
                ms.into_inner().0.click_right(
                    &interaction.position,
                    interaction.client,
                    layer.single_mut().into_inner(),
                    flagtype.0,
                )
            }
        });
    }
}

fn on_block_break(
    mut block_interacts: ResMut<Events<DiggingEvent>>,
    mut games: Query<&mut MinesweeperGameContainer>,
    mut layer: Query<&mut ChunkLayer>,
    players: Query<&UniqueId>,
    lock: Query<&GameLock>,
) {
    let mut r = block_interacts.get_reader();
    let mut que: Vec<DiggingEvent> = Vec::new();
    for interaction in r.read(&block_interacts) {
        match players.get(interaction.client) {
            Ok(entity) => {
                games.for_each_mut(|ms| {
                    let lock = lock.get(ms.get_player().0).unwrap();
                    if ms.get_player().0 == interaction.client || !lock.0 {
                        ms.into_inner().0.click_left(
                            &interaction.position,
                            (interaction.client, *entity),
                            layer.single_mut().into_inner(),
                            &mut que,
                        )
                    }
                });
            }
            Err(err) => tracing::error!("some error happened {}", err),
        }
    }
    que.iter().for_each(|e| block_interacts.send(*e));
}

fn tick_games(mut games: Query<&mut MinesweeperGameContainer>, mut layer: Query<&mut ChunkLayer>) {
    games.for_each_mut(|mut g| g.tick(layer.single_mut().into_inner()));
}

fn despawn_games(
    mut games: Query<(Entity, &mut MinesweeperGameContainer)>,
    mut layer: Query<&mut ChunkLayer>,
    mut commands: Commands,
    mut database: ResMut<PostgresWrapper>,
    clients: Query<With<Client>>,
) {
    let despawn_all = clients.iter().len() == 0;
    games.for_each_mut(|g| {
        if (g.1.should_despawn()) || despawn_all || clients.get(g.1.get_player().0).is_err() {
            g.1.reset(layer.single_mut().into_inner(), database.deref_mut());
            commands.entity(g.0).despawn()
        }
    });
}

fn add_states(mut commands: Commands, client: Query<Entity, Added<Client>>) {
    client.for_each(|c| {
        commands.entity(c).insert(FlagType(BlockState::RED_WOOL));
        commands.entity(c).insert(GameLock(false));
    })
}

#[derive(Component)]
pub struct MinesweeperGameContainer(pub Box<dyn MinesweeperGame + Send + Sync>);

impl Deref for MinesweeperGameContainer {
    type Target = Box<dyn MinesweeperGame + Send + Sync>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MinesweeperGameContainer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait MinesweeperGame {
    fn build_blocks(&mut self, layer: &mut ChunkLayer) -> Result<(), BuildGameError>;

    fn tick(&mut self, layer: &mut ChunkLayer);

    fn click_right(
        &mut self,
        click_pos: &BlockPos,
        player: Entity,
        layer: &mut ChunkLayer,
        flag: BlockState,
    );
    fn click_left(
        &mut self,
        click_pos: &BlockPos,
        player: (Entity, UniqueId),
        layer: &mut ChunkLayer,
        click_writer: &mut Vec<DiggingEvent>,
    );

    fn reset(&self, layer: &mut ChunkLayer, pgsql: &mut PostgresWrapper);

    fn should_despawn(&self) -> bool;

    fn get_player(&self) -> (Entity, UniqueId);

    fn get_action_state(&self) -> ActionState;
}

#[derive(Debug)]
pub enum BuildGameError {
    BlocksInTheWay,
    BlocksTooClose,
    InBorder,
}

impl Display for BuildGameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildGameError::BlocksInTheWay => write!(f, "Error! There are Blocks in the Way"),
            BuildGameError::InBorder => {
                write!(f, "Error! Your Blocks would have been in the border")
            }
            BuildGameError::BlocksTooClose => {
                write!(f, "Error! There are Blocks too close to the game")
            }
        }
    }
}

#[derive(Component)]
struct FlagType(BlockState);

#[derive(Component)]
struct GameLock(pub bool);

impl FlagType {
    fn set_next(&mut self) {
        match self.0 {
            BlockState::RED_WOOL => self.0 = BlockState::ORANGE_WOOL,
            BlockState::ORANGE_WOOL => self.0 = BlockState::YELLOW_WOOL,
            BlockState::YELLOW_WOOL => self.0 = BlockState::CYAN_WOOL,
            BlockState::CYAN_WOOL => self.0 = BlockState::BLUE_WOOL,
            BlockState::BLUE_WOOL => self.0 = BlockState::RED_WOOL,
            _ => unreachable!(),
        }
    }
}
