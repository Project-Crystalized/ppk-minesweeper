//! Per-tick driver: a fast 1-tick counter for cooldowns plus the existing
//! per-second work (game timers, sidebar status, lock cleanup).

use pumpkin_plugin_api::{scheduler::SchedulerExt, Context, Server};

use crate::{
    helpers::uuid_key,
    state,
};

pub fn start_tick_task(context: &Context) {
    context.schedule_repeating_task(1, 1, |server: Server| {
        tick(server);
    });
}

fn tick(server: Server) {
    let now = state::tick_advance();
    if now % 20 != 0 {
        return;
    }

    // Advance game timers (short critical sections).
    let ids: Vec<u32> = state::all_games_snapshot()
        .iter()
        .map(|(id, _, _, _, _, _, _)| *id)
        .collect();
    for id in ids {
        state::with_game_mut(id, |g| {
            if !g.is_over {
                g.comp_time_ticks += 20;
            }
        });
    }
    state::prune_flag_locks(now);

    // Leaderboards refresh on their own slow cadence (plus once shortly
    // after load so boards appear without waiting a full cycle).
    if now == 40 || now % crate::leaderboard::REFRESH_TICKS == 0 {
        crate::leaderboard::refresh(&server);
    }

    for player in server.get_all_players() {
        let key = uuid_key(&player);
        // Tripwire for the first-click pitch snap: log if anyone's view is
        // ever near-vertical server-side.
        let pitch = player.get_pitch();
        if pitch.abs() > 80.0 {
            tracing::info!(
                "look-tripwire: {} yaw={:.1} pitch={:.1}",
                player.get_name(),
                player.get_yaw(),
                pitch
            );
        }
        // Show the nearest board (any owner), like the old server's
        // `show_action_bar` which picked the closest `ActionState.center`.
        // Falls back to the player's own first game when worlds differ.
        let (px, py, pz) = player.get_position();
        let mut best: Option<(f64, u32, usize)> = None;
        let mut mine_first: Option<u32> = None;
        let mut mine_count = 0usize;
        for g in state::all_games_detailed() {
            if g.owner == key {
                mine_count += 1;
                mine_first.get_or_insert(g.id);
            }
            // Only boards in the player's current world compete by distance.
            let world = player.get_world();
            if world.get_id() != g.world_id {
                continue;
            }
            let dx = g.center.x as f64 - px;
            let dy = g.center.y as f64 - py;
            let dz = g.center.z as f64 - pz;
            let d2 = dx * dx + dy * dy + dz * dz;
            if best.is_none_or(|(bd, _, _)| d2 < bd) {
                best = Some((d2, g.id, 0));
            }
        }
        let show_id = best.map(|(_, id, _)| id).or(mine_first);
        match show_id {
            Some(id) => crate::sidebar::show(
                &player,
                crate::sidebar::status_lines(id, mine_count),
            ),
            None => crate::sidebar::clear(&player),
        }
    }
}
