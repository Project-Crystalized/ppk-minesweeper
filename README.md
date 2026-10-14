# ppk-minesweeper

A Pumpkin (Rust/WASM) plugin port of the old Valence-based minesweeper server
in `minesweeper/`. Valence is unmaintained, so the game now runs as a plugin
inside Pumpkin instead of as its own server.

## Commands (`/ms`)

| Command | Effect |
|---|---|
| `/ms help` | Show help |
| `/ms 2d20` / `2d30` / `2d50` | Start a flat board at the first free layer above your feet |
| `/ms 3d5` / `3d7` / `3d10` | Start a 3D cube (3-block spacing) |
| `/ms clear` | Remove all your games (blocks cleared to air) |
| `/ms list` | List your games |
| `/ms infinite` | Teleport to the endless `ms_infinite` world |

Left-click opens a cell, right-click toggles a flag. Clicking your own
finished board clears it. Max 3 box games per player. A per-second action bar
shows time / bombs / flags. Infinite mode tracks streaks in memory.

## Build

```sh
cargo check
cargo test --target x86_64-unknown-linux-gnu
cargo build --target wasm32-wasip2 --release
```

Copy `target/wasm32-wasip2/release/ppk_minesweeper.wasm` into Pumpkin's
plugins folder.

## Porting notes (Valence → Pumpkin)

- No ECS/`App`: games live in `Mutex<HashMap>` globals (`src/state.rs`).
- `ChunkLayer::set_block` → `World::set_block_by_name`; `DiggingEvent`
  re-injection for flood fill → direct iterative flood fill in the handler.
- `InteractBlockEvent`/`DiggingEvent` → cancellable `PlayerInteractEvent`
  (right-click) / `BlockBreakEvent` (left-click).
- Stick start items → `/ms` subcommands. Per-player `LayerBundle`s (infinite)
  → one shared `ms_infinite` world with a `ChunkGenerator` painting the
  covered board; content is a deterministic hash of `(x, z)`, not OpenSimplex.
- `PostgresWrapper` / armor-stand leaderboards dropped (no persistence in v1).
- Pure board logic in `src/core.rs` has unit tests; no engine deps.
- No extra crates: board RNG is an internal splitmix64, so `wasm32-wasip2`
  builds stay dependency-free.

## Runtime checklist (needs a live Pumpkin server)

- [ ] `/ms 2d20` builds a moss board above the player and reports coords
- [ ] Left-click opens / flood-fills; first click never a bomb
- [ ] Right-click flags/unflags; win and bomb-reveal paths work
- [ ] `/ms infinite` creates/teleports to `ms_infinite` with moss terrain
- [ ] Empty `register_command` permission (`""`) is accepted by the host
