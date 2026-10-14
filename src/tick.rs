//! Per-tick driver: a fast 1-tick counter for cooldowns plus the existing
//! per-second work (game timers, action bars, lock cleanup).

use pumpkin_plugin_api::{scheduler::SchedulerExt, Context, Server};

use crate::{
    games,
    helpers::{action_bar, uuid_key},
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
        let mine = state::player_games(key);
        if let Some(first) = mine.first() {
            if let Some(line) = games::status_line(*first) {
                let extra = if mine.len() > 1 {
                    format!(" (+{} more, /ms list)", mine.len() - 1)
                } else {
                    String::new()
                };
                action_bar(&player, &format!("{line}{extra}"));
            }
        }
    }
}
