use valence::{
    nbt::{compound, Value},
    prelude::*,
};

pub enum StartItemType {
    Minesweeper20x20,
    Minesweeper30x30,
    Minesweeper50x50,
}

impl StartItemType {
    pub fn all_types() -> impl IntoIterator<Item = StartItemType> {
        [
            StartItemType::Minesweeper20x20,
            StartItemType::Minesweeper30x30,
            StartItemType::Minesweeper50x50,
        ]
    }

    pub fn get_start_item_type(item: &ItemStack) -> Option<StartItemType> {
        if item.item != ItemKind::Stick {
            return None;
        }
        if let Value::Compound(c) = item.nbt.clone()?.get("display")? {
            if let Value::String(s) = c.get("Name")? {
                match s.as_str() {
                    "\"Minesweeper20x20\"" => Some(StartItemType::Minesweeper20x20),
                    "\"Minesweeper30x30\"" => Some(StartItemType::Minesweeper30x30),
                    "\"Minesweeper50x50\"" => Some(StartItemType::Minesweeper50x50),
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
            StartItemType::Minesweeper20x20 => ItemStack::new(
                ItemKind::Stick,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Minesweeper20x20\"",
                    }
                }),
            ),
            StartItemType::Minesweeper30x30 => ItemStack::new(
                ItemKind::Stick,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Minesweeper30x30\"",
                    }
                }),
            ),
            StartItemType::Minesweeper50x50 => ItemStack::new(
                ItemKind::Stick,
                1,
                Some(compound! {
                    "display" => compound! {
                        "Name" => "\"Minesweeper50x50\"",
                    }
                }),
            ),
        }
    }
}
