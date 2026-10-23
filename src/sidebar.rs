use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use pumpkin_plugin_api::{
    text::TextComponent,
    wit::pumpkin::plugin::{
        common::NamedColor,
        player::JavaPlayer,
        scoreboard as sb,
    },
    Player,
};

use crate::{
    helpers::uuid_key,
    state,
};

const MAX_LINES: usize = 10;
const OBJECTIVE: &str = "ms_status";

fn live_players() -> &'static Mutex<HashSet<state::UuidKey>> {
    static MAP: OnceLock<Mutex<HashSet<state::UuidKey>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashSet::new()))
}

fn entry_name(i: usize) -> String {
    "§0".repeat(i + 1)
}

fn line_team(i: usize) -> String {
    format!("ms_l{i}")
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

fn install(jp: &JavaPlayer) {
    let sb_handle = jp.get_scoreboard();
    sb_handle.add_objective(
        OBJECTIVE,
        TextComponent::from_legacy_string("§dCrystalized"),
        sb::RenderType::Integer,
        Some(sb::NumberFormat::Blank),
    );

    for i in 0..MAX_LINES {
        let name = line_team(i);
        if sb_handle.get_teams().contains(&name) {
            continue;
        }
        sb_handle.create_team(&name, team_settings(""));
        sb_handle.add_player_to_team(&name, entry_name(i).as_str());
    }

    sb_handle.set_display_slot(sb::DisplaySlot::Sidebar, OBJECTIVE);
}

pub fn show(player: &Player, lines: Vec<String>) {
    let Some(jp) = player.as_java() else { return };
    let fresh = live_players()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(uuid_key(player));
    if fresh {
        install(&jp);
    }

    let count = lines.len().min(MAX_LINES);
    let sb_handle = jp.get_scoreboard();
    for i in 0..MAX_LINES {
        let entry = entry_name(i);
        if i < count {
            sb_handle.update_score(
                &entry,
                OBJECTIVE,
                (MAX_LINES - 1 - i) as i32,
                Some(sb::NumberFormat::Blank),
            );
            sb_handle.update_team(&line_team(i), team_settings(&lines[i]));
        } else {
            sb_handle.remove_score(&entry, OBJECTIVE);
        }
    }
}

pub fn clear(player: &Player) {
    let removed = live_players()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&uuid_key(player));
    if removed {
        if let Some(jp) = player.as_java() {
            jp.reset_scoreboard();
        }
    }
}

pub fn lines(
    player: &Player,
    show_id: Option<u32>,
    mine_count: usize,
    online: usize,
    infinite: bool,
) -> Vec<String> {
    let mut lines = vec![
        " ".to_string(),
        "§bMinesweeper".to_string(),
        format!("§aPlayers: §f{online}"),
        " ".to_string(),
    ];

    if infinite {
        lines.extend(infinite_lines(player));
    } else if let Some(game) = show_id.and_then(|id| game_lines(id, mine_count)) {
        lines.extend(game);
    } else {
        lines.extend(hint_lines());
    }

    lines.push(" ".to_string());
    lines.push("§6crystalized.cc §7(minesweeper)".to_string());
    lines
}

fn hint_lines() -> Vec<String> {
    vec![
        "§7No board nearby".to_string(),
        "§7Right-click a stick to play".to_string(),
    ]
}

fn game_lines(id: u32, mine_count: usize) -> Option<Vec<String>> {
    state::with_game(id, |g| {
        let mut title = format!("§e{} §8#{}", spec_label(g.kind, g.dim), id);
        if g.locked {
            title.push_str(" §7[locked]");
        }
        if mine_count > 1 {
            title.push_str(&format!(" §8+{} more", mine_count - 1));
        }

        let mut lines = vec![
            title,
            format!("§bTime: §f{}", time_text(g.comp_time_ticks)),
            format!(
                "§cBombs: §f{} §8| §6Flags: §f{}",
                g.bomb_total(),
                g.flag_count()
            ),
        ];
        if g.is_over {
            lines.push(if g.is_won {
                "§aWON! §7Click the board to clear".to_string()
            } else {
                "§cBOOM! §7Click the board to clear".to_string()
            });
        } else if !g.helpers.is_empty() {
            lines.push(format!("§7Helpers: §f{}", g.helpers.len()));
        }
        lines
    })
}

fn infinite_lines(player: &Player) -> Vec<String> {
    let s = state::session(uuid_key(player)).unwrap_or_default();
    vec![
        "§dInfinite".to_string(),
        format!("§eStreak: §f{}", s.current_streak),
        format!(
            "§7Best: §f{} §8| §7Opened: §f{}",
            s.top_streak, s.total_opened
        ),
        format!("§cBombs hit: §f{}", s.bombs_clicked),
    ]
}

fn spec_label(kind: state::GameKind, dim: usize) -> String {
    match kind {
        state::GameKind::TwoD => format!("2D {dim}x{dim}"),
        state::GameKind::ThreeD => format!("3D {dim}x{dim}x{dim}"),
    }
}

fn time_text(ticks: u64) -> String {
    let secs = ticks / 20;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}
