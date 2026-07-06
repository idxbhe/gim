# `gim` — Game Files Version Control Tool

A CLI tool for versioning game files. Similar to `git`, but purpose-built for
game directories. Uses **SQLite** for metadata and **XXH3-128** for fast
non-cryptographic file hashing.

Built in Rust with a modular, production-ready architecture: per-command
modules, a transactional storage layer, content-addressable object store with
automatic deduplication, parallel hashing via Rayon, and advisory locking to
prevent concurrent mutation.

---

## Quick start

```bash
# Build
cargo build --release

# The binary is at target/release/gim. Copy it somewhere on your PATH.

# Add a game (creates data/mario/ structure)
gim add mario "C:/Games/Super Mario Bros"

# Take the first snapshot
gim snap mario -m "Initial snapshot"

# Make some changes to the game directory, then snapshot again
gim snap mario -m "Installed texture pack"

# See what changed since the last snapshot
gim status mario

# Browse history
gim log mario

# Compare two snapshots
gim diff mario original 20240115-143000

# Restore the game directory to a previous state
gim restore mario original --full

# Garbage-collect unreferenced objects
gim gc mario --dry-run
gim gc mario

# Remove a game and all its data
gim remove mario --confirm
```

---

## Binary directory layout

```
gim.exe
data/
  games.db                                  global game registry
  gignore                                   global ignore patterns (optional)
  [game alias]/
    snaps.db                                snapshot & file registry
    objects/                                content-addressable file store
      ab/cdef0123456789abcdef...            file blob, stored as [hash_prefix]/[hash]
    .gignore                                per-game ignore patterns (optional)
```

The data directory defaults to `[gim binary dir]/data/` and can be overridden
with the `G_DATA_DIR` environment variable (useful for tests).

---

## Architecture

The crate is structured as a library with a thin binary wrapper, so that the
core logic is reusable from integration tests and future tooling.

```
src/
├── main.rs              Binary entry point: parse CLI, dispatch, exit code
├── lib.rs               Library root: re-exports all modules
├── cli.rs               clap derive definitions for every subcommand
├── error.rs             GError enum + exit-code mapping
├── commands/            One module per subcommand
│   ├── add.rs
│   ├── remove.rs
│   ├── list.rs
│   ├── snap.rs          Core: walk → hash → diff → store → report
│   ├── restore.rs       Core: walk → diff → copy/delete in parallel
│   ├── status.rs
│   ├── log_cmd.rs
│   ├── diff.rs
│   ├── gc.rs
│   └── ignore_cmd.rs
├── db/                  SQLite layer
│   ├── games.rs         games.db CRUD
│   ├── snaps.rs         snaps.db CRUD + diff_states() pure helper
│   └── schema.rs        Idempotent DDL for both databases
├── storage/
│   └── cas.rs           Content-addressable store (atomic writes, dedup)
├── hashing/
│   └── mod.rs           XXH3-128 streaming hash + retry-on-locked-file
├── ignore_mod/
│   └── mod.rs           gitignore-compatible pattern matching (uses `ignore` crate)
├── walker/
│   └── mod.rs           Parallel walk + hash pipeline (Rayon)
├── path_utils/
│   └── mod.rs           Path normalization (forward-slash, relative, UTF-8)
├── locking/
│   └── mod.rs           Advisory file locks (fs2) — prevents concurrent snap/restore
├── config/
│   └── mod.rs           Path resolution (binary dir, data dir, per-game paths)
├── output/
│   ├── mod.rs           Colorizer + TTY/NO_COLOR detection
│   ├── color.rs         Color helpers (green/red/yellow/bold/dim)
│   └── fmt.rs           Size + timestamp formatting
└── parallel/
    └── mod.rs           Re-exports Rayon (future parallel-utility home)
```

### Design principles

- **Modular**: each command, storage primitive, and IO concern is in its own
  module. Adding a new command only touches `commands/mod.rs` (one new
  `pub mod` line + one new match arm in `dispatch`) plus the new file.
- **Pure where possible**: `diff_states()` and `normalize()` are pure
  functions with no side effects and full unit-test coverage.
- **Transactional**: every `gim snap` writes its `snaps` row, `files` rows,
  and `deleted_files` rows inside a single SQLite transaction. If anything
  fails, the transaction rolls back and any objects already copied to the
  CAS are deleted.
- **Streaming**: XXH3 hashing streams files through a 1 MiB buffer, so
  multi-GB game archives are hashed without loading them into memory.
- **Parallel**: the walk→hash pipeline uses Rayon. Worker count is
  configurable with `--threads`; defaults to `num_cpus`.
- **Resilient**: locked files (e.g. when a game is running) are retried 3
  times with 500 ms delay, then skipped with a warning rather than failing
  the whole snapshot.
- **Atomic CAS writes**: every object is written to a `.tmp` sibling, fsync'd,
  and atomically renamed to its final name. A crash never leaves a
  partially-written object visible to readers.

---

## Hashing

- **Algorithm**: XXH3-128 (128-bit, non-cryptographic)
- **Output**: 32-character lowercase hex string
- **Why**: XXH3 runs at multi-GB/s on modern CPUs, ideal for large game files
  (textures, archives, executables). 128-bit collision resistance is more
  than sufficient for file-integrity verification in the threat model of a
  personal backup tool.

---

## Path normalization

All paths stored in the database follow these rules:

1. Relative to the game directory root (no drive letters, no absolute paths).
2. Forward slash `/` as the directory separator on all platforms.
3. No leading or trailing slash.
4. UTF-8 encoded.

Example:

```
gameDir:  C:\Games\Super Mario Bros
filePath: mods/texture_pack/overworld.png
```

Normalization is applied at both `snap` time and `restore` time, and must be
consistent across all commands.

---

## Ignore patterns

Ignore patterns are evaluated in order and merged:

1. **Built-in defaults** (`*.tmp`, `*.temp`, `*.bak`, `*.swp`, `Thumbs.db`,
   `.DS_Store`, `desktop.ini`) — always applied, cannot be overridden.
2. **Global** (`data/gignore`) — applies to all games.
3. **Per-game** (`data/[alias]/.gignore`) — applies to a specific game.
4. **In-game** (`[gameDir]/.gignore`) — lives inside the game directory
   itself.

Pattern syntax follows gitignore semantics: `*.log`, `logs/`,
`saves/auto_save_*`, `!important.log` (re-include), etc. The matching engine
is the `ignore` crate, which is the same one `ripgrep` uses.

Manage patterns with:

```bash
gim ignore mario --add "logs/"
gim ignore mario --remove "logs/"
gim ignore mario --list
gim ignore mario --edit   # opens data/mario/.gignore in $EDITOR
```

---

## Commands

### `gim add`
```
gim add [alias] [game directory]
    --title   [optional: display title]
    --dataDir [optional: data directory]
```

### `gim remove`
```
gim remove [alias]
    --confirm  (required: prevents accidental deletion)
```

### `gim list`
```
gim list
    --details    (optional: show all columns from games.db)
    --json       (optional: output as JSON)
```

### `gim snap`
```
gim snap [alias]
    --id      [optional: custom snapshot ID]
    -m/--msg  [optional: snapshot message]
    -t/--threads [optional: thread count for hashing & copying]
    --dry-run [optional: preview changes without writing]
```

### `gim restore`
```
gim restore [alias] [target snapshot ID]
    --full     [optional: force full copy, skip current-state hashing]
    -t/--threads [optional: thread count]
    --dry-run  [optional: preview changes without modifying files]
```

### `gim status`
```
gim status [alias]
    -t/--threads [optional: thread count]
    --json       [optional: output as JSON]
```

### `gim log`
```
gim log [alias]
    --oneline   [optional: one snapshot per line]
    --json      [optional: output as JSON]
    -n [number] [optional: limit number of entries, default: all]
```

### `gim diff`
```
gim diff [alias] [snapshot ID A] [snapshot ID B]
    --stat      [optional: show summary statistics only]
    --json      [optional: output as JSON]
```

### `gim gc`
```
gim gc [alias]
    --dry-run  [optional: preview without deleting]
```

### `gim ignore`
```
gim ignore [alias]
    --add [pattern]     [add a pattern to per-game .gignore]
    --remove [pattern]  [remove a pattern from per-game .gignore]
    --list              [list all active ignore patterns for this game]
    --edit              [open .gignore in system default editor]
```

---

## Concurrency & atomicity

- **Advisory locking**: a sentinel file `data/[alias]/snaps.db.lock` is
  exclusively locked for the duration of every `snap` and `restore`
  operation. A second concurrent operation fails fast with a clear error
  message.
- **WAL mode**: `snaps.db` uses SQLite WAL journal mode for better concurrent
  read performance.
- **Atomic snap**: the snapshot record, all file rows, and all deleted-file
  rows are inserted inside a single SQLite transaction. On failure the
  transaction rolls back, and any objects already copied to the CAS are
  deleted.
- **Atomic object writes**: every object is written to a `.tmp` sibling,
  fsync'd, and atomically renamed. A crash never leaves a half-written
  object visible.
- **Integrity check**: every `snaps.db` connection runs
  `PRAGMA integrity_check` on open. Corruption is reported with a clear
  error pointing the user at `gim repair`.

---

## Environment variables

| Variable      | Effect                                                              |
|---------------|---------------------------------------------------------------------|
| `G_DATA_DIR`  | Override the data directory (default: `[gim binary dir]/data`).      |
| `NO_COLOR`    | Disable colored output (also auto-disabled when stdout isn't a TTY).|
| `EDITOR`      | Editor used by `g ignore --edit` (default: `vi` / `notepad`).      |

---

## Testing

```bash
# Run all unit tests
cargo test --lib

# Run integration tests
cargo test
```

The unit tests cover: path normalization, XXH3 hashing, ignore-pattern
matching (glob, directory, negation, path-specific), SQLite schema and CRUD
for both databases, CAS deduplication, and the diff algorithm. End-to-end
smoke tests exercise every CLI subcommand against a sandboxed game directory.

---

## Toolchain

- Rust **1.85.0** (edition 2021, `rust-version = "1.85"`)
- All dependencies are pinned to specific minor versions in `Cargo.toml`.

### Key dependencies

| Crate            | Version  | Purpose                                       |
|------------------|----------|-----------------------------------------------|
| `clap`           | 4.5.20   | CLI argument parsing (derive)                 |
| `rusqlite`       | 0.32.1   | SQLite (bundled — no system libsqlite3 needed)|
| `xxhash-rust`    | 0.8.12   | XXH3-128 hashing                              |
| `walkdir`        | 2.5.0    | Recursive directory walking                   |
| `ignore`         | 0.4.23   | gitignore-compatible pattern matching         |
| `rayon`          | 1.10.0   | Parallel hashing pipeline                     |
| `fs2`            | 0.4.3    | Advisory file locking                         |
| `serde`/`serde_json` | 1.0.x | `--json` output                            |
| `chrono`         | 0.4.38   | Timestamp formatting + snapshot IDs           |
| `colored`        | 2.1.0    | Terminal colors                               |
| `anyhow`/`thiserror` | 1.0.x | Error handling                          |

---

## License

MIT.
