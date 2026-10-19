//! Pumpkin event handlers: join help, leave cleanup, left-click opens,
//! right-click flags.

use pumpkin_plugin_api::{
    events::{
        BlockBreakEvent, BlockExplodeEvent, BlockPlaceEvent, CreatureSpawnEvent,
        EntityExplodeEvent, EventData, EventHandler, InteractAction, PlayerInteractEvent,
        PlayerJoinEvent, PlayerLeaveEvent, SpawnerSpawnEvent, TrialSpawnerSpawnEvent,
    },
    Server,
};

use crate::{
    games,
    helpers::{tell, to_pos3, uuid_key},
    infinite,
    state,
};

pub struct JoinListener;

impl EventHandler<PlayerJoinEvent> for JoinListener {
    fn handle(&self, _server: Server, event: EventData<PlayerJoinEvent>) -> EventData<PlayerJoinEvent> {
        // Fresh minigame inventory: wipe first (stale kit pieces could sit in
        // armor/offhand slots), then lay out the kit in the main inventory
        // grid. Drag a starter to your hotbar and right-click to play.
        crate::sticks::give_fresh_kit(&event.player);
        crate::infinite::hydrate_session(&event.player);
        tell(
            &event.player,
            "Minesweeper is installed! Starter sticks are in your inventory - drag one to your hotbar and right-click to play, or /ms help. Building, breaking and mob spawning are disabled.",
        );
        event
    }
}

pub struct LeaveListener;

impl EventHandler<PlayerLeaveEvent> for LeaveListener {
    fn handle(
        &self,
        server: Server,
        event: EventData<PlayerLeaveEvent>,
    ) -> EventData<PlayerLeaveEvent> {
        let owner = uuid_key(&event.player);
        for game in state::remove_player_games(owner) {
            // Best effort: clear blocks in the game's world if still loaded.
            if let Some(world) = server.get_world_by_name(&game.world_id) {
                games::clear_blocks(&world, &game);
            }
        }
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
        let pos = to_pos3(event.block_pos);
        // Look diagnostics (debug level): server-side yaw/pitch at click time.
        tracing::debug!(
            "look: {} yaw={:.1} pitch={:.1} clicking {}, {}, {}",
            player.get_name(),
            player.get_yaw(),
            player.get_pitch(),
            pos.x,
            pos.y,
            pos.z
        );

        if let Some((id, idx)) = games::route(pos) {
            // Board clicks never break the underlying block, even when the
            // click itself is a no-op (flagged/open/locked/finished board).
            games::click_left(&world, &server, player, id, idx);
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

        // Global build protection: no breaking outside minesweeper boards.
        // (Board clicks are routed above; plugin block writes are unaffected.)
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
        let is_right_click = event.action == InteractAction::RightClickBlock
            || event.action == InteractAction::RightClickAir;
        if !is_right_click {
            return event;
        }
        // Starter sticks win over flagging (mirrors the old item-use flow).
        if let Some(spec) = crate::sticks::held_starter(&event.player) {
            crate::games::create_game(&event.player, spec);
            event.cancelled = true;
            return event;
        }
        // Lock toggle / flag-color cycle items (slot 43/44 equivalents).
        // They work on air clicks too, like the old InteractItemEvent flow.
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
                    let block = crate::state::cycle_flag_style(key);
                    let name = crate::state::flag_style_name(key);
                    tell(&event.player, &format!("Flag color: {name}."));
                    crate::sticks::refresh_utility_items(&event.player);
                    let _ = block;
                }
            }
            event.cancelled = true;
            return event;
        }
        if event.action != InteractAction::RightClickBlock {
            return event;
        }
        let Some(clicked) = event.clicked_pos else {
            tracing::debug!(
                "interact: RightClickBlock with no position (block={})",
                event.block
            );
            return event;
        };
        let world = event.player.get_world();
        let pos = to_pos3(clicked);
        tracing::debug!(
            "interact: click at {}, {}, {} (block={})",
            pos.x,
            pos.y,
            pos.z,
            event.block
        );

        if let Some((id, idx)) = games::route(pos) {
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

/// Global build protection: block placement is disabled server-wide.
/// (Plugin board creation/clearing uses direct world writes, unaffected.)
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

/// No mob spawning: natural, spawner, and trial-spawner births are cancelled.
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

/// Explosions can't crater minesweeper boards (no creepers spawn anyway,
/// but lit TNT / beds / anchors could still do damage).
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
