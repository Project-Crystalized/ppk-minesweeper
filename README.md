# ppk-minesweeper

A Pumpkin (Rust/WASM) plugin port of the old Valence-based minesweeper server
in `minesweeper/`. Valence is unmaintained, so the game now runs as a plugin
inside Pumpkin instead of as its own server.

## Commands (`/ms`)

| Command | Effect |
|---|---|
| `/ms help` | Show help |
| `/ms 2d <size>` | Start a flat board with custom dimension (2+) |
| `/ms 3d <size>` | Start a 3D cube with custom dimension (2+) |
| `/ms lock [id]` | Lock/unlock your games (or right-click the horse-armor item) |
| `/ms flag` | Cycle flag wool color (or right-click the wool item) |
| `/ms clear` | Remove all your games (blocks cleared in their own worlds) |
| `/ms list` | List your games (kind, dim, locks, helpers) |
| `/ms kit` | Re-give starter sticks + lock/flag items |
| `/ms stats` | Per-size record (games, wins, best/avg) + co-op + infinite totals |
| `/ms top` | Fastest wins + best infinite streaks (from SQLite) |
| `/ms infinite` | Teleport to the endless `ms_infinite` world |

Left-click opens a cell, right-click toggles a flag with your own color.
Flagged/open cells ignore left-clicks (flagged bombs can't explode).
Locked games only respond to their owner; clicking your own
finished board clears it. Max 3 active games per player. A per-second action bar
shows the nearest board with time / bombs / flags. Infinite mode tracks
streaks with color tiers and 500/1000 broadcasts.

## Persistence (SQLite)

Finished sized games (win, loss or abandoned) and infinite-mode lifetime totals
are recorded in a single SQLite file, following the `LsDatabase` conventions
(one file, `CREATE TABLE IF NOT EXISTS`, fail-soft: a broken DB disables
recording, the game keeps running):

- `MsPlayers`: identity row per player (UUID, current name, a short name
  history, first/last seen, lifetime online playtime in ticks). Names on
  leaderboards are resolved from here, so renames show up immediately.
- `MsGames`: one row per finished or abandoned game: `kind` (0 = 2D,
  1 = 3D), `dim`, final board `seed`, `time_ticks`, `outcome`
  (0 = loss, 1 = win, 2 = abandoned), bombs, flags, flags placed, cells
  opened, left-clicks, first-click reshuffles, helper count, locked flag,
  epoch end time and board origin `x/y/z`. The world a game was played in is
  deliberately not stored.
- `MsGamePlayers`: game roster (`is_owner` 1/0), which powers co-op credit.
- `MsInfiniteStats`: lifetime infinite totals per player (best streak, cells
  opened, bombs clicked, playtime, last played). Only totals persist; the
  live streak restarts at 0 after every login.
- `MsDaily`: small per-day rollup (`games`, `wins`, `abandoned`, `opened`,
  `flags`, `time_ticks`), reserved for a future stats page.
- Indexes: a partial fastest-win index per `(kind, dim)` restricted to wins,
  a per-spec games index, a per-owner `(owner_id, kind, dim)` index and a
  roster lookup on `MsGamePlayers(player_id)`.

There is intentionally no migration/backfill code. Schema changes require
deleting `data/minesweeper.sql` before deploying (the plugin recreates it).
With one finished game per minute, the log grows by roughly 40–45 MB per year
including its indexes.

The WASM sandbox only exposes the plugin's own data folder (the manifest
requests `fs.write.data`), so the file lives at
`data/minesweeper.sql` inside the sandbox, i.e.
`<server>/plugins/data/<plugin>/minesweeper.sql` on the host.
`~/databases/` is NOT reachable from inside the plugin; to keep the file
there, bind-mount it over the plugin data folder server-side
(`mount --bind ~/databases/ms <server>/plugins/data/<plugin>`), then read it
with `sqlite3` like the other game DBs.

## Leaderboards (text displays)

Two all-time boards as text display entities, refreshed from SQLite every
60 seconds (plus once ~2s after load): **2D 50x50 at (4, 2, 27)** and
**3D 7x7 at (-12, 2, 27)** in the default world (`"world"`, else the first
loaded world). Each board is one gold header on top plus the top-10 fastest
wins below it in a single text display entity (tags `ms_lb_2d50`,
`ms_lb_3d7`). Lines are
tagged (`ms_lb_2d50_00`…), so refresh updates them in place and restarts
heal without ghosts. Coordinates live in `src/leaderboard.rs` (`BOARDS`).
Two of the four boards rank most games finished for a spec; abandoned boards
are stored in `MsGames` but do not count there. `/ms top` shows the fastest
wins and best infinite streaks in chat.

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

## License

GPL-3.0-or-later — see [`LICENSE`](./LICENSE) (GNU GPL v3, 29 June 2007).
