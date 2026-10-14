//! `/ms` command tree: help, new games, clear/list, infinite teleport.

use pumpkin_plugin_api::{
    command::{Arg, CommandError, CommandSender, ConsumedArgs},
    commands::CommandHandler,
    text::TextComponent,
    wit::pumpkin::plugin::command::Number,
    Server,
};
use tracing::info;

use crate::{games::NewGameSpec, helpers::uuid_key, state::GameKind};

fn help_text() -> TextComponent {
    TextComponent::text(&format!(
        "Minesweeper: right-click a starter stick (or /ms <SIZE>) to start a game at your feet. Sizes: {}. Left-click opens, right-click flags. /ms kit re-gives sticks. /ms clear removes your games. /ms infinite teleports to endless mode.",
        crate::games::NewGameSpec::all_labels()
    ))
}

fn player_of(sender: &CommandSender) -> Option<pumpkin_plugin_api::Player> {
    sender.as_player()
}

pub struct HelpCommand;

impl CommandHandler for HelpCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        info!("/ms help from {}", sender.get_name());
        sender.send_message(help_text());
        Ok(1)
    }
}

pub struct RootCommand;

impl CommandHandler for RootCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        info!("/ms from {}", sender.get_name());
        sender.send_message(help_text());
        Ok(1)
    }
}

pub struct NewGameCommand {
    pub spec: NewGameSpec,
}

impl CommandHandler for NewGameCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        info!("/ms {} from {}", self.spec.label(), sender.get_name());
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players can start games."));
            return Ok(0);
        };
        crate::games::create_game(&player, self.spec);
        Ok(1)
    }
}

fn parse_size(args: &ConsumedArgs) -> Option<i32> {
    match args.get_value("size") {
        Arg::Num(Ok(Number::Int32(n))) => Some(n),
        Arg::Num(Ok(Number::Int64(n))) => i32::try_from(n).ok(),
        // Out-of-range counts rejected by Brigadier bounds arrive here.
        Arg::Num(Err(_)) => None,
        _ => None,
    }
}

fn start_sized(sender: &CommandSender, kind: GameKind, args: &ConsumedArgs) -> Result<i32, CommandError> {
    let Some(player) = player_of(sender) else {
        sender.send_error(TextComponent::text("Only players can start games."));
        return Ok(0);
    };
    let label = match kind {
        GameKind::TwoD => "2D",
        GameKind::ThreeD => "3D",
    };
    match parse_size(args).and_then(|n| usize::try_from(n).ok()).and_then(|n| NewGameSpec::custom(kind, n)) {
        Some(spec) => {
            info!("/ms {} dim={} from {}", label, spec.dim, sender.get_name());
            crate::games::create_game(&player, spec);
            Ok(1)
        }
        None => {
            sender.send_error(TextComponent::text("Size must be an integer of 2 or more."));
            Ok(0)
        }
    }
}

pub struct NewGame2dCommand;

impl CommandHandler for NewGame2dCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        start_sized(&sender, GameKind::TwoD, &args)
    }
}

pub struct NewGame3dCommand;

impl CommandHandler for NewGame3dCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        start_sized(&sender, GameKind::ThreeD, &args)
    }
}

pub struct ClearCommand;impl CommandHandler for ClearCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players can clear games."));
            return Ok(0);
        };
        crate::games::clear_player_games(&player);
        Ok(1)
    }
}

pub struct ListCommand;

impl CommandHandler for ListCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let line = if let Some(player) = player_of(&sender) {
            crate::games::describe_all_for(Some(uuid_key(&player)))
        } else {
            crate::games::describe_all_for(None)
        };
        sender.send_message(TextComponent::text(&format!("Games: {line}")));
        Ok(1)
    }
}

pub struct InfiniteCommand;

impl CommandHandler for InfiniteCommand {
    fn handle(
        &self,
        sender: CommandSender,
        server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        info!("/ms infinite from {}", sender.get_name());
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players can teleport."));
            return Ok(0);
        };
        crate::infinite::teleport_here(&player, &server);
        Ok(1)
    }
}

pub struct KitCommand;

impl CommandHandler for KitCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players can get kits."));
            return Ok(0);
        };
        crate::sticks::give_kit_and_tell(&player);
        Ok(1)
    }
}
