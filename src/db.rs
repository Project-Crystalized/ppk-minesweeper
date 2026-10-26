//! SQLite persistence: one file, `CREATE TABLE IF NOT EXISTS` on startup, one
//! connection per operation, fail-soft (a broken DB disables recording; the
//! game keeps running). There is no migration code: reset the file whenever
//! the schema changes.
//!
//! Sandbox: the host only preopens the plugin's `data/` dir, so the file is
//! `data/minesweeper.sql` (host: `<server>/plugins/data/<plugin>/minesweeper.sql`).

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection, OptionalExtension};

use crate::state::UuidKey;

pub const DB_PATH: &str = "data/minesweeper.sql";

pub const KIND_2D: i64 = 0;
pub const KIND_3D: i64 = 1;

pub const OUTCOME_LOSS: i64 = 0;
pub const OUTCOME_WIN: i64 = 1;
pub const OUTCOME_ABANDONED: i64 = 2;

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

/// Open a fresh connection.
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

/// Inverse of `uuid_bytes` (tests pin the blob layout).
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
    // Plain creation: the SQLite file is reset with every schema change
    // (delete `data/minesweeper.sql` before deploying a new schema).
    conn.execute_batch(concat!(
        "CREATE TABLE IF NOT EXISTS MsPlayers(",
        "player_id INTEGER PRIMARY KEY,",
        "uuid BLOB NOT NULL UNIQUE,",
        "name TEXT NOT NULL,",
        "name_history TEXT NOT NULL DEFAULT '',",
        "first_seen INTEGER NOT NULL,",
        "last_seen INTEGER NOT NULL,",
        "playtime_ticks INTEGER NOT NULL DEFAULT 0);",

        "CREATE TABLE IF NOT EXISTS MsGames(",
        "game_id INTEGER PRIMARY KEY,",
        "owner_id INTEGER NOT NULL REFERENCES MsPlayers(player_id),",
        "kind INTEGER NOT NULL,",
        "dim INTEGER NOT NULL,",
        "seed INTEGER NOT NULL,",
        "time_ticks INTEGER NOT NULL,",
        "outcome INTEGER NOT NULL,",
        "bombs INTEGER NOT NULL,",
        "flags INTEGER NOT NULL,",
        "flags_placed INTEGER NOT NULL,",
        "opened INTEGER NOT NULL,",
        "clicks INTEGER NOT NULL,",
        "helpers INTEGER NOT NULL,",
        "locked INTEGER NOT NULL,",
        "ended INTEGER NOT NULL,",
        "x INTEGER NOT NULL, y INTEGER NOT NULL, z INTEGER NOT NULL);",

        "CREATE INDEX IF NOT EXISTS MsGames_wins ",
        "ON MsGames(kind, dim, time_ticks) WHERE outcome = 1;",
        "CREATE INDEX IF NOT EXISTS MsGames_games ",
        "ON MsGames(kind, dim, owner_id);",
        "CREATE INDEX IF NOT EXISTS MsGames_owner ",
        "ON MsGames(owner_id, kind, dim);",

        "CREATE TABLE IF NOT EXISTS MsGamePlayers(",
        "game_id INTEGER NOT NULL REFERENCES MsGames(game_id) ON DELETE CASCADE,",
        "player_id INTEGER NOT NULL REFERENCES MsPlayers(player_id),",
        "is_owner INTEGER NOT NULL,",
        "PRIMARY KEY(game_id, player_id));",
        "CREATE INDEX IF NOT EXISTS MsGamePlayers_player ",
        "ON MsGamePlayers(player_id);",

        "CREATE TABLE IF NOT EXISTS MsInfiniteStats(",
        "player_id INTEGER PRIMARY KEY REFERENCES MsPlayers(player_id),",
        "top_streak INTEGER NOT NULL,",
        "total_opened INTEGER NOT NULL,",
        "bombs_clicked INTEGER NOT NULL,",
        "playtime_ticks INTEGER NOT NULL DEFAULT 0,",
        "last_played INTEGER NOT NULL);",

        "CREATE TABLE IF NOT EXISTS MsDaily(",
        "day INTEGER PRIMARY KEY,",
        "games INTEGER NOT NULL,",
        "wins INTEGER NOT NULL,",
        "abandoned INTEGER NOT NULL,",
        "opened INTEGER NOT NULL,",
        "flags INTEGER NOT NULL,",
        "time_ticks INTEGER NOT NULL);",
    ))
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

/// Keep the last few distinct old names, comma separated.
fn push_name(history: &str, old: &str) -> String {
    let old = old.trim();
    if old.is_empty() || old == "?" {
        return history.to_string();
    }
    let mut names: Vec<&str> = history.split(',').filter(|s| !s.is_empty()).collect();
    if names.contains(&old) {
        return history.to_string();
    }
    names.push(old);
    if names.len() > 6 {
        names.remove(0);
    }
    names.join(",")
}

/// Fetch or create a player row. `name` is only written when supplied (and
/// different); helpers recorded offline are inserted as `"?"` and repaired
/// when they next join.
fn ensure_player_in(
    conn: &Connection,
    owner: UuidKey,
    name: Option<&str>,
) -> rusqlite::Result<i64> {
    let bytes = uuid_bytes(owner).to_vec();
    let existing: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT player_id, name, name_history FROM MsPlayers WHERE uuid = ?;",
            params![bytes],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match existing {
        Some((id, current, history)) => {
            match name.filter(|n| !n.is_empty() && *n != "?") {
                Some(name) if name != current => {
                    let history = push_name(&history, &current);
                    conn.execute(
                        "UPDATE MsPlayers SET name = ?, name_history = ?, \
                         last_seen = unixepoch() WHERE player_id = ?;",
                        params![name, history, id],
                    )?;
                }
                _ => {
                    conn.execute(
                        "UPDATE MsPlayers SET last_seen = unixepoch() \
                         WHERE player_id = ?;",
                        params![id],
                    )?;
                }
            }
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO MsPlayers(uuid, name, name_history, first_seen, last_seen) \
                 VALUES(?, ?, '', unixepoch(), unixepoch());",
                params![bytes, name.unwrap_or("?")],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }
}

pub fn touch_player(owner: UuidKey, name: &str) {
    let _ = with_conn(|c| ensure_player_in(c, owner, Some(name)));
}

/// Accumulate lifetime online time (ticks, 20/s).
pub fn add_playtime(owner: UuidKey, ticks: u64) {
    if ticks == 0 {
        return;
    }
    let _ = with_conn(|c| {
        c.execute(
            "UPDATE MsPlayers SET playtime_ticks = playtime_ticks + ? \
             WHERE uuid = ?;",
            params![ticks as i64, uuid_bytes(owner).to_vec()],
        )
    });
}

pub struct GameRecord<'a> {
    pub owner: UuidKey,
    /// Owner name when known (usually only when the owner is the finisher).
    pub owner_name: Option<&'a str>,
    pub helpers: &'a [UuidKey],
    pub kind: i64,
    pub dim: usize,
    /// Final board seed; `u64` bitcast into SQLite's signed INTEGER.
    pub seed: u64,
    pub time_ticks: u64,
    pub outcome: i64,
    pub bombs: usize,
    pub flags: usize,
    pub flags_placed: u32,
    pub opened: usize,
    pub clicks: u32,
    pub locked: bool,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

fn kind_label(kind: i64) -> &'static str {
    if kind == KIND_3D { "3D" } else { "2D" }
}

fn outcome_label(outcome: i64) -> &'static str {
    match outcome {
        OUTCOME_WIN => "win",
        OUTCOME_ABANDONED => "abandoned",
        _ => "loss",
    }
}

fn record_game_in(conn: &Connection, r: &GameRecord<'_>) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let owner_id = ensure_player_in(&tx, r.owner, r.owner_name)?;
    let mut helper_ids = Vec::with_capacity(r.helpers.len());
    for helper in r.helpers {
        let id = ensure_player_in(&tx, *helper, None)?;
        if id != owner_id && !helper_ids.contains(&id) {
            helper_ids.push(id);
        }
    }
    tx.execute(
        "INSERT INTO MsGames(owner_id, kind, dim, seed, time_ticks, outcome, \
         bombs, flags, flags_placed, opened, clicks, helpers, \
         locked, ended, x, y, z) \
         VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), ?, ?, ?);",
        params![
            owner_id,
            r.kind,
            r.dim as i64,
            r.seed as i64,
            r.time_ticks as i64,
            r.outcome,
            r.bombs as i64,
            r.flags as i64,
            r.flags_placed as i64,
            r.opened as i64,
            r.clicks as i64,
            helper_ids.len() as i64,
            r.locked as i64,
            r.x,
            r.y,
            r.z,
        ],
    )?;
    let game_id = tx.last_insert_rowid();
    {
        let mut stmt = tx.prepare(
            "INSERT INTO MsGamePlayers(game_id, player_id, is_owner) \
             VALUES(?, ?, ?);",
        )?;
        stmt.execute(params![game_id, owner_id, 1])?;
        for id in &helper_ids {
            stmt.execute(params![game_id, id, 0])?;
        }
    }
    tx.execute(
        "INSERT INTO MsDaily(day, games, wins, abandoned, opened, flags, time_ticks) \
         VALUES(unixepoch() / 86400, 1, ?, ?, ?, ?, ?) \
         ON CONFLICT(day) DO UPDATE SET \
           games = games + 1, \
           wins = wins + excluded.wins, \
           abandoned = abandoned + excluded.abandoned, \
           opened = opened + excluded.opened, \
           flags = flags + excluded.flags, \
           time_ticks = time_ticks + excluded.time_ticks;",
        params![
            (r.outcome == OUTCOME_WIN) as i64,
            (r.outcome == OUTCOME_ABANDONED) as i64,
            r.opened as i64,
            r.flags_placed as i64,
            r.time_ticks as i64,
        ],
    )?;
    tx.commit()
}

pub fn record_game(record: &GameRecord<'_>) -> bool {
    let ok = with_conn(|c| record_game_in(c, record)).is_some_and(|r| r.is_ok());
    if ok {
        tracing::info!(
            "database: recorded {} {} {} by {} ({} ticks, {} helpers)",
            kind_label(record.kind),
            record.dim,
            outcome_label(record.outcome),
            record.owner_name.unwrap_or("?"),
            record.time_ticks,
            record.helpers.len(),
        );
    } else {
        tracing::error!("database: failed to record game");
    }
    ok
}

/// `MsInfiniteStats` row: lifetime totals only (no session data).
pub struct InfiniteRow {
    pub top: usize,
    pub opened: usize,
    pub bombs_clicked: usize,
    pub playtime: u64,
}

fn save_infinite_in(
    conn: &Connection,
    owner: UuidKey,
    name: Option<&str>,
    top: usize,
    opened: usize,
    bombs_clicked: usize,
    playtime: u64,
) -> rusqlite::Result<()> {
    let player_id = ensure_player_in(conn, owner, name)?;
    conn.execute(
        "INSERT INTO MsInfiniteStats(player_id, top_streak, total_opened, \
         bombs_clicked, playtime_ticks, last_played) \
         VALUES(?, ?, ?, ?, ?, unixepoch()) \
         ON CONFLICT(player_id) DO UPDATE SET \
           top_streak = excluded.top_streak, \
           total_opened = excluded.total_opened, \
           bombs_clicked = excluded.bombs_clicked, \
           playtime_ticks = excluded.playtime_ticks, \
           last_played = excluded.last_played;",
        params![
            player_id,
            top as i64,
            opened as i64,
            bombs_clicked as i64,
            playtime as i64,
        ],
    )?;
    Ok(())
}

pub fn save_infinite(
    owner: UuidKey,
    name: Option<&str>,
    top: usize,
    opened: usize,
    bombs_clicked: usize,
    playtime: u64,
) -> bool {
    with_conn(|c| save_infinite_in(c, owner, name, top, opened, bombs_clicked, playtime))
        .is_some_and(|r| r.is_ok())
}

pub fn load_infinite_in(conn: &Connection, owner: UuidKey) -> Option<InfiniteRow> {
    conn.query_row(
        "SELECT top_streak, total_opened, bombs_clicked, playtime_ticks \
         FROM MsInfiniteStats \
         WHERE player_id = (SELECT player_id FROM MsPlayers WHERE uuid = ?);",
        params![uuid_bytes(owner).to_vec()],
        |row| {
            Ok(InfiniteRow {
                top: row.get::<_, i64>(0)? as usize,
                opened: row.get::<_, i64>(1)? as usize,
                bombs_clicked: row.get::<_, i64>(2)? as usize,
                playtime: row.get::<_, i64>(3)? as u64,
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

/// Aggregate record for one player and one exact board spec.
pub struct SpecRecord {
    pub kind: i64,
    pub dim: usize,
    pub games: usize,
    pub wins: usize,
    /// Games dropped unfinished (`/ms clear`, leave).
    pub abandoned: usize,
    pub best_secs: Option<u64>,
    pub avg_secs: Option<u64>,
    pub opened: u64,
    pub flags: u64,
    pub bombs: u64,
}

/// Per-spec history: games (any outcome), wins, abandoned count, fastest win,
/// average finished-game duration, and cumulative opened/flags/bombs.
pub fn spec_records_in(conn: &Connection, owner: UuidKey) -> Vec<SpecRecord> {
    let mut stmt = match conn.prepare(
        "SELECT kind, dim, COUNT(*), SUM(outcome = 1), SUM(outcome = 2), \
           MIN(CASE WHEN outcome = 1 THEN time_ticks END), \
           AVG(CASE WHEN outcome <> 2 THEN time_ticks END), \
           COALESCE(SUM(opened), 0), COALESCE(SUM(flags_placed), 0), \
           COALESCE(SUM(bombs), 0) \
         FROM MsGames \
         WHERE owner_id = (SELECT player_id FROM MsPlayers WHERE uuid = ?) \
         GROUP BY kind, dim ORDER BY kind, dim;",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![uuid_bytes(owner).to_vec()], |row| {
        Ok(SpecRecord {
            kind: row.get::<_, i64>(0)?,
            dim: row.get::<_, i64>(1)? as usize,
            games: row.get::<_, i64>(2)? as usize,
            wins: row.get::<_, i64>(3)? as usize,
            abandoned: row.get::<_, i64>(4)? as usize,
            best_secs: row
                .get::<_, Option<i64>>(5)?
                .map(|t| t as u64 / 20),
            avg_secs: row
                .get::<_, Option<f64>>(6)?
                .map(|t| (t / 20.0).round() as u64),
            opened: row.get::<_, i64>(7)? as u64,
            flags: row.get::<_, i64>(8)? as u64,
            bombs: row.get::<_, i64>(9)? as u64,
        })
    });
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn spec_records(owner: UuidKey) -> Vec<SpecRecord> {
    with_conn(|c| spec_records_in(c, owner)).unwrap_or_default()
}

/// Games another player finished on your board (and how many were wins).
pub struct HelperRecord {
    pub games: usize,
    pub wins: usize,
}

pub fn helper_record_in(conn: &Connection, owner: UuidKey) -> HelperRecord {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(g.outcome = 1), 0) \
         FROM MsGamePlayers gp JOIN MsGames g ON g.game_id = gp.game_id \
         WHERE gp.player_id = (SELECT player_id FROM MsPlayers WHERE uuid = ?) \
           AND gp.is_owner = 0;",
        params![uuid_bytes(owner).to_vec()],
        |row| {
            Ok(HelperRecord {
                games: row.get::<_, i64>(0)? as usize,
                wins: row.get::<_, i64>(1)? as usize,
            })
        },
    )
    .unwrap_or(HelperRecord { games: 0, wins: 0 })
}

pub fn helper_record(owner: UuidKey) -> HelperRecord {
    with_conn(|c| helper_record_in(c, owner)).unwrap_or(HelperRecord { games: 0, wins: 0 })
}

pub struct TopWin {
    pub owner_name: String,
    pub kind: i64,
    pub dim: usize,
    pub secs: u64,
}

fn map_top_win(row: &rusqlite::Row<'_>) -> rusqlite::Result<TopWin> {
    Ok(TopWin {
        owner_name: row.get::<_, String>(0)?,
        kind: row.get::<_, i64>(1)?,
        dim: row.get::<_, i64>(2)? as usize,
        secs: row.get::<_, i64>(3)? as u64 / 20,
    })
}

pub fn top_wins_in(conn: &Connection, kind: Option<i64>, limit: usize) -> Vec<TopWin> {
    let sql = match kind {
        Some(_) => {
            "SELECT p.name, g.kind, g.dim, MIN(g.time_ticks) \
             FROM MsGames g JOIN MsPlayers p ON p.player_id = g.owner_id \
             WHERE g.outcome = 1 AND g.kind = ? \
             GROUP BY g.owner_id, g.kind, g.dim \
             ORDER BY MIN(g.time_ticks) ASC LIMIT ?;"
        }
        None => {
            "SELECT p.name, g.kind, g.dim, MIN(g.time_ticks) \
             FROM MsGames g JOIN MsPlayers p ON p.player_id = g.owner_id \
             WHERE g.outcome = 1 \
             GROUP BY g.owner_id, g.kind, g.dim \
             ORDER BY MIN(g.time_ticks) ASC LIMIT ?;"
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match kind {
        Some(k) => stmt.query_map(params![k, limit as i64], map_top_win),
        None => stmt.query_map(params![limit as i64], map_top_win),
    };
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn top_wins(kind: Option<i64>, limit: usize) -> Vec<TopWin> {
    with_conn(|c| top_wins_in(c, kind, limit)).unwrap_or_default()
}

/// Fastest wins for one exact board spec, all-time. Backs the in-world
/// leaderboard displays.
pub fn top_spec_wins_in(
    conn: &Connection,
    kind: i64,
    dim: usize,
    limit: usize,
) -> Vec<TopWin> {
    let mut stmt = match conn.prepare(
        "SELECT p.name, g.kind, g.dim, MIN(g.time_ticks) \
         FROM MsGames g JOIN MsPlayers p ON p.player_id = g.owner_id \
         WHERE g.outcome = 1 AND g.kind = ? AND g.dim = ? \
         GROUP BY g.owner_id ORDER BY MIN(g.time_ticks) ASC LIMIT ?;",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![kind, dim as i64, limit as i64], map_top_win);
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn top_spec_wins(kind: i64, dim: usize, limit: usize) -> Vec<TopWin> {
    with_conn(|c| top_spec_wins_in(c, kind, dim, limit)).unwrap_or_default()
}

pub struct TopWinCount {
    pub owner_name: String,
    pub wins: u64,
}

/// Most wins for one exact board spec, all-time. Backs the in-world
/// leaderboard displays.
pub fn top_spec_win_counts_in(
    conn: &Connection,
    kind: i64,
    dim: usize,
    limit: usize,
) -> Vec<TopWinCount> {
    let mut stmt = match conn.prepare(
        "SELECT p.name, COUNT(*) \
         FROM MsGames g JOIN MsPlayers p ON p.player_id = g.owner_id \
         WHERE g.outcome = 1 AND g.kind = ? AND g.dim = ? \
         GROUP BY g.owner_id ORDER BY COUNT(*) DESC LIMIT ?;",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![kind, dim as i64, limit as i64], |row| {
        Ok(TopWinCount {
            owner_name: row.get::<_, String>(0)?,
            wins: row.get::<_, i64>(1)? as u64,
        })
    });
    rows.map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
}

pub fn top_spec_win_counts(kind: i64, dim: usize, limit: usize) -> Vec<TopWinCount> {
    with_conn(|c| top_spec_win_counts_in(c, kind, dim, limit)).unwrap_or_default()
}

pub struct TopInfinite {
    pub name: String,
    pub top: usize,
    pub opened: usize,
}

pub fn top_infinite_in(conn: &Connection, limit: usize) -> Vec<TopInfinite> {
    let mut stmt = match conn.prepare(
        "SELECT p.name, s.top_streak, s.total_opened \
         FROM MsInfiniteStats s JOIN MsPlayers p ON p.player_id = s.player_id \
         ORDER BY s.top_streak DESC, s.total_opened DESC LIMIT ?;",
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

    fn rec(
        owner: UuidKey,
        name: &'static str,
        kind: i64,
        dim: usize,
        time_ticks: u64,
        outcome: i64,
    ) -> GameRecord<'static> {
        GameRecord {
            owner,
            owner_name: Some(name),
            helpers: &[],
            kind,
            dim,
            seed: 0xDEAD_BEEF_CAFE_F00D,
            time_ticks,
            outcome,
            bombs: 3,
            flags: 1,
            flags_placed: 4,
            opened: 10,
            clicks: 12,
            locked: false,
            x: 100,
            y: 64,
            z: -200,
        }
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
    fn game_record_and_spec_stats_roundtrip() {
        let (conn, path) = memdb();
        let owner = (11, 22);
        record_game_in(&conn, &rec(owner, "Alice", KIND_2D, 30, 1200, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec(owner, "Alice", KIND_2D, 30, 2400, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec(owner, "Alice", KIND_2D, 30, 100, OUTCOME_LOSS)).unwrap();
        let records = spec_records_in(&conn, owner);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].games, 3);
        assert_eq!(records[0].wins, 2);
        assert_eq!(records[0].abandoned, 0);
        assert_eq!(records[0].best_secs, Some(60));
        assert_eq!(records[0].avg_secs, Some(62));
        assert_eq!(records[0].opened, 30);
        assert_eq!(records[0].flags, 12);
        assert_eq!(records[0].bombs, 9);
        // Seed survives the u64 -> i64 bitcast.
        let seed: i64 = conn
            .query_row("SELECT seed FROM MsGames LIMIT 1;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seed as u64, 0xDEAD_BEEF_CAFE_F00D);
        // Roster has one owner row per game (no helpers passed here).
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM MsGamePlayers;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
        dropdb(&path);
    }

    #[test]
    fn spec_leaderboard_filters_and_orders() {
        let (conn, path) = memdb();
        record_game_in(&conn, &rec((1, 1), "Slow", KIND_2D, 50, 3000, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec((2, 2), "Fast", KIND_2D, 50, 1000, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec((3, 3), "Cube", KIND_3D, 7, 500, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec((4, 4), "Unlucky", KIND_2D, 50, 100, OUTCOME_LOSS)).unwrap();
        let board = top_spec_wins_in(&conn, KIND_2D, 50, 10);
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].owner_name, "Fast");
        assert_eq!(board[0].secs, 50);
        assert_eq!(board[1].owner_name, "Slow");
        // Losses never appear; other specs never leak in.
        assert!(top_spec_wins_in(&conn, KIND_2D, 30, 10).is_empty());
        assert_eq!(top_spec_wins_in(&conn, KIND_3D, 7, 10).len(), 1);
        assert_eq!(top_spec_wins_in(&conn, KIND_2D, 50, 1).len(), 1);
        assert!(top_wins_in(&conn, Some(KIND_3D), 10).len() == 1);
        assert_eq!(top_wins_in(&conn, None, 10).len(), 3);
        dropdb(&path);
    }

    #[test]
    fn spec_win_counts_rank_and_filter() {
        let (conn, path) = memdb();
        // "Grinder" wins two 2D-50 games (plus one loss), "Fast" one.
        record_game_in(&conn, &rec((1, 1), "Grinder", KIND_2D, 50, 3000, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec((1, 1), "Grinder", KIND_2D, 50, 1000, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec((1, 1), "Grinder", KIND_2D, 50, 100, OUTCOME_LOSS)).unwrap();
        record_game_in(&conn, &rec((2, 2), "Fast", KIND_2D, 50, 2000, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec((3, 3), "Cube", KIND_3D, 7, 500, OUTCOME_WIN)).unwrap();
        let top = top_spec_win_counts_in(&conn, KIND_2D, 50, 10);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].owner_name, "Grinder");
        assert_eq!(top[0].wins, 2);
        assert_eq!(top[1].owner_name, "Fast");
        assert_eq!(top[1].wins, 1);
        // Other specs never leak in.
        assert_eq!(top_spec_win_counts_in(&conn, KIND_3D, 7, 10)[0].wins, 1);
        assert!(top_spec_win_counts_in(&conn, KIND_2D, 30, 10).is_empty());
        assert_eq!(top_spec_win_counts_in(&conn, KIND_2D, 50, 1).len(), 1);
        dropdb(&path);
    }

    #[test]
    fn abandon_counts_as_game_but_never_a_win() {
        let (conn, path) = memdb();
        let owner = (9, 9);
        record_game_in(&conn, &rec(owner, "Quitter", KIND_2D, 20, 200, OUTCOME_ABANDONED))
            .unwrap();
        let records = spec_records_in(&conn, owner);
        assert_eq!(records[0].games, 1);
        assert_eq!(records[0].wins, 0);
        assert_eq!(records[0].abandoned, 1);
        assert_eq!(records[0].best_secs, None);
        assert_eq!(records[0].avg_secs, None);
        // Abandoned and lost games don't rank on the "most wins" board.
        assert!(top_spec_win_counts_in(&conn, KIND_2D, 20, 10).is_empty());
        record_game_in(&conn, &rec(owner, "Quitter", KIND_2D, 20, 400, OUTCOME_LOSS)).unwrap();
        assert!(top_spec_win_counts_in(&conn, KIND_2D, 20, 10).is_empty());
        // A win does, and abandoned/lost games still don't add to it.
        record_game_in(&conn, &rec(owner, "Quitter", KIND_2D, 20, 500, OUTCOME_WIN)).unwrap();
        assert_eq!(top_spec_win_counts_in(&conn, KIND_2D, 20, 10)[0].wins, 1);
        dropdb(&path);
    }

    #[test]
    fn renames_show_the_current_name() {
        let (conn, path) = memdb();
        let owner = (7, 7);
        record_game_in(&conn, &rec(owner, "Alice", KIND_2D, 20, 400, OUTCOME_WIN)).unwrap();
        ensure_player_in(&conn, owner, Some("Alicia")).unwrap();
        let top = top_spec_wins_in(&conn, KIND_2D, 20, 10);
        assert_eq!(top[0].owner_name, "Alicia");
        let history: String = conn
            .query_row(
                "SELECT name_history FROM MsPlayers WHERE uuid = ?;",
                params![uuid_bytes(owner).to_vec()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(history, "Alice");
        dropdb(&path);
    }

    #[test]
    fn helper_credit_is_recorded() {
        let (conn, path) = memdb();
        let owner = (21, 21);
        let helper = (22, 22);
        let mut game = rec(owner, "Owner", KIND_2D, 20, 400, OUTCOME_WIN);
        let roster = [helper];
        game.helpers = &roster;
        record_game_in(&conn, &game).unwrap();
        let h = helper_record_in(&conn, helper);
        assert_eq!((h.games, h.wins), (1, 1));
        let own = helper_record_in(&conn, owner);
        assert_eq!((own.games, own.wins), (0, 0));
        // Owner row + helper row.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM MsGamePlayers;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        dropdb(&path);
    }

    #[test]
    fn infinite_upsert_keeps_latest() {
        let (conn, path) = memdb();
        let owner = (55, 66);
        assert!(load_infinite_in(&conn, owner).is_none());
        save_infinite_in(&conn, owner, Some("Bob"), 10, 100, 2, 400).unwrap();
        save_infinite_in(&conn, owner, Some("Bobby"), 42, 500, 3, 900).unwrap();
        let row = load_infinite_in(&conn, owner).unwrap();
        assert_eq!(
            (row.top, row.opened, row.bombs_clicked, row.playtime),
            (42, 500, 3, 900)
        );
        let top = top_infinite_in(&conn, 10);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].name, "Bobby");
        assert_eq!(top[0].top, 42);
        dropdb(&path);
    }

    #[test]
    fn daily_rollup_accumulates() {
        let (conn, path) = memdb();
        record_game_in(&conn, &rec((1, 1), "A", KIND_2D, 20, 400, OUTCOME_WIN)).unwrap();
        record_game_in(&conn, &rec((2, 2), "B", KIND_3D, 5, 200, OUTCOME_ABANDONED)).unwrap();
        let (games, wins, abandoned, opened, flags, ticks): (i64, i64, i64, i64, i64, i64) =
            conn.query_row(
                "SELECT games, wins, abandoned, opened, flags, time_ticks FROM MsDaily;",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!((games, wins, abandoned), (2, 1, 1));
        assert_eq!((opened, flags, ticks), (20, 8, 600));
        dropdb(&path);
    }
}
