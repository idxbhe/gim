# `gim` — Game Files Version Control Tool

A CLI tool for versioning game files. Similar to `git`, but purpose-built for
game directories. Uses **SQLite** for metadata, **XXH3-128** for fast
non-cryptographic file hashing, a **mtime+size fast pre-filter** for instant
status checks, **branches** for parallel timelines, and safe **snapshot
deletion** with automatic re-parenting.

---

## What's new in v0.3

- **Snapshot deletion** (`gim delete`): safely remove a snapshot from the
  chain. Children are automatically re-parented to the deleted snapshot's
  parent. Refuses to delete snapshots referenced by branches. Deleting the
  root ("original") requires `--force`.
- **Branches** (`gim branch`): create, delete, switch, and list named
  pointers to snapshots. `gim snap` advances the current branch. Switching
  branches restores the target snapshot to disk (with uncommitted-change
  detection). The "main" branch is auto-created and protected.
- **Snapshot ID collision fix**: two snaps within the same second now get
  `-2`, `-3`, ... suffixes instead of failing.

## What was new in v0.2

- **Renamed** binary & project from `g` → `gim` (env var `GIM_DATA_DIR`).
- **mtime + size fast pre-filter**: files whose size and mtime match the
  reference snapshot are not re-hashed. `gim status` on an unchanged 30 GB
  game directory completes in milliseconds.
- **`--full-hash` flag** on `gim snap` and `gim status` to bypass the
  pre-filter.
- **`modifiedTime` column** in the `files` table (auto-migrated from v0.1).
- **`gim restore` sets mtime** on restored files for fast post-restore status.

---

## Quick start

```bash
cargo build --release
# Binary: target/release/gim

gim add mario "/path/to/game"
gim snap mario -m "Initial snapshot"
gim snap mario -m "After mod install"
gim status mario              # instant if nothing changed
gim log mario

# Branches
gim branch mario --create experimental
gim branch mario --switch experimental
# ...make changes, snap on experimental...
gim branch mario --switch main --force  # discard experimental changes

# Delete a snapshot
gim delete mario <snapshot-id> --dry-run
gim delete mario <snapshot-id>

# Restore
gim restore mario original --full

# Garbage-collect orphaned objects
gim gc mario

# Remove a game entirely
gim remove mario --confirm
```

---

## The mtime + size fast pre-filter

`gim status`, `gim snap`, and `gim restore` (without `--full`) use a two-pass
pipeline:

1. **Walk + stat** (single-threaded): collect `(path, size, mtime)` for each
   file. No file content is read — only `stat()`.
2. **Smart hash** (parallel via Rayon): for each file:
   - Not in reference snapshot → hash (new file).
   - In reference, but size OR mtime differs → hash to verify.
   - In reference AND size AND mtime match → **skip hashing**, reuse the
     reference hash.

For an idle game directory, the hash pass does **zero** file reads. Use
`--full-hash` to bypass the pre-filter when you suspect stored mtimes are
misleading.

`gim restore` sets the mtime of restored files to the snapshot's recorded
mtime, so the post-restore `gim status` is also fast.

---

## Branches

A branch is a named, movable pointer to a snapshot. Exactly one branch is
"current" at any time. `gim snap` creates a new snapshot whose parent is the
current branch's snapshot, then advances the current branch.

```
gim branch [alias]                           # list branches
gim branch [alias] --create [name]           # create pointing to current branch's snapshot
gim branch [alias] --create [name] --from [snapshot-id]
gim branch [alias] --delete [name]           # refuse if current or "main"
gim branch [alias] --switch [name]           # restore + update current branch
gim branch [alias] --switch [name] --force   # discard uncommitted changes
```

**Edge cases handled:**
- Creating a duplicate branch name → `BranchExists` error.
- Deleting the current branch → refused; must switch first.
- Deleting "main" → refused (protected).
- Switching with uncommitted changes → refused unless `--force`.
- Switching to the current branch → no-op.
- `--from` pointing to a non-existent snapshot → `SnapshotNotFound`.
- Legacy DB without branches → auto-migrated: "main" created pointing to
  latest snapshot.

---

## Snapshot deletion

```
gim delete [alias] [snapshot-id]
    --dry-run    # preview
    --force      # required to delete the root ("original")
```

**Semantics:**
- The snapshot's `files` and `deleted_files` rows are deleted.
- Children (snapshots whose `parentSnapId` points to this one) are
  **re-parented** to the deleted snapshot's parent (could be NULL).
- Refuses if any branch points to the snapshot (must move/delete branch first).
- Deleting "original" (root) requires `--force`; children become new roots.
- Orphaned objects in `objects/` are NOT deleted here — run `gim gc` to
  reclaim disk space.
- Everything happens in a single SQLite transaction (atomic).

**Why re-parenting is safe:** every snapshot stores its full file set (not
deltas). The `parentSnapId` is only used for history traversal and `gim log`
display — it doesn't affect `gim restore` correctness.

---

## Binary directory layout

```
gim
data/
  games.db                                  global game registry
  gignore                                   global ignore patterns (optional)
  [alias]/
    snaps.db                                snapshot + file + branch + meta tables
    objects/                                content-addressable file store
      ab/cdef0123456789abcdef...            [hash_prefix]/[hash]
    .gignore                                per-game ignore patterns (optional)
```

Override the data directory with `GIM_DATA_DIR`.

---

## Commands

| Command | Description |
|---------|-------------|
| `gim add` | Register a game for tracking. |
| `gim remove` | Remove a game and all its data (`--confirm` required). |
| `gim list` | List tracked games (`--details`, `--json`). |
| `gim snap` | Take a snapshot (`--full-hash`, `--dry-run`, `-m`, `-t`). |
| `gim restore` | Restore to a snapshot (`--full`, `--dry-run`, `-t`). |
| `gim status` | Show changes since last snapshot (`--full-hash`, `--json`). |
| `gim log` | Show snapshot history (`--oneline`, `--json`, `-n`). |
| `gim diff` | Compare two snapshots (`--stat`, `--json`). |
| `gim delete` | Delete a snapshot (`--dry-run`, `--force`). |
| `gim branch` | Manage branches (`--create`, `--delete`, `--switch`, `--from`, `--force`). |
| `gim gc` | Garbage-collect orphaned objects (`--dry-run`). |
| `gim ignore` | Manage ignore patterns (`--add`, `--remove`, `--list`, `--edit`). |

---

## Architecture

```
src/
├── main.rs              Entry point
├── lib.rs               Library root
├── cli.rs               clap derive definitions
├── error.rs             GError enum + exit codes (includes branch/delete errors)
├── commands/            One module per subcommand
│   ├── snap.rs          Walk+stat → smart hash → diff → store → advance branch
│   ├── restore.rs       Walk+stat → smart hash → copy/delete in parallel + set mtime
│   ├── status.rs        Smart walk vs current branch's snapshot
│   ├── delete.rs        Re-parent children + refuse if branch refs + atomic tx
│   ├── branch.rs        Create/delete/switch/list + uncommitted-change detection
│   ├── log_cmd.rs       History with branch markers
│   └── ...              add, remove, list, diff, gc, ignore
├── db/
│   ├── games.rs         games.db CRUD
│   ├── snaps.rs         snaps.db CRUD + branches + meta + diff_states + children_of
│   └── schema.rs        DDL + auto-migration (modifiedTime, branches, meta)
├── storage/cas.rs       Content-addressable store (atomic writes, dedup)
├── hashing/mod.rs       XXH3-128 streaming hash + retry
├── ignore_mod/mod.rs    gitignore-compatible matching
├── walker/mod.rs        Parallel walk + stat + smart-hash pipeline
├── path_utils/mod.rs    Path normalization
├── locking/mod.rs       Advisory file locks
├── config/mod.rs        Path resolution + GIM_DATA_DIR
├── output/              Colorizer + formatting
└── parallel/mod.rs      Rayon re-exports
```

---

## Concurrency & atomicity

- **Advisory locking**: `data/[alias]/snaps.db.lock` is exclusively locked
  for `snap`, `restore`, `delete`, and `branch --switch`.
- **WAL mode** + `PRAGMA integrity_check` on every connection.
- **Atomic snap**: snapshot row + files + deleted_files + branch advance in
  one transaction.
- **Atomic delete**: re-parenting + file deletion + snap deletion in one
  transaction.
- **Atomic CAS writes**: `.tmp` → fsync → rename.

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `GIM_DATA_DIR` | Override data directory. |
| `NO_COLOR` | Disable colored output. |
| `EDITOR` | Editor for `gim ignore --edit`. |

---

## Testing

```bash
cargo test --lib     # 35 unit tests
```

Tests cover: path normalization, XXH3 hashing, ignore patterns, SQLite
schema + migrations, branch CRUD, current-branch roundtrip, children-of
queries, CAS dedup, diff algorithm, and the mtime+size smart pre-filter.

---

## Toolchain

- Rust **1.85.0** (edition 2021)
- All dependencies pinned to specific minor versions.

### Key dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `clap` | 4.5.20 | CLI parsing |
| `rusqlite` | 0.32.1 | SQLite (bundled) |
| `xxhash-rust` | 0.8.12 | XXH3-128 hashing |
| `walkdir` | 2.5.0 | Directory walking |
| `ignore` | 0.4.23 | gitignore matching |
| `rayon` | 1.10.0 | Parallel hashing |
| `filetime` | 0.2.25 | Set mtime on restored files |
| `fs2` | 0.4.3 | Advisory locking |
| `serde`/`serde_json` | 1.0.x | JSON output |
| `chrono` | 0.4.38 | Timestamps + snapshot IDs |

---

## License

MIT.
