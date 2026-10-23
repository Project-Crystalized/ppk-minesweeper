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

    // Advance game timers (short critical sections). A disconnected owner's
    // timer is paused: the board stays, but time spent offline does not count.
    let ids: Vec<(u32, state::UuidKey)> = state::all_games_snapshot()
        .iter()
        .map(|(id, owner, _, _, _, _, _)| (*id, *owner))
        .collect();
    for (id, owner) in ids {
        if !state::is_online(owner) {
            continue;
        }
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

    let players = server.get_all_players();
    let online = players.len();
    let games = state::all_games_detailed();
    for player in players {
        let key = uuid_key(&player);
        let world = player.get_world();
        let infinite = crate::infinite::is_infinite_world(&world);
        if infinite {
            // Lifetime time in the endless world (1 tick loop = 1 second).
            state::with_session_mut(key, |s| {
                s.playtime_ticks = s.playtime_ticks.saturating_add(20);
            });
            // Flush totals every 30 s so idling playtime survives a crash.
            if now % 600 == 0 {
                crate::infinite::persist_session_for(key, &player);
            }
        }
        // Show the nearest board (any owner), like the old server's
        // `show_action_bar` which picked the closest `ActionState.center`.
        // Falls back to the player's own first game when worlds differ.
        let (px, py, pz) = player.get_position();
        let mut best: Option<(f64, u32)> = None;
        let mut mine_first: Option<u32> = None;
        let mut mine_count = 0usize;
        if !infinite {
            for g in &games {
                if g.owner == key {
                    mine_count += 1;
                    mine_first.get_or_insert(g.id);
                }
                // Only boards in the player's current world compete by distance.
                if world.get_id() != g.world_id {
                    continue;
                }
                let dx = g.center.x as f64 - px;
                let dy = g.center.y as f64 - py;
                let dz = g.center.z as f64 - pz;
                let d2 = dx * dx + dy * dy + dz * dz;
                if best.is_none_or(|(bd, _)| d2 < bd) {
                    best = Some((d2, g.id));
                }
            }
        }
        let show_id = best.map(|(_, id)| id).or(mine_first);
        crate::sidebar::show(
            &player,
            crate::sidebar::lines(&player, show_id, mine_count, online, infinite),
        );
    }
}
