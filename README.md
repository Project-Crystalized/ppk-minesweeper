# ppk-minesweeper

A Pumpkin (Rust/WASM) plugin port of the old Valence-based minesweeper server
in `minesweeper/`. Valence is unmaintained, so the game now runs as a plugin
inside Pumpkin instead of as its own server.

Everything can be controlled with the /ms command

Included are a implementation of minesweeper, even in 3d!
and a implemetation of sudoku!

Starter sticks live in the hotbar. Inside the endless infinite minesweeper
world the kit is instead a flag color wool (slot 8) and a stick (slot 9)
that teleports back to the default world spawn.


## Persistence (SQLite)

Finished minesweeper and Sudoku games, including boards explicitly abandoned
with `/ms clear`, plus infinite-mode lifetime totals are recorded in a single
SQLite file, following the `LsDatabase` conventions (one file,
`CREATE TABLE IF NOT EXISTS`, fail-soft: a broken DB disables recording, the
game keeps running):

The WASM sandbox only exposes the plugin's own data folder (the manifest
requests `fs.write.data`), so the file lives at
`data/minesweeper.sql` inside the sandbox, i.e.
`<server>/plugins/data/<plugin>/minesweeper.sql` on the host.
`~/databases/` is NOT reachable from inside the plugin; to keep the file
there, bind-mount it over the plugin data folder server-side
(`mount --bind ~/databases/ms <server>/plugins/data/<plugin>`), then read it
with `sqlite3` like the other game DBs.

## Leaderboards

There are Leaderboards

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

## License

GPL-3.0-or-later — see [`LICENSE`](./LICENSE) (GNU GPL v3, 29 June 2007).
