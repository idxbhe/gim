# `gim` — Game Files Version Control Tool

A CLI tool for versioning game files. Similar to `git`, but purpose-built for
game directories. Uses **SQLite** for metadata, **XXH3-128** for fast
non-cryptographic file hashing, and a **mtime+size fast pre-filter** so that
`gim status` / `gim snap` on an unchanged game directory completes in
milliseconds instead of minutes.

Built in Rust with a modular, production-ready architecture: per-command
modules, a transactional storage layer, content-addressable object store with
automatic deduplication, parallel hashing via Rayon, and advisory locking to
prevent concurrent mutation.

---

## What's new in v0.2

- **Renamed binary & project** from `g` → `gim` (env var `G_DATA_DIR` → `GIM_DATA_DIR`).
- **mtime + size fast pre-filter** on `gim status`, `gim snap`, `gim restore`:
  files whose `size` and `mtime` match the reference snapshot are not re-hashed.
  On an idle 30 GB game directory with 10,000+ files, `gim status` drops from
  "several minutes" to "milliseconds" because zero files are hashed.
- **`--full-hash` flag** on `gim snap` and `gim status` to bypass the
  pre-filter when the user suspects stored mtimes are misleading.
- **`modifiedTime` column** added to the `files` table. Old `snaps.db`
  databases are auto-migrated on first open.
- **`gim restore` now sets mtime** on restored files to match the snapshot's
  recorded mtime, so the post-restore `gim status` is fast.

---

## Quick start

```bash
# Build
cargo build --release

# The binary is at target/release/gim. Copy it somewhere on your PATH.

# Add a game
gim add mario "C:/Games/Super Mario Bros"

# Take the first snapshot
gim snap mario -m "Initial snapshot"

# Make some changes to the game directory, then snapshot again
gim snap mario -m "Installed texture pack"

# See what changed since the last snapshot (instant if nothing changed)
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

## The mtime + size fast pre-filter

`gim status`, `gim snap`, and `gim restore` (without `--full`) use a two-pass
pipeline:

1. **Walk + stat** (single-threaded): walkdir traverses the game directory,
   applies ignore patterns, and collects `(path, size, mtime)` for each file.
   No file content is read — only `stat()`, which takes milliseconds even for
   10,000+ files.
2. **Smart hash** (parallel via Rayon): for each file:
   - File is NOT in the reference snapshot → hash (new file).
   - File IS in reference, but `size` OR `mtime` differs → hash to verify.
   - File IS in reference AND `size` AND `mtime` match → **skip hashing**,
     reuse the reference hash.

**Result:** for an idle game directory, the hash pass does **zero** file
reads. Only 5 changed files? Only 5 files are hashed, not 10,000.

### Why this is safe

The pre-filter is a **heuristic**, not ground truth. Its correctness rests on:
"if size and mtime are unchanged, the file content is unchanged." This holds
for the overwhelming majority of game files because game engines always
rewrite save/config files wholesale, which updates mtime.

The rare edge case (content changed but mtime preserved by `touch -d` or by
an editor that explicitly preserves mtime) is documented. Users can defeat
it with:

```bash
gim snap mario --full-hash      # force full hashing on snap
gim status mario --full-hash    # force full hashing on status
```

### `gim restore` cooperation

When `gim restore` writes a file back to the game directory, the OS would
normally set its mtime to "now" — which would cause the next `gim status`
to think every restored file had changed. To prevent this, `gim restore`
explicitly sets the mtime of each restored file to the snapshot's recorded
mtime via `filetime::set_file_mtime`. This means the post-restore `gim status`
is as fast as a no-op status check.

---

## Binary directory layout

```
gim
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
with the `GIM_DATA_DIR` environment variable.

---

## Architecture

```
src/
├── main.rs              Binary entry point: parse CLI, dispatch, exit code
├── lib.rs               Library root: re-exports all modules
├── cli.rs               clap derive definitions for every subcommand
├── error.rs             GError enum + exit-code mapping
├── commands/            One module per subcommand
│   ├── snap.rs          Core: walk+stat → smart hash → diff → store → report
│   ├── restore.rs       Core: walk+stat → smart hash → copy/delete in parallel
│   ├── status.rs        Uses smart walk for instant "no changes" result
│   ├── diff.rs          Pure DB query (no walking)
│   └── ...              add, remove, list, log, gc, ignore
├── db/                  SQLite layer
│   ├── games.rs         games.db CRUD
│   ├── snaps.rs         snaps.db CRUD + diff_states() + FileMeta (with mtime)
│   └── schema.rs        Idempotent DDL + auto-migration for modifiedTime
├── storage/cas.rs       Content-addressable store (atomic writes, dedup)
├── hashing/mod.rs       XXH3-128 streaming hash + retry-on-locked-file
├── ignore_mod/mod.rs    gitignore-compatible pattern matching
├── walker/mod.rs        Parallel walk + stat + smart-hash pipeline (Rayon)
├── path_utils/mod.rs    Path normalization
├── locking/mod.rs       Advisory file locks (fs2)
├── config/mod.rs        Path resolution + GIM_DATA_DIR override
├── output/              Colorizer + size/timestamp formatting
└── parallel/mod.rs      Re-exports Rayon
```

### Design principles

- **Modular**: each command, storage primitive, and IO concern is in its own
  module. Adding a new command only touches `commands/mod.rs` plus the new
  file.
- **Pure where possible**: `diff_states()` and `normalize()` are pure
  functions with no side effects and full unit-test coverage.
- **Transactional**: every `gim snap` writes its `snaps` row, `files` rows,
  and `deleted_files` rows inside a single SQLite transaction. On failure the
  transaction rolls back and any objects already copied to the CAS are
  deleted.
- **Streaming**: XXH3 hashing streams files through a 1 MiB buffer, so
  multi-GB game archives are hashed without loading them into memory.
- **Smart pre-filter**: the mtime+size heuristic skips hashing for files
  that haven't changed. `--full-hash` is the escape hatch.
- **Parallel**: the walk→hash pipeline uses Rayon. Worker count is
  configurable with `--threads`; defaults to `num_cpus`.
- **Resilient**: locked files (e.g. when a game is running) are retried 3
  times with 500 ms delay, then skipped with a warning rather than failing
  the whole snapshot.
- **Atomic CAS writes**: every object is written to a `.tmp` sibling, fsync'd,
  and atomically renamed. A crash never leaves a partially-written object.
- **Auto-migration**: old `snaps.db` files (created by gim v0.1) are
  automatically upgraded to include the `modifiedTime` column on first open.

---

## Hashing

- **Algorithm**: XXH3-128 (128-bit, non-cryptographic)
- **Output**: 32-character lowercase hex string
- **Why**: XXH3 runs at multi-GB/s on modern CPUs, ideal for large game files.
  128-bit collision resistance is more than sufficient for file-integrity
  verification in the threat model of a personal backup tool.

---

## Path normalization

All paths stored in the database follow these rules:

1. Relative to the game directory root.
2. Forward slash `/` as the directory separator on all platforms.
3. No leading or trailing slash.
4. UTF-8 encoded.

Normalization is applied at both `snap` time and `restore` time.

---

## Ignore patterns

Ignore patterns are evaluated in order and merged:

1. **Built-in defaults** (`*.tmp`, `*.temp`, `*.bak`, `*.swp`, `Thumbs.db`,
   `.DS_Store`, `desktop.ini`) — always applied.
2. **Global** (`data/gignore`) — applies to all games.
3. **Per-game** (`data/[alias]/.gignore`) — applies to a specific game.
4. **In-game** (`[gameDir]/.gignore`) — lives inside the game directory.

Pattern syntax follows gitignore semantics. The matching engine is the
`ignore` crate (same one `ripgrep` uses).

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
    --id         [optional: custom snapshot ID]
    -m/--msg     [optional: snapshot message]
    -t/--threads [optional: thread count for hashing]
    --dry-run    [optional: preview changes without writing]
    --full-hash  [optional: bypass mtime+size pre-filter, hash every file]
```

### `gim restore`
```
gim restore [alias] [target snapshot ID]
    --full       [optional: skip current-state hashing, overwrite everything]
    -t/--threads [optional: thread count]
    --dry-run    [optional: preview changes without modifying files]
```

### `gim status`
```
gim status [alias]
    -t/--threads [optional: thread count]
    --json       [optional: output as JSON]
    --full-hash  [optional: bypass mtime+size pre-filter, hash every file]
```

### `gim log`
```
gim log [alias]
    --oneline   [optional: one snapshot per line]
    --json      [optional: output as JSON]
    -n [number] [optional: limit number of entries]
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
    --add [pattern]
    --remove [pattern]
    --list
    --edit   [opens .gignore in $EDITOR]
```

---

## Concurrency & atomicity

- **Advisory locking**: a sentinel file `data/[alias]/snaps.db.lock` is
  exclusively locked for the duration of every `snap` and `restore`.
- **WAL mode**: `snaps.db` uses SQLite WAL journal mode.
- **Atomic snap**: the snapshot record, all file rows, and all deleted-file
  rows are inserted inside a single SQLite transaction.
- **Atomic object writes**: every object is written to a `.tmp` sibling,
  fsync'd, and atomically renamed.
- **Integrity check**: every `snaps.db` connection runs
  `PRAGMA integrity_check` on open.

---

## Environment variables

| Variable       | Effect                                                              |
|----------------|---------------------------------------------------------------------|
| `GIM_DATA_DIR` | Override the data directory (default: `[gim binary dir]/data`).    |
| `NO_COLOR`     | Disable colored output (also auto-disabled when stdout isn't a TTY).|
| `EDITOR`       | Editor used by `gim ignore --edit` (default: `vi` / `notepad`).    |

---

## Testing

```bash
cargo test --lib     # 29 unit tests
cargo test           # includes integration tests
```

---

## Toolchain

- Rust **1.85.0** (edition 2021, `rust-version = "1.85"`)
- All dependencies are pinned to specific minor versions in `Cargo.toml`.

### Key dependencies

| Crate               | Version  | Purpose                                       |
|---------------------|----------|-----------------------------------------------|
| `clap`              | 4.5.20   | CLI argument parsing (derive)                 |
| `rusqlite`          | 0.32.1   | SQLite (bundled — no system libsqlite3 needed)|
| `xxhash-rust`       | 0.8.12   | XXH3-128 hashing                              |
| `walkdir`           | 2.5.0    | Recursive directory walking                   |
| `ignore`            | 0.4.23   | gitignore-compatible pattern matching         |
| `rayon`             | 1.10.0   | Parallel hashing pipeline                     |
| `filetime`          | 0.2.25   | Set mtime on restored files                   |
| `fs2`               | 0.4.3    | Advisory file locking                         |
| `serde`/`serde_json`| 1.0.x    | `--json` output                               |
| `chrono`            | 0.4.38   | Timestamp formatting + snapshot IDs           |
| `colored`           | 2.1.0    | Terminal colors                               |
| `anyhow`/`thiserror`| 1.0.x    | Error handling                                |

---

## License

MIT.
