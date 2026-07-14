//! `gim migrate` — migrate database schema to latest version.
//!
//! This command runs all schema migrations (add columns, create tables,
//! etc.) to bring an older database up to the current version. It is
//! safe to run on already-up-to-date databases (migrations are
//! idempotent).
//!
//! Without an alias, migrates the global `games.db`. With an alias,
//! migrates the per-game `snaps.db`.

use crate::config::{env_data_dir_override, Paths};
use crate::db::schema;
use crate::error::GResult;
use crate::output::Colorizer;
use rusqlite::Connection;

pub fn run(c: &Colorizer, alias: Option<String>) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;

    match alias {
        Some(a) => {
            // Migrate per-game snaps.db.
            let sdb_path = paths.snaps_db(&a);
            if !sdb_path.exists() {
                println!("no snaps.db for game \"{a}\" — nothing to migrate");
                return Ok(());
            }
            println!("migrating snaps.db for game \"{}\"...", c.bold(&a));
            let conn = Connection::open(&sdb_path)?;
            crate::db::apply_pragmas(&conn)?;
            schema::init_snaps_db(&conn)?;
            println!("  {}", c.green("done"));
        }
        None => {
            // Migrate global games.db + all per-game snaps.db.
            println!("migrating global games.db...");
            let gdb_path = &paths.games_db;
            let conn = Connection::open(gdb_path)?;
            crate::db::apply_pragmas(&conn)?;
            schema::init_games_db(&conn)?;
            println!("  {}", c.green("done"));

            // Find all per-game databases.
            if paths.data_dir.exists() {
                for entry in std::fs::read_dir(&paths.data_dir)? {
                    let entry = entry?;
                    if !entry.file_type()?.is_dir() { continue; }
                    let alias_name = match entry.file_name().to_str() {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    let sdb_path = entry.path().join("snaps.db");
                    if !sdb_path.exists() { continue; }
                    println!("migrating snaps.db for game \"{}\"...", c.bold(&alias_name));
                    let conn = Connection::open(&sdb_path)?;
                    crate::db::apply_pragmas(&conn)?;
                    schema::init_snaps_db(&conn)?;
                    println!("  {}", c.green("done"));
                }
            }
        }
    }

    println!("\n{}", c.green("migration complete"));
    Ok(())
}
