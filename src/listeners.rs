//! Pumpkin event handlers: join setup, leave accounting, left-click opens,
//! right-click flags.

use pumpkin_plugin_api::{
    events::{
        BlockBreakEvent, BlockExplodeEvent, BlockPlaceEvent, CreatureSpawnEvent,
        EntityExplodeEvent, EventData, EventHandler, InteractAction, PlayerInteractEvent,
        PlayerJoinEvent, PlayerLeaveEvent, SpawnerSpawnEvent, TrialSpawnerSpawnEvent,
    },
    Player, Server,
};

use crate::{
    games,
    helpers::{tell, to_pos3, uuid_key},
    infinite,
    state,
};

fn reset_flag_color(player: &Player) {
    crate::state::reset_flag_style(uuid_key(player));
    tell(player, "Flag color reset to red.");
    crate::sticks::refresh_utility_items(player);
}

pub struct JoinListener;

impl EventHandler<PlayerJoinEvent> for JoinListener {
    fn handle(&self, _server: Server, event: EventData<PlayerJoinEvent>) -> EventData<PlayerJoinEvent> {
        // Fresh kit: wipe stale slots, then lay out the hotbar kit.
        crate::sticks::give_fresh_kit(&event.player);
        crate::state::mark_join(crate::helpers::uuid_key(&event.player));
        crate::db::touch_player(
            crate::helpers::uuid_key(&event.player),
            &event.player.get_name(),
        );
        crate::state::update_owner_name(
            crate::helpers::uuid_key(&event.player),
            &event.player.get_name(),
        );
        crate::infinite::hydrate_session(&event.player);
        tell(
            &event.player,
            "Minesweeper is installed! Starter sticks are in your hotbar - press 1-4 and right-click to play, or /ms help. Right-click the wool to cycle flag colors; left-click it to reset to red. Building, breaking and mob spawning are disabled.",
        );
        event
    }
}

pub struct LeaveListener;

impl EventHandler<PlayerLeaveEvent> for LeaveListener {
    fn handle(
        &self,
        _server: Server,
        event: EventData<PlayerLeaveEvent>,
    ) -> EventData<PlayerLeaveEvent> {
        let owner = uuid_key(&event.player);
        crate::sidebar::clear(&event.player);
        // Boards deliberately survive disconnects: an owner coming back keeps
        // their progress, and helpers can keep playing unlocked boards.
        crate::db::add_playtime(owner, state::take_playtime_ticks(owner));
        infinite::persist_session_for(owner, &event.player);
        event
    }
}

pub struct BreakListener;

impl EventHandler<BlockBreakEvent> for BreakListener {
    fn handle(
        &self,
        server: Server,
        mut event: EventData<BlockBreakEvent>,
    ) -> EventData<BlockBreakEvent> {
        let Some(player) = &event.player else {
            return event;
        };
        let world = player.get_world();
        let world_id = world.get_id();
        let pos = to_pos3(event.block_pos);

        // Bedrock does not fire PlayerInteractEvent for block clicks.
        if let Some(crate::sticks::HeldSpecial::FlagCycle) =
            crate::sticks::held_special(player)
        {
            reset_flag_color(player);
            event.cancelled = true;
            event.should_drop = false;
            event.exp = 0;
            return event;
        }

        if let Some((id, idx)) = games::route(&world_id, pos) {
            // Board clicks never break the underlying block, even when the
            // click itself is a no-op (flagged/open/locked/finished board).
            games::click_left(&world, player, id, idx);
            event.cancelled = true;
            event.should_drop = false;
            event.exp = 0;
            return event;
        }

        if infinite::is_infinite_world(&world)
            && infinite::handle_break(&world, &server, player, pos.x, pos.y, pos.z)
        {
            event.cancelled = true;
            event.should_drop = false;
            event.exp = 0;
            return event;
        }

        // Global build protection: breaking is disabled outside boards.
        event.cancelled = true;
        event.should_drop = false;
        event.exp = 0;
        event
    }
}

pub struct InteractListener;
impl EventHandler<PlayerInteractEvent> for InteractListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<PlayerInteractEvent>,
    ) -> EventData<PlayerInteractEvent> {
        let is_left_click = event.action == InteractAction::LeftClickBlock
            || event.action == InteractAction::LeftClickAir;
        if is_left_click {
            if let Some(crate::sticks::HeldSpecial::FlagCycle) =
                crate::sticks::held_special(&event.player)
            {
                reset_flag_color(&event.player);
                event.cancelled = true;
            }
            return event;
        }
        let is_right_click = event.action == InteractAction::RightClickBlock
            || event.action == InteractAction::RightClickAir;
        if !is_right_click {
            return event;
        }
        // Starter sticks win over flagging.
        if let Some(spec) = crate::sticks::held_starter(&event.player) {
            crate::games::create_game(&event.player, spec);
            event.cancelled = true;
            return event;
        }
        // Lock toggle / flag color items also work on air clicks.
        if let Some(special) = crate::sticks::held_special(&event.player) {
            use crate::sticks::HeldSpecial;
            match special {
                HeldSpecial::LockToggle => {
                    let key = uuid_key(&event.player);
                    let line = crate::games::toggle_lock_report(key, None);
                    tell(&event.player, &line);
                    crate::sticks::refresh_utility_items(&event.player);
                }
                HeldSpecial::FlagCycle => {
                    let key = uuid_key(&event.player);
                    crate::state::cycle_flag_style(key);
                    let name = crate::state::flag_style_name(key);
                    tell(
                        &event.player,
                        &format!("Flag color: {name}. Left-click to reset to red."),
                    );
                    crate::sticks::refresh_utility_items(&event.player);
                }
            }
            event.cancelled = true;
            return event;
        }
        if event.action != InteractAction::RightClickBlock {
            return event;
        }
        let Some(clicked) = event.clicked_pos else {
            return event;
        };
        let world = event.player.get_world();
        let world_id = world.get_id();
        let pos = to_pos3(clicked);

        if let Some((id, idx)) = games::route(&world_id, pos) {
            games::click_right(&world, &event.player, id, idx);
            event.cancelled = true;
            return event;
        }

        if infinite::is_infinite_world(&world)
            && infinite::handle_interact(&world, &event.player, pos.x, pos.y, pos.z)
        {
            event.cancelled = true;
        }
        event
    }
}

pub struct PlaceListener;

impl EventHandler<BlockPlaceEvent> for PlaceListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<BlockPlaceEvent>,
    ) -> EventData<BlockPlaceEvent> {
        event.can_build = false;
        event.cancelled = true;
        event
    }
}

pub struct NoMobSpawnListener;

impl EventHandler<CreatureSpawnEvent> for NoMobSpawnListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<CreatureSpawnEvent>,
    ) -> EventData<CreatureSpawnEvent> {
        event.cancelled = true;
        event
    }
}

impl EventHandler<SpawnerSpawnEvent> for NoMobSpawnListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<SpawnerSpawnEvent>,
    ) -> EventData<SpawnerSpawnEvent> {
        event.cancelled = true;
        event
    }
}

impl EventHandler<TrialSpawnerSpawnEvent> for NoMobSpawnListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<TrialSpawnerSpawnEvent>,
    ) -> EventData<TrialSpawnerSpawnEvent> {
        event.cancelled = true;
        event
    }
}

/// Explosions can't crater boards (TNT, beds, anchors).
pub struct NoExplodeListener;

impl EventHandler<BlockExplodeEvent> for NoExplodeListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<BlockExplodeEvent>,
    ) -> EventData<BlockExplodeEvent> {
        event.cancelled = true;
        event
    }
}

impl EventHandler<EntityExplodeEvent> for NoExplodeListener {
    fn handle(
        &self,
        _server: Server,
        mut event: EventData<EntityExplodeEvent>,
    ) -> EventData<EntityExplodeEvent> {
        event.cancelled = true;
        event
    }
}
