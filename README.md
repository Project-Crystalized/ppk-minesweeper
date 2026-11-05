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
| `/ms sudoku` | Start a 9x9 Sudoku board at your feet |
| `/ms clear` | Remove all your games (unfinished ones logged as losses, blocks cleared) |
| `/ms clearnearest` | Admin: clear the closest board in your world |
| `/ms list` | List your games (kind, dim, locks, helpers) |
| `/ms stats` | Per-size record (games, wins, best/avg win time) + co-op + infinite totals |
| `/ms top` | Fastest wins + best infinite streaks (from SQLite) |
| `/ms infinite` | Teleport to the endless `ms_infinite` world |

Left-click a cell to open it, right-click toggles a flag with your own color.
Flagged/open cells ignore left-clicks (flagged bombs can't explode). Only
opening a cell adds you as a helper; flagging alone does not.
Your starter sticks (20x20, 40x40, 3D 5x5, 3D 7x7) and the Sudoku stick are
placed in hotbar slots 1-5 — that is the entire default kit; the lock toggle and flag
switcher are not part of it. While you are near a minesweeper board, the
hotbar swaps to just those two items (lock toggle in slot 1, flag color
switcher in slot 2; sticks are hidden so you can't start a messy board while
inside one). Entering either board mode saves your whole hotbar; stepping
away restores it. The swap is re-evaluated every 5 game ticks (worst case
~250 ms), so it feels instant; a wider drop-off radius keeps the kit from
flapping when hugging a board edge. Lock/unlock with the horse-armor item. Right-click the wool
item to cycle through 12 flag colors; left-click it to reset to red. Locked
games only respond to their owner;
clicking your own finished board clears it. Boards survive disconnects: the
owner keeps their progress and helpers can continue unlocked games, while the
owner's timer is paused until they return. Max 3 active games per player. A
per-second sidebar shows the nearest board, its owner, time, and progress
(bombs/flags for minesweeper, filled cells for Sudoku). Infinite mode tracks
streaks with color tiers and 500/1000 broadcasts.

### Sudoku

Right-click the Sudoku stick (hotbar slot 5) or run `/ms sudoku` to spawn a
9x9 board below you. The nine cells in each 3x3 box are contiguous; the box
boundaries are drawn with end-rod particles instead of air gaps.

While you are near a Sudoku board, the hotbar is replaced by nine colored
wool blocks (white, yellow, orange, red, magenta, blue, light blue, lime,
light pink for digits 1-9 - no black, so every digit reads as a
different color). The cells the board comes prefilled with use the same
colors as smooth **concrete** instead, so fixed givens always read as part
of the board while every digit a player places stays colored wool - the same
blocks the hotbar uses. Select slot
1-9, then right-click an editable cell to place that digit; left-click a cell
to clear it. Fixed givens cannot be changed. Leaving the board restores your
previous hotbar/kit. The sidebar shows filled-cell progress, and the board is
accepted as solved once every cell is filled without row/column/box conflicts.
Generated boards are uniquely solvable: the generator blanks digits only
while the board still has exactly one valid solution.
Clicking your own solved board clears it. Boards need a free 9x9 area below
the player and count toward the same 3-game limit.

## Persistence (SQLite)

Finished minesweeper and Sudoku games, including boards explicitly abandoned
with `/ms clear`, plus infinite-mode lifetime totals are recorded in a single
SQLite file, following the `LsDatabase` conventions (one file,
`CREATE TABLE IF NOT EXISTS`, fail-soft: a broken DB disables recording, the
game keeps running):

- `MsPlayers`: identity row per player (UUID, current name, a short name
  history, first/last seen, lifetime online playtime in ticks). Names on
  leaderboards are resolved from here, so renames show up immediately.
- `MsGames`: one row per finished or abandoned game: `kind` (0 = 2D,
  1 = 3D, 2 = Sudoku), `dim`, final board `seed`, `time_ticks`, `outcome`
  (0 = loss, 1 = win; abandoned games are stored as losses), bombs, flags,
  flags placed, cells opened (filled cells for Sudoku), left-clicks, helper
  count, locked flag, epoch end time and board origin `x/y/z`. The world a
  game was played in is deliberately not stored.
- `MsGamePlayers`: game roster (`is_owner` 1/0), which powers co-op credit.
- `MsInfiniteStats`: lifetime infinite totals per player (best streak, cells
  opened, bombs clicked, playtime, last played). Only totals persist; the
  live streak restarts at 0 after every login.
- `MsDaily`: small per-day rollup (`games`, `wins`, `opened`, `flags`,
  `time_ticks`), reserved for a future stats page.
- Indexes: a partial fastest-win index per `(kind, dim)` restricted to wins,
  a per-spec games index, a per-owner `(owner_id, kind, dim)` index and a
  roster lookup on `MsGamePlayers(player_id)`.

There is intentionally no automatic migration/backfill code. For the
`MsDaily.abandoned` removal, stop the server, back up the DB, then run:

```sh
DB=/path/to/plugins/data/<plugin>/minesweeper.sql
cp "$DB" "$DB.bak"
sqlite3 "$DB" "ALTER TABLE MsDaily DROP COLUMN abandoned;"
```

If your `sqlite3` is older than 3.35, rebuild the table instead:

```sh
sqlite3 "$DB" <<'SQL'
BEGIN;
CREATE TABLE MsDaily_new(
  day INTEGER PRIMARY KEY,
  games INTEGER NOT NULL,
  wins INTEGER NOT NULL,
  opened INTEGER NOT NULL,
  flags INTEGER NOT NULL,
  time_ticks INTEGER NOT NULL);
INSERT INTO MsDaily_new(day, games, wins, opened, flags, time_ticks)
SELECT day, games, wins, opened, flags, time_ticks FROM MsDaily;
DROP TABLE MsDaily;
ALTER TABLE MsDaily_new RENAME TO MsDaily;
COMMIT;
SQL
```

The new build does not write `abandoned`; migrate before deploying or
`MsDaily` writes will fail because the old column is `NOT NULL` with no
default. For any other schema change, delete `data/minesweeper.sql` before
deploying (the plugin recreates it). With one finished game per minute, the log
grows by roughly 40–45 MB per year including its indexes.

The WASM sandbox only exposes the plugin's own data folder (the manifest
requests `fs.write.data`), so the file lives at
`data/minesweeper.sql` inside the sandbox, i.e.
`<server>/plugins/data/<plugin>/minesweeper.sql` on the host.
`~/databases/` is NOT reachable from inside the plugin; to keep the file
there, bind-mount it over the plugin data folder server-side
(`mount --bind ~/databases/ms <server>/plugins/data/<plugin>`), then read it
with `sqlite3` like the other game DBs.

## Leaderboards

In-world leaderboards are displayed around the spawn plaza and refresh
automatically. The six boards show:

- Infinite: highest streak and highest net cells opened
  (`total_opened - bombs_clicked`).
- 2D: fastest win and most wins.
- 3D: fastest win and most wins.

The "most wins" boards count wins only; losses do not count. Abandoned games
are stored as losses. On the team best-time boards, each row shows the full
roster that set the time. On the team most-wins boards, every participant gets
credit for the team win, and each row shows one player and their count.

Right-click any wooden button, anywhere on the server, to toggle solo-only vs
team-only games. Right-click any stone button to toggle the small sizes
(2D 20x20, 3D 5x5x5) vs the big sizes (2D 40x40, 3D 7x7x7). Server start
defaults to solo-only and the big sizes. Infinite boards are all-player and
have no team or size dimension, so neither button affects them.

The world spawn is world data, not plugin code: it lives in
`world/level.dat` (`SpawnX/Y/Z`, `SpawnAngle`, plus the `spawn` compound).
Keep both in sync; Pumpkin reads the compound first, then the legacy keys.

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
- Pure board logic in `src/core.rs` (minesweeper) and `src/sudoku.rs`
  (generation, conflicts, win check) has unit tests; no engine deps.
- No extra crates: board RNG is an internal splitmix64, so `wasm32-wasip2`
  builds stay dependency-free.

## Runtime checklist (needs a live Pumpkin server)

- [ ] `/ms 2d20` builds a moss board above the player and reports coords
- [ ] Left-click opens / flood-fills; first click never a bomb
- [ ] Right-click flags/unflags; win and bomb-reveal paths work
- [ ] Open/flag work with the hotbar showing only the lock + flag items near
  a minesweeper board, sticks are hidden, and stepping away restores the
  sticks-only kit (the whole hotbar is cleared and re-laid out)
- [ ] `/ms sudoku` and hotbar slot 5 spawn a 9x9 board; givens stay fixed,
  the hotbar becomes digits 1-9 near the board, right-click places the selected
  digit, left-click clears, and solving records a win
- [ ] Sudoku 3x3 boundaries show end-rod particle lines with no air gaps
- [ ] Disconnecting with active boards keeps them; the sidebar shows their owner
  and the owner's timer pauses while offline
- [ ] `/ms infinite` creates/teleports to `ms_infinite` with moss terrain
- [ ] Finished games show up in `data/minesweeper.sql` (`sqlite3` CLI);
  `/ms stats` + `/ms top` read them back; infinite totals survive restart
- [ ] Empty `register_command` permission (`""`) is accepted by the host

## License

GPL-3.0-or-later — see [`LICENSE`](./LICENSE) (GNU GPL v3, 29 June 2007).
