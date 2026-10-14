use valence::{
    nbt::{compound, Value},
    prelude::*,
};

pub enum StartItemType {
    Minesweeper3D5x5,
    Minesweeper3D7x7,
    Minesweeper3D10x10,
}

impl StartItemType {
    pub fn all_types() -> impl IntoIterator<Item = StartItemType> {
        [
            StartItemType::Minesweeper3D5x5,
            StartItemType::Minesweeper3D7x7,
            StartItemType::Minesweeper3D10x10,
        ]
    }

    pub fn get_start_item_type(item: &ItemStack) -> Option<StartItemType> {
        if item.item != ItemKind::Stick {
            return None;
        }
        if let Value::Compound(c) = item.nbt.clone()?.get("display")? {
            if let Value::String(s) = c.get("Name")? {
                match s.as_str() {
                    "\"Minesweeper3d5x5\"" => Some(StartItemType::Minesweeper3D5x5),
                    "\"Minesweeper3d7x7\"" => Some(StartItemType::Minesweeper3D7x7),
                    "\"Minesweeper3d10x10\"" => Some(StartItemType::Minesweeper3D10x10),
                    _ => None,
                }
            } else {
                return None;
            }
        } else {
            return None;
        }
        //TODO test this
    }

    pub fn create_start_item(item_type: StartItemType) -> ItemStack {
        match item_type {
            StartItemType::Minesweeper3D5x5 => ItemStack::new(
                ItemKind::Stick,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Minesweeper3d5x5\"",
                    }
                }),
            ),
            StartItemType::Minesweeper3D7x7 => ItemStack::new(
                ItemKind::Stick,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Minesweeper3d7x7\"",
                    }
                }),
            ),
            StartItemType::Minesweeper3D10x10 => ItemStack::new(
                ItemKind::Stick,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Minesweeper3d10x10\"",
                    }
                }),
            ),
        }
    }
}
