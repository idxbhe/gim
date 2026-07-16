//! `gim repack` — compress snapshots + CAS objects into portable archive.
//!
//! Pipeline:
//! 1. Collect all unique CAS objects → concatenate → temp file
//! 2. Layer 1: xtool precomp (precompress streams) → temp file
//! 3. Layer 2: compress (zstd/lzma/lz4) → objects.bin
//! 4. For each snapshot: serialize file list → xtool precomp → compress → .bin
//! 5. Write manifest .gim

use crate::config::{env_data_dir_override, GimConfig, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::{Colorizer, ProgressReporter};
use crate::output::format_size;
use crate::repack::{compress_file, CompressAlgorithm, ProfileFile, GimFile, GimGameInfo, GimCompressionInfo, GimManifest, GimObject, GimObjectsFile, GimSnapshot, Xtool};
use crate::storage::Cas;
use std::path::PathBuf;

pub fn run(
    c: &Colorizer,
    alias: Option<String>,
    profile_name: Option<String>,
    list_profiles: bool,
    level: Option<u32>,
    snapshots: Option<Vec<String>>,
    threads: Option<usize>,
    _memory: Option<u64>,
    output: Option<PathBuf>,
    dry_run: bool,
    progress: &ProgressReporter,
) -> GResult<()> {
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    paths.ensure_data_dir()?;

    let profiles_dir = paths.binary_dir.join("xtool").join("profiles");
    ProfileFile::ensure_dir(&profiles_dir)?;

    // ── --list-profiles ────────────────────────────────────────────
    if list_profiles || (alias.is_none() && profile_name.is_none()) {
        let profiles = ProfileFile::list_all(&profiles_dir)?;
        if profiles.is_empty() {
            println!("no profiles found in {}", profiles_dir.display());
            return Ok(());
        }
        println!("available compression profiles:\n");
        for (filename, p) in &profiles {
            println!("  {} ({})", c.bold(&p.name), c.dim(filename));
            println!("    {}", p.description);
            println!("    {}", c.dim(&p.summary()));
            println!();
        }
        return Ok(());
    }

    let alias = alias.ok_or_else(|| GError::Other(
        "alias is required. Use --list-profiles to list available profiles.".into()
    ))?;

    // ── Load profile ───────────────────────────────────────────────
    let profile_name = profile_name.as_deref().unwrap_or("zstd");
    let profile = ProfileFile::load_by_name(&profiles_dir, profile_name)?;

    // ── Resolve game + snapshots ───────────────────────────────────
    progress.phase_start("preparing", 0);
    let gdb = GamesDb::open(&paths.games_db)?;
    let game = gdb.get(&alias)?.ok_or_else(|| GError::AliasNotFound(alias.clone()))?;
    let sdb = SnapsDb::open(&paths.snaps_db(&alias))?;
    let cfg = GimConfig::load_game(&paths, &alias)?;

    let all_snaps = sdb.list_snapshots()?;
    let snaps_to_pack: Vec<&crate::db::Snap> = match &snapshots {
        Some(ids) => {
            let mut out = Vec::new();
            for id in ids {
                let _ = sdb.get_snapshot(id)?.ok_or_else(|| GError::SnapshotNotFound(id.clone(), alias.clone()))?;
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

    let output_dir = output.unwrap_or_else(|| {
        paths.binary_dir.join("repacked").join(&game.title)
    });

    progress.phase_cancel();

    // Parse compression algorithm from profile.
    let compress_algo = CompressAlgorithm::parse(&profile.compress.algorithm)?;
    let compress_level = level.unwrap_or(profile.compress.level);
    compress_algo.validate_level_or_default(compress_level);

    if dry_run {
        println!("dry run: would repack {} snapshot(s) for \"{}\"", snaps_to_pack.len(), game.title);
        println!("  profile: {} ({})", profile.name, profile.summary());
        if let Some(l) = level { println!("  level override: {}", l); }
        println!("  output: {}", output_dir.display());
        println!("\n  snapshots:");
        for s in &snaps_to_pack {
            println!("    {} ({}, {} files)", s.snapshot_id, s.message.as_deref().unwrap_or("(no message)"), s.file_count);
        }
        return Ok(());
    }

    // Find xtool.
    let xtool = Xtool::find(&paths.binary_dir)?;
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

    // ── Phase 2: Concatenate objects → temp ────────────────────────
    progress.phase_start("packing objects", hash_list.len());
    let objects_raw = output_dir.join(".objects.raw");
    let mut obj_entries: Vec<GimObject> = Vec::with_capacity(hash_list.len());
    let mut obj_offset: u64 = 0;
    {
        let mut f = std::fs::File::create(&objects_raw)?;
        for hash in &hash_list {
            let h = crate::hashing::Hash(hash.clone());
            let mut obj_file = cas.open(&h)?;
            let offset = obj_offset;
            let written = std::io::copy(&mut obj_file, &mut f)?;
            obj_entries.push(GimObject {
                hash: hash.clone(), offset,
                compressed_size: written, orig_size: written,
            });
            obj_offset += written;
            progress.phase_tick();
        }
        f.sync_all()?;
    }
    progress.phase_cancel();

    // ── Phase 3: Layer 1 — xtool precomp ───────────────────────────
    let xtool_args = profile.xtool_encode_args(threads);
    progress.phase_start("precompressing (xtool)", 0);
    let objects_precomp = output_dir.join(".objects.precomp");
    xtool.encode(&objects_raw, &objects_precomp, &xtool_args)?;
    let _ = std::fs::remove_file(&objects_raw);
    progress.phase_cancel();

    // ── Phase 4: Layer 2 — compress ────────────────────────────────
    progress.phase_start("compressing", 0);
    let objects_file = output_dir.join("objects.bin");
    compress_file(&objects_precomp, &objects_file, compress_algo, compress_level)?;
    let _ = std::fs::remove_file(&objects_precomp);
    progress.phase_cancel();

    // ── Phase 5: Pack each snapshot's file list ───────────────────
    let mut snap_entries: Vec<GimSnapshot> = Vec::with_capacity(snaps_to_pack.len());
    for snap in &snaps_to_pack {
        progress.phase_start(&format!("packing {}", snap.snapshot_id), 0);
        let files = sdb.files_for_snapshot(&snap.snapshot_id)?;
        let gim_files: Vec<GimFile> = files.iter().map(|(path, meta)| GimFile {
            path: path.clone(), hash: meta.hash.0.clone(),
            size: meta.file_size, mtime: meta.modified_time,
        }).collect();

        // Serialize → xtool precomp → compress → .bin
        let snap_json = serde_json::to_vec(&gim_files)?;
        let snap_raw = output_dir.join(format!(".{}.raw", snap.snapshot_id));
        let snap_precomp = output_dir.join(format!(".{}.precomp", snap.snapshot_id));
        let snap_bin = output_dir.join(format!("{}.bin", snap.snapshot_id));

        std::fs::write(&snap_raw, &snap_json)?;
        xtool.encode(&snap_raw, &snap_precomp, &xtool_args)?;
        let _ = std::fs::remove_file(&snap_raw);
        compress_file(&snap_precomp, &snap_bin, compress_algo, compress_level)?;
        let _ = std::fs::remove_file(&snap_precomp);

        snap_entries.push(GimSnapshot {
            id: snap.snapshot_id.clone(), parent: snap.parent_snap_id.clone(),
            timestamp: snap.timestamp, message: snap.message.clone(),
            file_count: snap.file_count, added_size: snap.added_size,
            data_file: format!("{}.bin", snap.snapshot_id), files: gim_files,
        });
        progress.phase_cancel();
    }

    // ── Phase 6: Write manifest ────────────────────────────────────
    progress.phase_start("writing manifest", 0);
    let manifest = GimManifest {
        version: 1,
        game: GimGameInfo { title: game.title.clone(), alias: alias.clone() },
        config: serde_json::json!({
            "hash.algorithm": cfg.get("hash.algorithm"),
        }),
        compression: GimCompressionInfo {
            profile: profile.name.clone(),
            algorithm: compress_algo.as_str().to_string(),
            level: compress_level,
            precomp_codecs: profile.precomp.codecs.clone(),
            chunk_size: profile.precomp.chunk_size.clone(),
            dedup: profile.precomp.dedup,
            xtool_version: "0.7.9".to_string(),
        },
        snapshots: snap_entries,
        objects: GimObjectsFile { file: "objects.bin".to_string(), entries: obj_entries },
    };
    let gim_path = output_dir.join("game.gim");
    std::fs::write(&gim_path, manifest.to_json()?)?;
    progress.phase_cancel();

    let objects_size = std::fs::metadata(&objects_file)?.len();
    println!("repacked {} → {}", c.green(&alias), c.bold(&output_dir.display().to_string()));
    println!("  profile: {} ({})", profile.name, profile.summary());
    println!("  {} snapshots, {} objects", manifest.snapshots.len(), manifest.objects.entries.len());
    println!("  objects.bin: {} (compressed)", format_size(objects_size as i64));
    println!("  manifest: {}", gim_path.display());

    Ok(())
}
