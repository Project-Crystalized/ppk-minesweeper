//! `/ms` command tree: help, new games, clear/list, infinite teleport.

use pumpkin_plugin_api::{
    command::{Arg, CommandError, CommandSender, ConsumedArgs},
    commands::CommandHandler,
    text::TextComponent,
    wit::pumpkin::plugin::command::Number,
    Server,
};

use crate::{games::NewGameSpec, helpers::uuid_key, state::GameKind};

fn help_text() -> TextComponent {
    TextComponent::text(&format!(
        "Minesweeper: right-click a starter stick (or /ms <SIZE>) to start a game at your feet. Sizes: {}. Left-click opens, right-click flags. /ms kit re-gives sticks + lock/flag items. /ms lock [id] toggles game locks (or right-click the horse-armor item). /ms flag cycles flag color. /ms clear removes your games. /ms stats shows your records. /ms top shows leaderboards. /ms infinite teleports to endless mode.",
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
        sender.send_message(help_text());
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
    match parse_size(args).and_then(|n| usize::try_from(n).ok()).and_then(|n| NewGameSpec::custom(kind, n)) {
        Some(spec) => {
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
        server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players can clear games."));
            return Ok(0);
        };
        crate::games::clear_player_games(&player, &server);
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

pub struct LockCommand;

impl CommandHandler for LockCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players can lock games."));
            return Ok(0);
        };
        let id = match args.get_value("id") {
            Arg::Num(Ok(Number::Int32(n))) => u32::try_from(n).ok(),
            Arg::Num(Ok(Number::Int64(n))) => u32::try_from(n).ok(),
            _ => None,
        };
        let line = crate::games::toggle_lock_report(uuid_key(&player), id);
        sender.send_message(TextComponent::text(&line));
        crate::sticks::refresh_utility_items(&player);
        Ok(1)
    }
}

pub struct FlagCommand;

impl CommandHandler for FlagCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players can switch flags."));
            return Ok(0);
        };
        let name = {
            let key = uuid_key(&player);
            crate::state::cycle_flag_style(key);
            crate::state::flag_style_name(key)
        };
        sender.send_message(TextComponent::text(&format!("Flag color: {name}.")));
        crate::sticks::refresh_utility_items(&player);
        Ok(1)
    }
}

pub struct StatsCommand;

/// Board label with the right dimensionality ("2D 30x30", "3D 5x5x5").
fn spec_label(kind: i64, dim: usize) -> String {
    if kind == crate::db::KIND_3D {
        format!("3D {dim}x{dim}x{dim}")
    } else {
        format!("2D {dim}x{dim}")
    }
}

/// Compact lifetime duration ("2h 5m", "12m", "45s") from ticks (20/s).
fn fmt_duration(ticks: u64) -> String {
    let secs = ticks / 20;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
}

impl CommandHandler for StatsCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(player) = player_of(&sender) else {
            sender.send_error(TextComponent::text("Only players have stats."));
            return Ok(0);
        };
        let key = uuid_key(&player);
        let mut lines = vec![format!("Stats for {}:", player.get_name())];
        let records = crate::db::spec_records(key);
        if records.is_empty() {
            lines.push("No recorded games yet.".to_string());
        }
        for r in records {
            let games = if r.games == 1 { "1 game".into() } else { format!("{} games", r.games) };
            let wins = if r.wins == 1 { "1 win".into() } else { format!("{} wins", r.wins) };
            let mut line = format!("{}: {games}, {wins}", spec_label(r.kind, r.dim));
            if r.abandoned > 0 {
                line.push_str(&format!(", {} abandoned", r.abandoned));
            }
            if let Some(best) = r.best_secs {
                line.push_str(&format!(", best {best}s"));
            }
            if let Some(avg) = r.avg_secs {
                line.push_str(&format!(", avg {avg}s"));
            }
            lines.push(line);
            lines.push(format!(
                "  {} opened, {} flags placed, {} bombs",
                r.opened, r.flags, r.bombs
            ));
        }
        let helper = crate::db::helper_record(key);
        if helper.games > 0 {
            lines.push(format!(
                "Co-op: {} games as helper, {} wins",
                helper.games, helper.wins
            ));
        }
        match crate::db::load_infinite(key) {
            Some(row) => lines.push(format!(
                "Infinite: best streak {} ({} opened, {} bombs clicked, {} played)",
                row.top,
                row.opened,
                row.bombs_clicked,
                fmt_duration(row.playtime)
            )),
            None => lines.push("Infinite: no recorded cells yet.".to_string()),
        }
        sender.send_message(TextComponent::text(&lines.join("\n")));
        Ok(1)
    }
}

pub struct TopCommand;

impl CommandHandler for TopCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let mut lines = vec!["Top wins (fastest):".to_string()];
        let wins = crate::db::top_wins(None, 10);
        if wins.is_empty() {
            lines.push("No recorded wins yet.".to_string());
        }
        for (i, w) in wins.iter().enumerate() {
            lines.push(format!(
                "{}. {} - {} in {}s",
                i + 1,
                w.owner_name,
                spec_label(w.kind, w.dim),
                w.secs
            ));
        }
        lines.push("Top infinite streaks:".to_string());
        let inf = crate::db::top_infinite(5);
        if inf.is_empty() {
            lines.push("No recorded streaks yet.".to_string());
        }
        for (i, t) in inf.iter().enumerate() {
            lines.push(format!(
                "{}. {} - streak {} ({} opened)",
                i + 1,
                t.name,
                t.top,
                t.opened
            ));
        }
        sender.send_message(TextComponent::text(&lines.join("\n")));
        Ok(1)
    }
}
