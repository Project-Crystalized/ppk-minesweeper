# ppk-minesweeper

A Pumpkin (Rust/WASM) plugin port of the old Valence-based minesweeper server
in `minesweeper/`. Valence is unmaintained, so the game now runs as a plugin
inside Pumpkin instead of as its own server.

## Commands (`/ms`)

| Command | Effect |
|---|---|
| `/ms help` | Show help |
| `/ms 2d20` / `2d30` / `2d50` | Start a flat board at the first free layer above your feet |
| `/ms 2d <size>` | Start a flat board with custom dimension (2+) |
| `/ms 3d5` / `3d7` / `3d10` | Start a 3D cube (3-block spacing) |
| `/ms 3d <size>` | Start a 3D cube with custom dimension (2+) |
| `/ms lock [id]` | Lock/unlock your games (or right-click the horse-armor item) |
| `/ms flag` | Cycle flag wool color (or right-click the wool item) |
| `/ms clear` | Remove all your games (blocks cleared in their own worlds) |
| `/ms list` | List your games (kind, dim, locks, helpers) |
| `/ms kit` | Re-give starter sticks + lock/flag items |
| `/ms stats` | Your box record per size (games, wins, best) + infinite totals |
| `/ms top` | Fastest wins + best infinite streaks (from SQLite) |
| `/ms infinite` | Teleport to the endless `ms_infinite` world |

Left-click opens a cell, right-click toggles a flag with your own color.
Flagged/open cells ignore left-clicks (flagged bombs can't explode).
Locked games only respond to their owner; clicking your own
finished board clears it. Max 3 box games per player. A per-second action bar
shows the nearest board with time / bombs / flags. Infinite mode tracks
streaks with color tiers and 500/1000 broadcasts.

## Persistence (SQLite)

Finished box games (win or loss) and infinite-mode sessions are recorded in
a single SQLite file, following the `LsDatabase` conventions (one file,
`CREATE TABLE IF NOT EXISTS` + additive migrations, fail-soft: a broken DB
disables recording, the game keeps running):

- `MsBoxGames` / `MsBoxPlayers`: every finished game (dim, mode, time, won,
  owner + helpers, timestamp) — mirrors the old `insert_minesweeper`.
- `MsInfiniteStats`: per-player best streak + totals (upsert, write-through
  per click) — mirrors the old infinite stats. Totals are restored on join.

The WASM sandbox only exposes the plugin's own data folder (the manifest
requests `fs.write.data`), so the file lives at
`data/minesweeper.sql` inside the sandbox, i.e.
`<server>/plugins/data/<plugin>/minesweeper.sql` on the host.
`~/databases/` is NOT reachable from inside the plugin; to keep the file
there, bind-mount it over the plugin data folder server-side
(`mount --bind ~/databases/ms <server>/plugins/data/<plugin>`), then read it
with `sqlite3` like the other game DBs.

## Build

Prerequisite for the SQLite dependency: a wasi-sdk for the
`wasm32-wasip2` C build (`rusqlite/bundled` compiles the amalgamation).
Repo defaults point at `~/.local/wasi-sdk` (override via environment):

```sh
# one-time setup (or adjust .cargo/config.toml paths)
export CC_wasm32_wasip2="$HOME/.local/wasi-sdk/bin/clang"
export CFLAGS_wasm32_wasip2="--sysroot=$HOME/.local/wasi-sdk/share/wasi-sysroot"

cargo check
cargo test --target x86_64-unknown-linux-gnu
cargo build --target wasm32-wasip2 --release
```

Copy `target/wasm32-wasip2/release/ppk_minesweeper.wasm` into Pumpkin's
plugins folder. The server must grant the requested `fs.write.data`
permission (unless blocked in server config, the manifest request applies).

## Porting notes (Valence → Pumpkin)

- No ECS/`App`: games live in `Mutex<HashMap>` globals (`src/state.rs`).
- `ChunkLayer::set_block` → `World::set_block_by_name`; `DiggingEvent`
  re-injection for flood fill → direct iterative flood fill in the handler.
- `InteractBlockEvent`/`DiggingEvent` → cancellable `PlayerInteractEvent`
  (right-click) / `BlockBreakEvent` (left-click).
- Stick start items → `/ms` subcommands. Per-player `LayerBundle`s (infinite)
  → one shared `ms_infinite` world with a `ChunkGenerator` painting the
  covered board; content is a deterministic hash of `(x, z)`, not OpenSimplex.
- `PostgresWrapper` / armor-stand leaderboards → SQLite records (`src/db.rs`)
  with `/ms stats` + `/ms top` instead of holograms.
- Pure board logic in `src/core.rs` has unit tests; no engine deps.
- No extra crates: board RNG is an internal splitmix64, so `wasm32-wasip2`
  builds stay dependency-free.

## Runtime checklist (needs a live Pumpkin server)

- [ ] `/ms 2d20` builds a moss board above the player and reports coords
- [ ] Left-click opens / flood-fills; first click never a bomb
- [ ] Right-click flags/unflags; win and bomb-reveal paths work
- [ ] `/ms infinite` creates/teleports to `ms_infinite` with moss terrain
- [ ] Finished games show up in `data/minesweeper.sql` (`sqlite3` CLI);
  `/ms stats` + `/ms top` read them back; infinite totals survive restart
- [ ] Empty `register_command` permission (`""`) is accepted by the host
