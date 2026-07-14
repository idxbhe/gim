use crate::db::schema;
use crate::error::{GError, GResult};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Game { pub id: i64, pub alias: String, pub title: String, pub game_dir: PathBuf, pub data_dir: PathBuf, pub added_at: i64 }

pub struct GamesDb { conn: Connection }
impl GamesDb {
    pub fn open(path: &Path) -> GResult<Self> {
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let conn = Connection::open(path)?;
        crate::db::apply_pragmas(&conn)?;
        schema::init_games_db(&conn)?;
        Ok(Self { conn })
    }
    pub fn add(&self, alias: &str, title: &str, gd: &Path, dd: &Path) -> GResult<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(|_| {
                // System clock is before UNIX_EPOCH (CMOS battery dead,
                // NTP sync issue). Use 0 as fallback — the DB column
                // has DEFAULT (unixepoch()) which would have been more
                // correct, but we need a value for the explicit INSERT.
                log::warn!("system clock is before UNIX epoch; using 0 as timestamp");
                0
            });
        self.conn.execute("INSERT INTO games (alias, title, gameDir, dataDir, addedAt) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![alias, title, gd.to_string_lossy(), dd.to_string_lossy(), now])
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(err, _) if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE => GError::AliasExists(alias.to_string()),
                o => GError::Sqlite(o),
            })?;
        Ok(())
    }
    pub fn remove(&self, alias: &str) -> GResult<bool> { Ok(self.conn.execute("DELETE FROM games WHERE alias = ?1", params![alias])? > 0) }
    pub fn get(&self, alias: &str) -> GResult<Option<Game>> {
        self.conn.prepare("SELECT id, alias, title, gameDir, dataDir, addedAt FROM games WHERE alias = ?1")?
            .query_row(params![alias], |r| Ok(Game { id: r.get(0)?, alias: r.get(1)?, title: r.get(2)?, game_dir: PathBuf::from(r.get::<_, String>(3)?), data_dir: PathBuf::from(r.get::<_, String>(4)?), added_at: r.get(5)? }))
            .optional().map_err(GError::Sqlite)
    }
    pub fn list(&self) -> GResult<Vec<Game>> {
        let mut stmt = self.conn.prepare("SELECT id, alias, title, gameDir, dataDir, addedAt FROM games ORDER BY alias ASC")?;
        let rows = stmt.query_map([], |r| Ok(Game { id: r.get(0)?, alias: r.get(1)?, title: r.get(2)?, game_dir: PathBuf::from(r.get::<_, String>(3)?), data_dir: PathBuf::from(r.get::<_, String>(4)?), added_at: r.get(5)? }))?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }
}
