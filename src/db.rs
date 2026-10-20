//! SQLite persistence, mirroring the house `LsDatabase` conventions:
//! a single SQLite file, `CREATE TABLE IF NOT EXISTS` on startup, additive
//! column migrations, one connection per operation, and fail-soft behavior
//! (a broken DB disables recording; the game keeps running).
//!
//! Sandbox note: the Pumpkin host only preopens the plugin's own data folder
//! (needs the `fs.write.data` manifest permission), so the file lives at
//! `data/minesweeper.sql` inside the sandbox, i.e.
//! `<server>/plugins/data/<plugin>/minesweeper.sql` on the host.
//! `~/databases/` is NOT reachable from inside the plugin; to land the file
//! there, bind-mount that directory over the plugin data folder server-side.

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection, OptionalExtension};

use crate::state::UuidKey;

/// Guest-side path: resolved against the preopened `data` dir by the host.
pub const DB_PATH: &str = "data/minesweeper.sql";

/// Fail-soft latch: once setup (or the directory) proves broken, every op
/// becomes a no-op instead of logging on every click.
static DB_OK: AtomicBool = AtomicBool::new(true);

fn db_ok() -> bool {
    DB_OK.load(Ordering::Relaxed)
}

fn db_broken(reason: &str) {
    if DB_OK.swap(false, Ordering::Relaxed) {
        tracing::error!("database: disabled ({reason})");
    }
}

/// Open a fresh connection (one per operation, like `LsDatabase`).
fn open(path: &str) -> Option<Connection> {
    if !db_ok() {
        return None;
    }
    match Connection::open(path) {
        Ok(conn) => {
            // Single-threaded sequential use; don't block on locks.
            conn.busy_timeout(std::time::Duration::from_secs(0)).ok();
            Some(conn)
        }
        Err(e) => {
            tracing::warn!("database: open failed ({e})");
            None
        }
    }
}

fn open_default() -> Option<Connection> {
    // Best effort: the host preopens `data/`; ignore create errors and let
    // the open attempt below produce the real diagnostic.
    let _ = std::fs::create_dir_all("data");
    open(DB_PATH)
}

/// Run `f` with a default-path connection. `None` when the DB is disabled.
fn with_conn<R>(f: impl FnOnce(&Connection) -> R) -> Option<R> {
    open_default().map(|c| f(&c))
}

/// 16-byte big-endian UUID blob, matching the house `uuid_to_bytes` layout
/// (most-significant bits first).
pub fn uuid_bytes(id: UuidKey) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&id.0.to_be_bytes());
    b[8..].copy_from_slice(&id.1.to_be_bytes());
    b
}

/// Inverse of `uuid_bytes`, kept for tests pinning the blob layout.
#[cfg(test)]
pub fn uuid_from_bytes(b: &[u8]) -> Option<UuidKey> {
    if b.len() != 16 {
        return None;
    }
    let mut hi = [0u8; 8];
    let mut lo = [0u8; 8];
    hi.copy_from_slice(&b[..8]);
    lo.copy_from_slice(&b[8..]);
    Some((u64::from_be_bytes(hi), u64::from_be_bytes(lo)))
}

fn setup_in(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(concat!(
        "CREATE TABLE IF NOT EXISTS MsBoxGames(",
        "game_id INTEGER PRIMARY KEY,",
        "dim INTEGER, mode STRING, time_ticks INTEGER, won INTEGER,",
        "timestamp INTEGER, owner_uuid BLOB, owner_name STRING, helper_count INTEGER);",
        "CREATE TABLE IF NOT EXISTS MsBoxPlayers(",
        "game INTEGER REFERENCES MsBoxGames(game_id),",
        "player_uuid BLOB, is_owner INTEGER);",
        "CREATE TABLE IF NOT EXISTS MsInfiniteStats(",
        "player_uuid BLOB UNIQUE, player_name STRING,",
        "top_streak INTEGER, total_opened INTEGER, total_bombs INTEGER);",
    ))?;
    // Additive migrations for DBs created by older plugin versions.
    ensure_column(conn, "MsBoxGames", "owner_name", "owner_name STRING")?;
    ensure_column(conn, "MsBoxGames", "helper_count", "helper_count INTEGER")?;
    ensure_column(
        conn,
        "MsInfiniteStats",
        "player_name",
        "player_name STRING",
    )?;
    Ok(())
}

/// Add `column` to `table` when the stored schema lacks it (same pattern
/// as `LsDatabase.ensure_column`, but disambiguated via `sqlite_master` so
/// a missing table isn't mistaken for a missing column).
fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> rusqlite::Result<()> {
    let ddl: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = ?;",
            params![table],
            |row| row.get(0),
        )
        .optional()?;
    let Some(ddl) = ddl else {
        return Ok(());
    };
    if ddl.contains(column) {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {definition};"))?;
    tracing::info!("database: added missing column '{column}' to {table}.");
    Ok(())
}

/// Create tables on startup. Fail-soft: the game runs without recording.
pub fn setup() {
    match open_default() {
        Some(conn) => match setup_in(&conn) {
            Ok(()) => tracing::info!("database: ready at {DB_PATH}"),
            Err(e) => db_broken(&format!("setup failed: {e}")),
        },
        None => db_broken("could not open file"),
    }
}

fn record_box_game_in(
    conn: &Connection,
    dim: usize,
    mode: &str,
    time_ticks: u64,
    won: bool,
    owner: UuidKey,
    owner_name: &str,
    helpers: &[UuidKey],
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO MsBoxGames(dim, mode, time_ticks, won, timestamp, owner_uuid, owner_name, helper_count) \
         VALUES(?, ?, ?, ?, unixepoch(), ?, ?, ?);",
        params![
            dim as i64,
            mode,
            time_ticks as i64,
            won as i64,
            uuid_bytes(owner).to_vec(),
            owner_name,
            helpers.len() as i64,
        ],
    )?;
    let game_id = conn.last_insert_rowid();
    {
        let mut stmt = conn.prepare(
            "INSERT INTO MsBoxPlayers(game, player_uuid, is_owner) VALUES(?, ?, ?);",
        )?;
        stmt.execute(params![game_id, uuid_bytes(owner).to_vec(), 1])?;
        for h in helpers {
            stmt.execute(params![game_id, uuid_bytes(*h).to_vec(), 0])?;
        }
    }
    Ok(())
}

/// Record a finished box game plus its roster. Returns success.
pub fn record_box_game(
    dim: usize,
    mode: &str,
    time_ticks: u64,
    won: bool,
    owner: UuidKey,
    owner_name: &str,
    helpers: &[UuidKey],
) -> bool {
    let ok = with_conn(|c| {
        record_box_game_in(c, dim, mode, time_ticks, won, owner, owner_name, helpers)
    })
    .is_some_and(|r| r.is_ok());
    if ok {
        tracing::info!(
            "database: recorded {mode} {dim} {} by {owner_name} ({} ticks, {} helpers)",
            if won { "win" } else { "loss" },
            time_ticks,
            helpers.len(),
        );
    } else {
        tracing::warn!("database: failed to record box game; will retry on next click.");
    }
    ok
}

fn save_infinite_in(
    conn: &Connection,
    owner: UuidKey,
    name: &str,
    top: usize,
    opened: usize,
    bombs: usize,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO MsInfiniteStats(player_uuid, player_name, top_streak, total_opened, total_bombs) \
         VALUES(?, ?, ?, ?, ?) \
         ON CONFLICT(player_uuid) DO UPDATE SET \
           player_name = excluded.player_name, top_streak = excluded.top_streak, \
           total_opened = excluded.total_opened, total_bombs = excluded.total_bombs;",
        params![
            uuid_bytes(owner).to_vec(),
            name,
            top as i64,
            opened as i64,
            bombs as i64,
        ],
    )?;
    Ok(())
}

/// Write-through infinite session snapshot. Cheap (one row) and fail-soft.
pub fn save_infinite(owner: UuidKey, name: &str, top: usize, opened: usize, bombs: usize) -> bool {
    with_conn(|c| save_infinite_in(c, owner, name, top, opened, bombs)).is_some_and(|r| r.is_ok())
}

pub struct InfiniteRow {
    pub top: usize,
    pub opened: usize,
    pub bombs: usize,
}

pub fn load_infinite_in(conn: &Connection, owner: UuidKey) -> Option<InfiniteRow> {
    conn.query_row(
        "SELECT top_streak, total_opened, total_bombs FROM MsInfiniteStats WHERE player_uuid = ?;",
        params![uuid_bytes(owner).to_vec()],
        |row| {
            Ok(InfiniteRow {
                top: row.get::<_, i64>(0)? as usize,
                opened: row.get::<_, i64>(1)? as usize,
                bombs: row.get::<_, i64>(2)? as usize,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn load_infinite(owner: UuidKey) -> Option<InfiniteRow> {
    with_conn(|c| load_infinite_in(c, owner)).flatten()
}

pub struct BoxRecord {
    pub mode: String,
    pub dim: usize,
    pub games: usize,
    pub wins: usize,
    pub best_secs: Option<u64>,
}

/// Full box record per (mode, dim): games played, wins, and fastest win.
/// Losses are stored too, so they count toward `games` but never `best`.
pub fn box_records_in(conn: &Connection, owner: UuidKey) -> Vec<BoxRecord> {
    let mut stmt = match conn.prepare(
        "SELECT mode, dim, COUNT(*), SUM(won), MIN(CASE WHEN won = 1 THEN time_ticks END) \
         FROM MsBoxGames WHERE owner_uuid = ? GROUP BY mode, dim ORDER BY mode, dim;",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![uuid_bytes(owner).to_vec()], |row| {
        Ok(BoxRecord {
            mode: row.get::<_, String>(0)?,
            dim: row.get::<_, i64>(1)? as usize,
            games: row.get::<_, i64>(2)? as usize,
            wins: row.get::<_, i64>(3)? as usize,
            best_secs: row
                .get::<_, Option<i64>>(4)?
                .map(|t| t as u64 / 20),
        })
    });
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn box_records(owner: UuidKey) -> Vec<BoxRecord> {
    with_conn(|c| box_records_in(c, owner)).unwrap_or_default()
}

pub struct TopWin {
    pub owner_name: String,
    pub mode: String,
    pub dim: usize,
    pub secs: u64,
}

/// Fastest wins overall, optionally filtered to one mode ("2D"/"3D").
pub fn top_wins_in(conn: &Connection, mode: Option<&str>, limit: usize) -> Vec<TopWin> {
    let sql = match mode {
        Some(_) => {
            "SELECT owner_name, mode, dim, MIN(time_ticks) FROM MsBoxGames \
             WHERE won = 1 AND mode = ? GROUP BY owner_uuid, mode, dim \
             ORDER BY MIN(time_ticks) ASC LIMIT ?;"
        }
        None => {
            "SELECT owner_name, mode, dim, MIN(time_ticks) FROM MsBoxGames \
             WHERE won = 1 GROUP BY owner_uuid, mode, dim \
             ORDER BY MIN(time_ticks) ASC LIMIT ?;"
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match mode {
        Some(m) => stmt.query_map(params![m, limit as i64], map_top_win),
        None => stmt.query_map(params![limit as i64], map_top_win),
    };
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

fn map_top_win(row: &rusqlite::Row<'_>) -> rusqlite::Result<TopWin> {
    Ok(TopWin {
        owner_name: row.get::<_, String>(0)?,
        mode: row.get::<_, String>(1)?,
        dim: row.get::<_, i64>(2)? as usize,
        secs: row.get::<_, i64>(3)? as u64 / 20,
    })
}

pub fn top_wins(mode: Option<&str>, limit: usize) -> Vec<TopWin> {
    with_conn(|c| top_wins_in(c, mode, limit)).unwrap_or_default()
}

/// Fastest wins for one exact board spec (mode + dim), all-time.
/// Backs the in-world leaderboard displays.
pub fn top_spec_wins_in(conn: &Connection, mode: &str, dim: usize, limit: usize) -> Vec<TopWin> {
    let mut stmt = match conn.prepare(
        "SELECT owner_name, mode, dim, MIN(time_ticks) FROM MsBoxGames \
         WHERE won = 1 AND mode = ? AND dim = ? GROUP BY owner_uuid \
         ORDER BY MIN(time_ticks) ASC LIMIT ?;",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![mode, dim as i64, limit as i64], map_top_win);
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn top_spec_wins(mode: &str, dim: usize, limit: usize) -> Vec<TopWin> {
    with_conn(|c| top_spec_wins_in(c, mode, dim, limit)).unwrap_or_default()
}

pub struct TopGames {
    pub owner_name: String,
    pub games: u64,
}

/// Most games finished for one exact board spec (mode + dim), all-time.
/// Every row in `MsBoxGames` is a finished game (rows are written on game
/// end, win or loss), so a plain count per owner is the leaderboard. The
/// name shown is the owner's most recent one (MAX over the group).
pub fn top_spec_games_in(conn: &Connection, mode: &str, dim: usize, limit: usize) -> Vec<TopGames> {
    let mut stmt = match conn.prepare(
        "SELECT MAX(owner_name), COUNT(*) FROM MsBoxGames \
         WHERE mode = ? AND dim = ? GROUP BY owner_uuid \
         ORDER BY COUNT(*) DESC LIMIT ?;",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![mode, dim as i64, limit as i64], |row| {
        Ok(TopGames {
            owner_name: row.get::<_, String>(0)?,
            games: row.get::<_, i64>(1)? as u64,
        })
    });
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn top_spec_games(mode: &str, dim: usize, limit: usize) -> Vec<TopGames> {
    with_conn(|c| top_spec_games_in(c, mode, dim, limit)).unwrap_or_default()
}

pub struct TopInfinite {
    pub name: String,
    pub top: usize,
    pub opened: usize,
}

pub fn top_infinite_in(conn: &Connection, limit: usize) -> Vec<TopInfinite> {
    let mut stmt = match conn.prepare(
        "SELECT player_name, top_streak, total_opened FROM MsInfiniteStats \
         ORDER BY top_streak DESC LIMIT ?;",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok(TopInfinite {
            name: row.get::<_, String>(0)?,
            top: row.get::<_, i64>(1)? as usize,
            opened: row.get::<_, i64>(2)? as usize,
        })
    });
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn top_infinite(limit: usize) -> Vec<TopInfinite> {
    with_conn(|c| top_infinite_in(c, limit)).unwrap_or_default()
}

/// Swappable path for unit tests (host temp dir; production always uses
/// `DB_PATH`). Mirrors `LsDatabase.setDatabaseUrlForTests`.
#[cfg(test)]
fn test_db_path(tag: &str) -> String {
    static N: OnceLock<Mutex<u64>> = OnceLock::new();
    let n = {
        let mut g = N.get_or_init(|| Mutex::new(0)).lock().unwrap();
        *g += 1;
        *g
    };
    std::env::temp_dir().join(format!(
        "ppk_ms_test_{tag}_{}_{n}.sql",
        std::process::id()
    ))
    .to_string_lossy()
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memdb() -> (Connection, String) {
        let path = test_db_path("db");
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        setup_in(&conn).unwrap();
        (conn, path)
    }

    fn dropdb(path: &str) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn uuid_blob_roundtrips() {
        let id = (0x0123_4567_89AB_CDEF, 0xFEDC_BA98_7654_3210);
        let bytes = uuid_bytes(id);
        // House layout: most-significant bits first, 16 bytes total.
        assert_eq!(&bytes[..8], &id.0.to_be_bytes());
        assert_eq!(uuid_from_bytes(&bytes), Some(id));
        assert_eq!(uuid_from_bytes(&[0u8; 15]), None);
    }

    #[test]
    fn box_game_record_and_bests_roundtrip() {
        let (conn, path) = memdb();
        let owner = (11, 22);
        record_box_game_in(&conn, 30, "2D", 1200, true, owner, "Alice", &[(33, 44)]).unwrap();
        record_box_game_in(&conn, 30, "2D", 2400, true, owner, "Alice", &[]).unwrap();
        record_box_game_in(&conn, 30, "2D", 100, false, owner, "Alice", &[]).unwrap();
        // Full record per spec; losses count toward games but never best.
        let records = box_records_in(&conn, owner);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].games, 3);
        assert_eq!(records[0].wins, 2);
        assert_eq!(records[0].best_secs, Some(60));
        // Roster rows: owner + helper.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM MsBoxPlayers;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2 + 1 + 1);
        let top = top_wins_in(&conn, Some("2D"), 10);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].owner_name, "Alice");
        assert!(top_wins_in(&conn, Some("3D"), 10).is_empty());
        dropdb(&path);
    }

    #[test]
    fn spec_leaderboard_filters_and_orders() {
        let (conn, path) = memdb();
        record_box_game_in(&conn, 50, "2D", 3000, true, (1, 1), "Slow", &[]).unwrap();
        record_box_game_in(&conn, 50, "2D", 1000, true, (2, 2), "Fast", &[]).unwrap();
        record_box_game_in(&conn, 7, "3D", 500, true, (3, 3), "Cube", &[]).unwrap();
        record_box_game_in(&conn, 50, "2D", 100, false, (4, 4), "Unlucky", &[]).unwrap();
        let board = top_spec_wins_in(&conn, "2D", 50, 10);
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].owner_name, "Fast");
        assert_eq!(board[0].secs, 50);
        assert_eq!(board[1].owner_name, "Slow");
        // Losses never appear; other specs never leak in.
        assert!(top_spec_wins_in(&conn, "2D", 30, 10).is_empty());
        assert_eq!(top_spec_wins_in(&conn, "3D", 7, 10).len(), 1);
        assert_eq!(top_spec_wins_in(&conn, "2D", 50, 1).len(), 1);
        dropdb(&path);
    }

    #[test]
    fn spec_games_count_ranks_and_filters() {
        let (conn, path) = memdb();
        // "Grinder" finishes three 2D-50 games (any outcome), "Fast" one.
        record_box_game_in(&conn, 50, "2D", 3000, true, (1, 1), "Grinder", &[]).unwrap();
        record_box_game_in(&conn, 50, "2D", 1000, true, (1, 1), "Grinder", &[]).unwrap();
        record_box_game_in(&conn, 50, "2D", 100, false, (1, 1), "Grinder", &[]).unwrap();
        record_box_game_in(&conn, 50, "2D", 2000, true, (2, 2), "Fast", &[]).unwrap();
        record_box_game_in(&conn, 7, "3D", 500, true, (3, 3), "Cube", &[]).unwrap();
        let top = top_spec_games_in(&conn, "2D", 50, 10);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].owner_name, "Grinder");
        assert_eq!(top[0].games, 3);
        assert_eq!(top[1].owner_name, "Fast");
        assert_eq!(top[1].games, 1);
        // Other specs never leak in.
        assert_eq!(top_spec_games_in(&conn, "3D", 7, 10)[0].games, 1);
        assert!(top_spec_games_in(&conn, "2D", 30, 10).is_empty());
        assert_eq!(top_spec_games_in(&conn, "2D", 50, 1).len(), 1);
        dropdb(&path);
    }

    #[test]
    fn infinite_upsert_keeps_latest() {
        let (conn, path) = memdb();
        let owner = (55, 66);
        assert!(load_infinite_in(&conn, owner).is_none());
        save_infinite_in(&conn, owner, "Bob", 10, 100, 2).unwrap();
        save_infinite_in(&conn, owner, "Bobby", 42, 500, 3).unwrap();
        let row = load_infinite_in(&conn, owner).unwrap();
        assert_eq!((row.top, row.opened, row.bombs), (42, 500, 3));
        let top = top_infinite_in(&conn, 10);
        assert_eq!(top.len(), 1);
        assert_eq!((top[0].name.as_str(), top[0].top), ("Bobby", 42));
        dropdb(&path);
    }

    #[test]
    fn migrations_add_missing_columns() {
        let path = test_db_path("mig");
        {
            let conn = Connection::open(&path).unwrap();
            // Simulate an older DB: table without the newer columns.
            conn.execute_batch(
                "CREATE TABLE MsBoxGames(game_id INTEGER PRIMARY KEY, dim INTEGER);",
            )
            .unwrap();
            setup_in(&conn).unwrap();
            let ddl: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name = 'MsBoxGames';",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(ddl.contains("owner_name"), "{ddl}");
            assert!(ddl.contains("helper_count"), "{ddl}");
        }
        let _ = std::fs::remove_file(&path);
    }
}
