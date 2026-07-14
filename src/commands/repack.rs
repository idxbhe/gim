//! `gim repack` — compress snapshots + CAS objects into portable archive.

use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::{Colorizer, ProgressReporter};
use crate::output::format_size;
use crate::repack::{CompressionConfig, CompressionProfile, GimFile, GimManifest, GimObject, GimSnapshot, GimGameInfo, GimCompressionInfo, GimObjectsFile, Xtool};
use crate::storage::Cas;
use std::io::Write;
use std::path::PathBuf;

pub fn run(
    c: &Colorizer,
    alias: String,
    profile: String,
    level: Option<u32>,
    snapshots: Option<Vec<String>>,
    threads: Option<usize>,
    memory: Option<u64>,
    output: Option<PathBuf>,
    dry_run: bool,
    progress: &ProgressReporter,
) -> GResult<()> {
    progress.phase_start("preparing", 0);

    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb = SnapsDb::open(&paths.snaps_db(&alias))?;

    // Load config.
    let cfg = GimConfig::load_game(&paths, &alias)?;

    // Parse compression profile.
    let profile = CompressionProfile::parse(&profile)?;
    let mut comp_config = CompressionConfig::new(profile, level);
    if let Some(t) = threads { comp_config.threads = t; }
    if let Some(m) = memory { comp_config.memory_mb = m; }

    // Determine which snapshots to repack.
    let all_snaps = sdb.list_snapshots()?;
    let snaps_to_pack: Vec<&crate::db::Snap> = match &snapshots {
        Some(ids) => {
            let mut out = Vec::new();
            for id in ids {
                let snap = sdb.get_snapshot(id)?.ok_or_else(|| GError::SnapshotNotFound(id.clone(), alias.clone()))?;
                out.push(all_snaps.iter().find(|s| &s.snapshot_id == id).unwrap());
            }
            out
        }
        None => all_snaps.iter().collect(),
    };

    if snaps_to_pack.is_empty() {
        progress.phase_cancel();
        return Err(GError::NoSnapshots(alias.clone()));
    }

    // Determine output directory.
    let output_dir = output.unwrap_or_else(|| {
        paths.binary_dir.join("repacked").join(&game.title)
    });

    progress.phase_cancel();

    if dry_run {
        println!("dry run: would repack {} snapshot(s) for \"{}\"", snaps_to_pack.len(), game.title);
        println!("  profile: {}", comp_config.profile);
        println!("  level: {}", comp_config.level);
        println!("  threads: {}", comp_config.threads);
        println!("  memory: {}mb", comp_config.memory_mb);
        println!("  codecs: {}", comp_config.profile.codec_string());
        println!("  output: {}", output_dir.display());
        println!("\n  snapshots:");
        for s in &snaps_to_pack {
            println!("    {} ({}, {} files)", s.snapshot_id, s.message.as_deref().unwrap_or("(no message)"), s.file_count);
        }
        return Ok(());
    }

    // Find xtool.
    let xtool = Xtool::find(&paths.binary_dir)?;

    // Create output directory.
    std::fs::create_dir_all(&output_dir)?;

    // ── Phase 1: Collect all unique CAS objects ────────────────────
    progress.phase_start("collecting objects", 0);
    let cas = Cas::new(paths.objects_dir(&alias));
    cas.ensure()?;
    let mut all_hashes: std::collections::HashSet<String> = std::collections::HashSet::new();
    for snap in &snaps_to_pack {
        let files = sdb.files_for_snapshot(&snap.snapshot_id)?;
        for (_, meta) in &files {
            all_hashes.insert(meta.hash.0.clone());
        }
        progress.phase_tick();
    }
    let hash_list: Vec<String> = all_hashes.into_iter().collect();
    progress.phase_cancel();

    // ── Phase 2: Precompress objects → objects.bin ─────────────────
    progress.phase_start("packing objects", hash_list.len());
    let objects_file = output_dir.join("objects.bin");
    let mut obj_entries: Vec<GimObject> = Vec::with_capacity(hash_list.len());
    let mut obj_offset: u64 = 0;

    // We pack all objects into a single concatenated file, then
    // precompress the whole thing at once (xtool processes stdin → stdout).
    // But xtool's precomp works on a single stream. For per-object
    // compression, we'd need to call xtool per object (slow).
    //
    // Better approach: concatenate all objects into a temp file, then
    // precompress the temp file → objects.bin. Record offsets.
    let temp_cat = output_dir.join(".objects.tmp");
    {
        let mut f = std::fs::File::create(&temp_cat)?;
        for hash in &hash_list {
            let h = crate::hashing::Hash(hash.clone());
            let mut obj_file = cas.open(&h)?;
            let offset = obj_offset;
            let written = std::io::copy(&mut obj_file, &mut f)?;
            obj_entries.push(GimObject {
                hash: hash.clone(),
                offset,
                compressed_size: written, // will update after precompress
                orig_size: written,
            });
            obj_offset += written;
            progress.phase_tick();
        }
        f.sync_all()?;
    }

    // Precompress the concatenated objects.
    let encode_args = comp_config.xtool_encode_args();
    progress.phase_cancel();
    progress.phase_start("compressing objects", 0);
    xtool.encode(&temp_cat, &objects_file, &encode_args)?;
    let _ = std::fs::remove_file(&temp_cat);
    progress.phase_cancel();

    // Update compressed_size in obj_entries based on actual output size.
    // Since we precompressed the whole file at once, individual offsets
    // in the compressed file are NOT the same as in the uncompressed file.
    // We need to store objects as: [hash][orig_size] + precompressed data,
    // OR store offsets in the UNCOMPRESSED stream and decode the whole
    // file during unpack.
    //
    // Simplest correct approach: during unpack, decode the entire
    // objects.bin to a temp file, then read objects by offset/size
    // from the decoded file. This means obj_entries store offsets
    // in the DECODED (original) stream.
    //
    // We already have the offsets from the concatenation. The
    // compressed_size field is not per-object (whole-file compressed).
    // Let's store the decoded offsets and orig_size, and during unpack
    // we decode the whole file first.

    // ── Phase 3: Pack each snapshot's file list ────────────────────
    let mut snap_entries: Vec<GimSnapshot> = Vec::with_capacity(snaps_to_pack.len());
    for snap in &snaps_to_pack {
        progress.phase_start(&format!("packing {}", snap.snapshot_id), 0);

        let files = sdb.files_for_snapshot(&snap.snapshot_id)?;
        let gim_files: Vec<GimFile> = files.iter().map(|(path, meta)| GimFile {
            path: path.clone(),
            hash: meta.hash.0.clone(),
            size: meta.file_size,
            mtime: meta.modified_time,
        }).collect();

        // Serialize file list to JSON, precompress, write to file.
        let snap_json = serde_json::to_vec(&gim_files)?;
        let snap_tmp = output_dir.join(format!(".{}.tmp", snap.snapshot_id));
        let snap_bin = output_dir.join(format!("{}.bin", snap.snapshot_id));
        std::fs::write(&snap_tmp, &snap_json)?;
        xtool.encode(&snap_tmp, &snap_bin, &encode_args)?;
        let _ = std::fs::remove_file(&snap_tmp);

        snap_entries.push(GimSnapshot {
            id: snap.snapshot_id.clone(),
            parent: snap.parent_snap_id.clone(),
            timestamp: snap.timestamp,
            message: snap.message.clone(),
            file_count: snap.file_count,
            added_size: snap.added_size,
            data_file: format!("{}.bin", snap.snapshot_id),
            files: gim_files,
        });
        progress.phase_cancel();
    }

    // ── Phase 4: Write manifest .gim ────────────────────────────────
    progress.phase_start("writing manifest", 0);
    let manifest = GimManifest {
        version: 1,
        game: GimGameInfo {
            title: game.title.clone(),
            alias: alias.clone(),
        },
        config: serde_json::json!({
            "hash.algorithm": cfg.get("hash.algorithm"),
        }),
        compression: GimCompressionInfo {
            profile: comp_config.profile.as_str().to_string(),
            level: comp_config.level,
            codecs: comp_config.profile.codecs(),
            chunk_size: comp_config.profile.chunk_size().to_string(),
            xtool_version: "0.7.9".to_string(),
        },
        snapshots: snap_entries,
        objects: GimObjectsFile {
            file: "objects.bin".to_string(),
            entries: obj_entries,
        },
    };

    let gim_path = output_dir.join("game.gim");
    std::fs::write(&gim_path, manifest.to_json()?)?;
    progress.phase_cancel();

    // Report.
    let objects_size = std::fs::metadata(&objects_file)?.len();
    println!("repacked {} → {}", c.green(&alias), c.bold(&output_dir.display().to_string()));
    println!("  {} snapshots, {} objects", manifest.snapshots.len(), manifest.objects.entries.len());
    println!("  objects.bin: {} (compressed)", format_size(objects_size as i64));
    println!("  manifest: {}", gim_path.display());

    Ok(())
}
