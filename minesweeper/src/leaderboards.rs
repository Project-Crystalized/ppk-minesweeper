use std::ops::Add;

use mampfer_plugin::{postgres_wrapper::LeaderBoardType, PostgresWrapper};
use valence::{
    entity::{
        armor_stand::ArmorStandEntityBundle,
        entity::{CustomName, Flags, NameVisible},
    },
    prelude::*,
};

pub fn create_leaderbaords(
    query: &LeaderBoardType,
    pg: &mut PostgresWrapper,
    commands: &mut Commands,
    layer: Entity,
) {
    let solo_scores: [Option<(String, i32, i32)>; 10] = pg.get_solo_lb(query);
    let solo_pos = DVec3::new(6.0, 2.5, 27.0);
    let head: Text = format!("Solo {}D lb", query.dim).into();
    create_solo_lb(commands, layer, solo_scores, solo_pos, head);

    let team_scores: [Option<(String, i32, i32)>; 5] = pg.get_team_lb(query);
    let team_pos = DVec3::new(-13.0, 2.5, 27.0);
    let head: Text = format!("Teams {}D lb", query.dim).into();
    create_team_lb(commands, layer, team_scores, team_pos, head);

    let solo_scores_monthly: [Option<(String, i32, i32)>; 10] = pg.get_solo_lb_monthly(query);
    let solo_pos_month = DVec3::new(-24.0, 2.5, 9.0);
    let head: Text = format!("Monthly Solo {}D lb", query.dim).into();
    create_solo_lb(commands, layer, solo_scores_monthly, solo_pos_month, head);

    let team_scores_monthly: [Option<(String, i32, i32)>; 5] = pg.get_team_lb_monthly(query);
    let team_pos_month = DVec3::new(-24.0, 2.5, -19.0);
    let head: Text = format!("Monthly Teams {}D lb", query.dim).into();
    create_team_lb(commands, layer, team_scores_monthly, team_pos_month, head);
}

fn create_solo_lb(
    commands: &mut Commands,
    layer: Entity,
    scores: [Option<(String, i32, i32)>; 10],
    solo_pos: DVec3,
    head: Text,
) {
    create_armorstand(commands, head, layer, solo_pos);
    for (i, entry) in scores.iter().enumerate() {
        let i = i + 1;
        let t = if let Some((name, size, time)) = entry {
            format!(
                "{i}. {name}: {size}x{size} in {:.2} seconds",
                (*time as f64 / 20.0)
            )
            .bold()
        } else {
            "None".bold()
        };
        let pos = solo_pos.add(DVec3::new(0.0, -0.5 - (i as f64 * 0.25), 0.0));
        create_armorstand(commands, t, layer, pos);
    }
}

fn create_team_lb(
    commands: &mut Commands,
    layer: Entity,
    scores: [Option<(String, i32, i32)>; 5],
    team_pos: DVec3,
    head: Text,
) {
    create_armorstand(commands, head, layer, team_pos);
    for (i, entry) in scores.iter().enumerate() {
        let game_info = if let Some((_, size, time)) = entry {
            format!(
                "{}. {size}x{size} in {:.2} seconds by",
                i + 1,
                (*time as f64 / 20.0)
            )
            .bold()
        } else {
            "None".bold()
        };
        let participants_text = if let Some((participants, _, _)) = entry {
            format!("{}", participants).bold()
        } else {
            "None".bold()
        };

        let pos1 = team_pos.add(DVec3::new(0.0, -0.5 - (i as f64 * 0.5), 0.0));
        create_armorstand(commands, game_info, layer, pos1);
        let pos2 = team_pos.add(DVec3::new(0.0, -0.75 - (i as f64 * 0.5), 0.0));
        create_armorstand(commands, participants_text, layer, pos2);
    }
}

fn create_armorstand(commands: &mut Commands, t: Text, layer: Entity, pos: DVec3) {
    let mut f = Flags(0);
    f.set_invisible(true);
    commands.spawn(ArmorStandEntityBundle {
        layer: EntityLayerId(layer),
        position: Position(pos),
        entity_custom_name: CustomName(Some(t)),
        entity_name_visible: NameVisible(true),
        entity_flags: f,
        ..Default::default()
    });
}
