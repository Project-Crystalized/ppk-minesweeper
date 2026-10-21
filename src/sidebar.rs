//! Sidebar scoreboard replacing the old action-bar game status.
//!
//! Same lines the action bar used to carry (time / bombs / flags / state),
//! rendered as a per-player sidebar so the bar is free for cross-server
//! things (queue status from velocity).
//!
//! Layout mirrors `ScoreboardManager` in the lobby plugin: one dummy
//! objective on the sidebar slot, lines as fake-entry scores that live in
//! per-line teams so the text (team prefix) is the only thing that changes
//! between ticks. Entry names are made of bare `§` color codes so nothing
//! leaks as visible text next to the prefix.
//!
//! Java-only: `get_scoreboard` lives on the Java player handle; Bedrock
//! players simply get no custom sidebar.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use pumpkin_plugin_api::{
    text::TextComponent,
    wit::pumpkin::plugin::{
        common::NamedColor,
        scoreboard as sb,
    },
    Player,
};

use crate::{
    helpers::uuid_key,
    state,
};

/// Stable fake entries available for lines.
const MAX_LINES: usize = 10;

/// Per-player refresh state: `Some(active_lines)` while the custom sidebar
/// is installed. Cleared entries lose their score (vanilla prunes sidebar
/// rows without a score), so the count is tracked to re-assert scores when
/// a line comes back.
fn live_players() -> &'static Mutex<HashMap<state::UuidKey, usize>> {
    static MAP: OnceLock<Mutex<HashMap<state::UuidKey, usize>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Invisible fake-entry name for line `i` (only `§` color codes, so the
/// sidebar renders prefix-only). Uniqueness comes from prefix length.
fn entry_name(i: usize) -> String {
    const PALETTE: [&str; 13] = [
        "§0", "§1", "§2", "§3", "§4", "§5", "§6", "§7", "§8", "§9", "§a", "§b", "§c",
    ];
    let n = (i + 1).min(PALETTE.len());
    PALETTE[..n].concat()
}

fn line_team(i: usize) -> String {
    format!("ms_l{i}")
}

fn objective() -> String {
    "ms_status".to_string()
}

fn team_settings(prefix: &str) -> sb::TeamSettings {
    sb::TeamSettings {
        display_name: TextComponent::text("ms"),
        friendly_fire: false,
        see_friendly_invisibles: false,
        nametag_visibility: sb::NametagVisibility::Never,
        collision_rule: sb::CollisionRule::Never,
        color: NamedColor::White,
        prefix: TextComponent::from_legacy_string(prefix),
        suffix: TextComponent::text(""),
    }
}

/// One-time installation of objective + line teams + fixed scores. Host
/// calls are fire-and-forget (no `Result`), and the player scoreboard
/// objective / team adds are idempotent warn-no-ops when they already exist.
fn install(jp: &pumpkin_plugin_api::wit::pumpkin::plugin::player::JavaPlayer) -> bool {
    let sb_handle = jp.get_scoreboard();
    sb_handle.add_objective(
        &objective(),
        TextComponent::from_legacy_string("§dMinesweeper"),
        sb::RenderType::Integer,
        Some(sb::NumberFormat::Blank), // hide the score numbers, text is all prefix
    );

    for i in 0..MAX_LINES {
        let name = line_team(i);
        if sb_handle.get_teams().contains(&name) {
            continue;
        }
        sb_handle.create_team(&name, team_settings(""));
        sb_handle.add_player_to_team(&name, entry_name(i).as_str());
        // Descending scores so line 0 sits on top of the sidebar.
        sb_handle.update_score(
            entry_name(i).as_str(),
            &objective(),
            (MAX_LINES - 1 - i) as i32,
            Some(sb::NumberFormat::Blank),
        );
    }

    sb_handle.set_display_slot(sb::DisplaySlot::Sidebar, &objective());
    true
}

/// Render the sidebar with `lines[0]` on top. Shrunk lines lose their
/// scores; re-grown lines get them back (vanilla prunes sidebar rows once
/// their score entry is gone, so the score must be re-asserted).
pub fn show(player: &Player, lines: Vec<String>) {
    let Some(jp) = player.as_java() else { return };
    let key = uuid_key(player);
    let mut live = live_players()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let fresh = !live.contains_key(&key);
    if fresh && !install(&jp) {
        return;
    }
    let sb_handle = jp.get_scoreboard();
    let old_count = live.get(&key).copied().unwrap_or(0);

    let count = lines.len().min(MAX_LINES);
    for i in 0..MAX_LINES {
        if i >= count {
            if old_count > i {
                sb_handle.remove_score(&entry_name(i), &objective());
            }
            continue;
        }
        if i >= old_count && fresh {
            // line had no score on an older install: restore it
            sb_handle.update_score(
                entry_name(i).as_str(),
                &objective(),
                (MAX_LINES - 1 - i) as i32,
                Some(sb::NumberFormat::Blank),
            );
        }
        sb_handle.update_team(&line_team(i), team_settings(&lines[i]));
    }
    live.insert(key, count);
}

/// Remove the sidebar entirely (no board to show).
pub fn clear(player: &Player) {
    let mut live = live_players()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if live.remove(&uuid_key(player)).is_none() {
        return;
    }
    if let Some(jp) = player.as_java() {
        jp.reset_scoreboard();
    }
}

/// Status lines for `id`, state extras included (over/locked/more).
/// Palette mirrors the old `status_component` action bar.
pub fn status_lines(id: u32, mine_count: usize) -> Vec<String> {
    let mut lines = state::with_game(id, |g| {
        let mut lines = Vec::new();
        if !g.is_over {
            lines.push(format!("§bTime: {}", g.comp_time_ticks / 20));
        }
        lines.push(format!("§cBombs: {}", g.bomb_total()));
        lines.push(format!("§6Flags: {}", g.flag_count()));
        if g.is_over {
            lines.push(if g.is_won {
                "§aWON!".to_string()
            } else {
                "§cBOOM!".to_string()
            });
        }
        if g.locked {
            lines.push("§7[Locked]".to_string());
        }
        lines
    })
    .unwrap_or_default();
    if mine_count > 1 {
        lines.push(format!("§7+{} more, /ms list", mine_count - 1));
    }
    lines
}
