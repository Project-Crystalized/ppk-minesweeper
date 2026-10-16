use pumpkin_plugin_api::{
    command::{Command, CommandNode},
    events::{
        BlockExplodeEvent, CreatureSpawnEvent, EntityExplodeEvent, EventPriority,
        SpawnerSpawnEvent, TrialSpawnerSpawnEvent,
    },
    permission::{Permission, PermissionDefault},
    Context, Plugin, PluginMetadata,
};
use tracing::*;

mod commands;
mod core;
mod db;
mod games;
mod helpers;
mod infinite;
mod listeners;
mod state;
mod sticks;
mod tick;

struct MinesweeperPlugin;

impl Plugin for MinesweeperPlugin {
    fn new() -> Self {
        MinesweeperPlugin
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "Minesweeper".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["cooltexture1".into()],
            description: "2D, 3D and infinite minesweeper in survival worlds".into(),
            dependencies: vec![],
            // Host capability for the SQLite file in the data folder.
            permissions: vec!["fs.write.data".to_string()],
        }
    }

    fn on_load(&self, context: Context) -> pumpkin_plugin_api::Result<()> {
        info!("Minesweeper loading");

        // SQLite records (fail-soft: the game runs without them).
        // Needs the `fs.write.data` manifest permission for `data/`.
        crate::db::setup();

        infinite::register_generator();

        context.register_event_handler(listeners::JoinListener, EventPriority::Normal, false)?;
        context.register_event_handler(listeners::LeaveListener, EventPriority::Normal, true)?;
        context.register_event_handler(listeners::BreakListener, EventPriority::Normal, true)?;
        context.register_event_handler(
            listeners::InteractListener,
            EventPriority::Normal,
            true,
        )?;
        context.register_event_handler(
            listeners::PlaceListener,
            EventPriority::Normal,
            true,
        )?;
        context.register_event_handler::<CreatureSpawnEvent, _>(
            listeners::NoMobSpawnListener,
            EventPriority::Normal,
            true,
        )?;
        context.register_event_handler::<SpawnerSpawnEvent, _>(
            listeners::NoMobSpawnListener,
            EventPriority::Normal,
            true,
        )?;
        context.register_event_handler::<TrialSpawnerSpawnEvent, _>(
            listeners::NoMobSpawnListener,
            EventPriority::Normal,
            true,
        )?;
        context.register_event_handler::<BlockExplodeEvent, _>(
            listeners::NoExplodeListener,
            EventPriority::Normal,
            true,
        )?;
        context.register_event_handler::<EntityExplodeEvent, _>(
            listeners::NoExplodeListener,
            EventPriority::Normal,
            true,
        )?;

        use games::NewGameSpec as Spec;
        use pumpkin_plugin_api::command::ArgumentType;
        let cmd = Command::new(&["ms".to_string()], "Play minesweeper")
            .then(CommandNode::literal("help").execute(commands::HelpCommand))
            .then(
                CommandNode::literal("2d20")
                    .execute(commands::NewGameCommand { spec: Spec::TWO_D_20 }),
            )
            .then(
                CommandNode::literal("2d30")
                    .execute(commands::NewGameCommand { spec: Spec::TWO_D_30 }),
            )
            .then(
                CommandNode::literal("2d50")
                    .execute(commands::NewGameCommand { spec: Spec::TWO_D_50 }),
            )
            .then(CommandNode::literal("2d").then(
                CommandNode::argument("size", &ArgumentType::Integer((None, None)))
                    .execute(commands::NewGame2dCommand),
            ))
            .then(
                CommandNode::literal("3d5")
                    .execute(commands::NewGameCommand { spec: Spec::THREE_D_5 }),
            )
            .then(
                CommandNode::literal("3d7")
                    .execute(commands::NewGameCommand { spec: Spec::THREE_D_7 }),
            )
            .then(
                CommandNode::literal("3d10")
                    .execute(commands::NewGameCommand { spec: Spec::THREE_D_10 }),
            )
            .then(CommandNode::literal("3d").then(
                CommandNode::argument("size", &ArgumentType::Integer((None, None)))
                    .execute(commands::NewGame3dCommand),
            ))
            .then(CommandNode::literal("clear").execute(commands::ClearCommand))
            .then(CommandNode::literal("list").execute(commands::ListCommand))
            .then(CommandNode::literal("kit").execute(commands::KitCommand))
            .then(
                CommandNode::literal("lock")
                    .then(
                        CommandNode::argument("id", &ArgumentType::Integer((None, None)))
                            .execute(commands::LockCommand),
                    )
                    .execute(commands::LockCommand),
            )
            .then(CommandNode::literal("flag").execute(commands::FlagCommand))
            .then(CommandNode::literal("stats").execute(commands::StatsCommand))
            .then(CommandNode::literal("top").execute(commands::TopCommand))
            .then(CommandNode::literal("infinite").execute(commands::InfiniteCommand))
            .execute(commands::RootCommand);
        if let Err(e) = context.register_permission(&Permission {
            node: "Minesweeper:command.ms".to_string(),
            description: "Allows using /ms minesweeper commands".to_string(),
            default: PermissionDefault::Allow,
            children: vec![],
        }) {
            warn!("Failed to register minesweeper permission: {e}");
        }
        context.register_command(cmd, "Minesweeper:command.ms");

        // Lock the server down for minigame mode: no mob spawning etc.
        let server = context.get_server();
        for world in server.get_all_worlds() {
            helpers::apply_peaceful_rules(&world);
        }
        info!("Peaceful gamerules applied");

        tick::start_tick_task(&context);

        info!("Minesweeper loaded: /ms help");
        Ok(())
    }

    fn on_unload(&self, _context: Context) -> pumpkin_plugin_api::Result<()> {
        info!("Minesweeper unloaded");
        Ok(())
    }
}

pumpkin_plugin_api::register_plugin!(MinesweeperPlugin);
