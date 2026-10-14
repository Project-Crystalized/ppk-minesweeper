use minesweeper_lib::*;
use rand::{thread_rng, Rng};
use std::{
    collections::{BTreeMap, HashMap},
    time::SystemTime,
};
use valence::prelude::*;

fn main() {
    let mut map: HashMap<ChunkPos, [[(CellContent, CellState); 16]; 16]> = HashMap::new();
    let mut vec: Vec<(ChunkPos, [[(CellContent, CellState); 16]; 16])> = vec![];
    let mut tree: BTreeMap<ChunkPos, [[(CellContent, CellState); 16]; 16]> = BTreeMap::new();
    let mut rng = thread_rng();

    println!("filling map and vec with 1000 values\n");

    for _ in 0..1000 {
        let cpos = ChunkPos::new(rng.gen(), rng.gen());
        let mut array = [[(CellContent::Empty, CellState::Closed); 16]; 16];

        for x in 0..16 {
            for z in 0..16 {
                array[z][x].0 = match rng.gen_range(0..3) {
                    0 => CellContent::Bomb,
                    1 => CellContent::Empty,
                    2 => CellContent::Number(rng.gen()),
                    _ => unreachable!(),
                };
                array[z][x].1 = match rng.gen_range(0..3) {
                    0 => CellState::Opened,
                    1 => CellState::Closed,
                    2 => CellState::Flagged,
                    _ => unreachable!(),
                };
            }
        }
        vec.push((cpos, array));
        map.insert(cpos, array);
        tree.insert(cpos, array);
    }

    println!("measuring performance now\n");

    let search = vec.get(vec.len() / 2).unwrap().0;

    for _ in 0..10 {
        let time = SystemTime::now();
        let _ = map.get(&search).unwrap();
        let measured_map = time.elapsed().unwrap();
        println!("measured time for map was: {:?}", measured_map);
    }
    println!("\n");

    for _ in 0..10 {
        let time = SystemTime::now();
        let _ = tree.get(&search).unwrap();
        let measured_tree = time.elapsed().unwrap();
        println!("measured time for tree was: {:?}", measured_tree);
    }

    println!("\n");

    for _ in 0..10 {
        let time = SystemTime::now();
        let _ = vec.iter().find(|e| e.0 == search);
        let measured_vec = time.elapsed().unwrap();
        println!("measured time for vec was: {:?}", measured_vec);
    }
}
