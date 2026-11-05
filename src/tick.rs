//! Per-tick driver: cooldowns, board timers every tick; the hotbar
//! proximity swap on a 5-tick cadence; the sidebar/status per-second work
//! runs on the 20-tick cadence.

use pumpkin_plugin_api::{Context, Server, scheduler::SchedulerExt};

use crate::{helpers::uuid_key, state};

pub fn start_tick_task(context: &Context) {
    context.schedule_repeating_task(1, 1, |server: Server| {
        tick(server);
    });
}

fn tick(server: Server) {
    let now = state::tick_advance();
    crate::helpers::tick_open_sounds(&server, now);
    state::advance_game_timers();
    crate::sudoku::advance_game_timers();
    crate::sudoku::spawn_grid_particles(&server, now);
    if crate::leaderboard::take_refresh_request() {
        crate::leaderboard::refresh(&server);
    }

    let per_second = now % 20 == 0;
    let hotbar_tick = now % crate::hotbar::SYNC_INTERVAL_TICKS == 0;
    if per_second {
        state::prune_flag_locks(now);

        // Leaderboard cadence, plus once shortly after load.
        if now == 40 || now % crate::leaderboard::REFRESH_TICKS == 0 {
            crate::leaderboard::refresh(&server);
        }
    }

    let players = server.get_all_players();
    let online = players.len();
    let games = state::all_games_detailed();
    let sudoku_games = crate::sudoku::all_summaries();
    for player in players {
        let key = uuid_key(&player);
        let world = player.get_world();
        let infinite = crate::infinite::is_infinite_world(&world);
        if per_second && infinite {
            // Lifetime time in the endless world (1 tick loop = 1 second).
            state::with_session_mut(key, |s| {
                s.playtime_ticks = s.playtime_ticks.saturating_add(20);
            });
            // Flush totals every 30 s so idling playtime survives a crash.
            if now % 600 == 0 {
                crate::infinite::persist_session_for(key, &player);
            }
        }
        // Proximity scan on the 5-tick hotbar cadence: whichever board the
        // player is closest to within reach decides what their hotbar
        // becomes (Sudoku digits vs minesweeper lock/flag items); no board
        // in reach restores the saved kit. (The one-second sidebar pass
        // below always implies a hotbar tick, so it sees fresh distances.)
        let (px, py, pz) = player.get_position();
        let mut best_mine: Option<(f64, u32)> = None;
        let mut best_sudoku: Option<f64> = None;
        // Distance to the minesweeper board's bounding box: the utility
        // hotbar should engage while the player is anywhere on/inside a
        // board, not just within radius of its center.
        let mut mine_edge_d2: Option<f64> = None;
        let mut mine_first: Option<u32> = None;
        let mut sudoku_first: Option<u32> = None;
        let mut game_count = 0usize;
        let mut best_sudoku_id: Option<u32> = None;
        if !infinite && hotbar_tick {
            for g in &games {
                if g.owner == key {
                    game_count += 1;
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
                if best_mine.is_none_or(|(bd, _)| d2 < bd) {
                    best_mine = Some((d2, g.id));
                }
                // Per-axis distance to the box around the board center. The
                // vertical half-extent is the full dimension so boards with
                // layers above their center still count.
                let half = (g.dim / 2) as f64;
                let ex = (px - g.center.x as f64).abs() - half;
                let ey = (py - g.center.y as f64).abs() - g.dim as f64;
                let ez = (pz - g.center.z as f64).abs() - half;
                let edge_d2 = ex.max(0.0).powi(2) + ey.max(0.0).powi(2) + ez.max(0.0).powi(2);
                if mine_edge_d2.is_none_or(|bd| edge_d2 < bd) {
                    mine_edge_d2 = Some(edge_d2);
                }
            }
            for s in &sudoku_games {
                if s.owner == key {
                    game_count += 1;
                    sudoku_first.get_or_insert(s.id);
                }
                if world.get_id() != s.world_id {
                    continue;
                }
                let dx = s.center.x as f64 - px;
                let dy = s.center.y as f64 - py;
                let dz = s.center.z as f64 - pz;
                let d2 = dx * dx + dy * dy + dz * dz;
                if best_sudoku.is_none_or(|bd| d2 < bd) {
                    best_sudoku = Some(d2);
                    best_sudoku_id = Some(s.id);
                }
            }
        }
        // Hotbar mode: re-evaluate on the 5-tick cadence so the swap feels
        // instant while the tick loop stays cheap.
        if hotbar_tick {
            let hotbar_mode = crate::hotbar::choose_mode(
                crate::hotbar::active_mode(&player),
                mine_edge_d2,
                best_sudoku,
            );
            crate::hotbar::sync(&player, hotbar_mode);
        }

        if !per_second {
            continue;
        }
        // Sidebar focus: nearest board in this world; fall back to the
        // player's own first minesweeper/Sudoku board (only runs on the
        // one-second cadence, so cloning the summary here is fine).
        let best_sudoku_summary =
            best_sudoku_id.and_then(|sid| sudoku_games.iter().find(|s| s.id == sid));
        let focus = match (best_mine, best_sudoku_summary) {
            (Some((dm, id)), Some(s)) => {
                if best_sudoku.unwrap_or(f64::MAX) < dm {
                    Some(crate::sidebar::Focus::Sudoku(s.clone()))
                } else {
                    Some(crate::sidebar::Focus::Minesweeper(id))
                }
            }
            (Some((_, id)), None) => Some(crate::sidebar::Focus::Minesweeper(id)),
            (None, Some(s)) => Some(crate::sidebar::Focus::Sudoku(s.clone())),
            (None, None) => mine_first
                .map(crate::sidebar::Focus::Minesweeper)
                .or_else(|| {
                    sudoku_first
                        .and_then(|id| sudoku_games.iter().find(|s| s.id == id).cloned())
                        .map(crate::sidebar::Focus::Sudoku)
                }),
        };
        crate::sidebar::show(
            &player,
            crate::sidebar::lines(&player, focus, game_count, online, infinite),
        );
    }
}
