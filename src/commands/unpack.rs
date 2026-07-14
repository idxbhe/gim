//! `gim unpack` / `gim install` — unpack a .gim archive.

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::{Colorizer, ProgressReporter};
use crate::repack::{CompressionConfig, GimManifest, Xtool};
use rayon::prelude::*;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub fn run(
    c: &Colorizer,
    gim_file: PathBuf,
    output_dir: PathBuf,
    snapshot: Option<String>,
    track: bool,
    threads: Option<usize>,
    dry_run: bool,
    is_install: bool,
    interactive: bool,
    progress: &ProgressReporter,
) -> GResult<()> {
    // ── Read manifest ───────────────────────────────────────────────
    progress.phase_start("reading manifest", 0);
    if !gim_file.exists() {
        progress.phase_cancel();
        return Err(GError::Other(format!("manifest file not found: {}", gim_file.display())));
    }
    let manifest_str = fs::read_to_string(&gim_file)?;
    let manifest = GimManifest::from_json(&manifest_str)?;
    progress.phase_cancel();

    // Determine snapshot to unpack.
    let snap = match &snapshot {
        Some(id) => manifest.find_snapshot(id).ok_or_else(|| GError::SnapshotNotFound(id.clone(), manifest.game.title.clone()))?,
        None => manifest.snapshots.last().ok_or_else(|| GError::Other("no snapshots in manifest".into()))?,
    };

    // Interactive setup.
    if interactive && !dry_run {
        run_interactive_setup(&manifest, &mut output_dir.clone(), &snapshot, &mut track.clone())?;
    }

    // Determine xtool location.
    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    let xtool = Xtool::find(&paths.binary_dir)?;

    // Compression config for decode (threads only matter).
    let mut comp_config = CompressionConfig::default();
    if let Some(t) = threads { comp_config.threads = t; }

    if dry_run {
        println!("dry run: would unpack \"{}\" from {}", c.green(&manifest.game.title), gim_file.display());
        println!("  snapshot: {} ({}, {} files)", snap.id, snap.message.as_deref().unwrap_or("(no message)"), snap.file_count);
        println!("  output: {}", output_dir.display());
        println!("  objects: {} unique", manifest.objects.entries.len());
        if track { println!("  tracking: will add to gim registry"); }
        if is_install { println!("  install: will register game + create shortcut"); }
        return Ok(());
    }

    // Create output directory.
    fs::create_dir_all(&output_dir)?;

    // ── Phase 1: Decode objects.bin ────────────────────────────────
    progress.phase_start("decoding objects", 0);
    let manifest_dir = gim_file.parent().ok_or_else(|| GError::Other("cannot determine manifest directory".into()))?;
    let objects_bin = manifest_dir.join(&manifest.objects.file);
    let objects_decoded = output_dir.join(".objects.decoded.tmp");

    let decode_args = comp_config.xtool_decode_args();
    xtool.decode(&objects_bin, &objects_decoded, &decode_args)?;
    progress.phase_cancel();

    // ── Phase 2: Restore files from objects ─────────────────────────
    progress.phase_start("unpacking files", snap.files.len() as usize);

    // Build hash → offset map for object lookup.
    let obj_map: std::collections::HashMap<String, (u64, u64)> = manifest.objects.entries.iter()
        .map(|e| (e.hash.clone(), (e.offset, e.orig_size)))
        .collect();

    // Open decoded objects file for reading.
    let objects_file = fs::File::open(&objects_decoded)?;

    // Restore files in parallel.
    let results: Vec<Result<(), String>> = snap.files.par_iter().map(|f| {
        let (offset, size) = match obj_map.get(&f.hash) {
            Some(v) => *v,
            None => return Err(format!("object {} not found for {}", f.hash, f.path)),
        };

        let abs = output_dir.join(&f.path);
        if let Some(parent) = abs.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Err(format!("mkdir {}: {e}", f.path));
            }
        }

        // Read object data from decoded file.
        use std::io::{Seek, SeekFrom};
        let mut obj_file = objects_file.try_clone().map_err(|e| format!("clone objects: {e}"))?;
        obj_file.seek(SeekFrom::Start(offset)).map_err(|e| format!("seek: {e}"))?;
        let mut data = vec![0u8; size as usize];
        obj_file.read_exact(&mut data).map_err(|e| format!("read: {e}"))?;

        // Verify hash integrity.
        let actual_hash = crate::hashing::hash_bytes(&data, manifest.config.get("hash.algorithm")
            .and_then(|v| v.as_str()).unwrap_or("xxhash").parse().unwrap_or(crate::hashing::HashAlgorithm::Xxhash));
        if actual_hash.0 != f.hash {
            return Err(format!("hash mismatch for {}: expected {}, got {}", f.path, f.hash, actual_hash.0));
        }

        // Write file.
        let mut dst = fs::File::create(&abs).map_err(|e| format!("create {}: {e}", f.path))?;
        dst.write_all(&data).map_err(|e| format!("write {}: {e}", f.path))?;

        // Set mtime.
        if f.mtime > 0 {
            let _ = filetime::set_file_mtime(&abs, filetime::FileTime::from_unix_time(f.mtime, 0));
        }

        progress.phase_tick();
        Ok(())
    }).collect();

    progress.phase_cancel();

    // Cleanup temp file.
    let _ = fs::remove_file(&objects_decoded);

    // Report errors.
    let mut errors = Vec::new();
    for r in results {
        if let Err(e) = r { errors.push(e); }
    }

    println!("unpacked {} → {}", c.green(&manifest.game.title), c.bold(&output_dir.display().to_string()));
    println!("  snapshot: {} ({} files)", snap.id, snap.files.len());
    if !errors.is_empty() {
        eprintln!("warning: {} error(s):", errors.len());
        for e in &errors { eprintln!("  {e}"); }
    }

    // ── Tracking (add to gim registry) ─────────────────────────────
    if track {
        progress.phase_start("registering game", 0);
        register_game(&manifest, &output_dir, &paths)?;
        progress.phase_cancel();
        println!("  registered as \"{}\"", c.green(&manifest.game.alias));
    }

    // ── Install: create shortcut + registry ─────────────────────────
    if is_install {
        progress.phase_start("creating shortcut", 0);
        create_shortcut(&manifest, &output_dir)?;
        progress.phase_cancel();
        println!("  shortcut created");
    }

    Ok(())
}

/// Register the unpacked game in gim's games.db.
fn register_game(manifest: &GimManifest, game_dir: &Path, paths: &Paths) -> GResult<()> {
    let gdb = GamesDb::open(&paths.games_db)?;
    // Check if alias already exists.
    if gdb.get(&manifest.game.alias)?.is_some() {
        // Skip — already tracked.
        return Ok(());
    }
    let data_dir = paths.game_data_dir(&manifest.game.alias);
    gdb.add(&manifest.game.alias, &manifest.game.title, game_dir, &data_dir)?;
    fs::create_dir_all(data_dir.join("objects"))?;
    let _ = SnapsDb::open(&data_dir.join("snaps.db"))?;
    Ok(())
}

/// Create a desktop shortcut (Windows only; no-op on other platforms).
fn create_shortcut(manifest: &GimManifest, game_dir: &Path) -> GResult<()> {
    #[cfg(windows)]
    {
        // On Windows, create a .lnk shortcut using PowerShell.
        let desktop = dirs::desktop_dir().ok_or_else(|| GError::Other("cannot find desktop".into()))?;
        let lnk = desktop.join(format!("{}.lnk", manifest.game.title));
        let ps_script = format!(
            "$ws = New-Object -ComObject WScript.Shell; \
             $s = $ws.CreateShortcut('{}'); \
             $s.TargetPath = '{}'; \
             $s.WorkingDirectory = '{}'; \
             $s.Save()",
            lnk.display(),
            game_dir.join("game.exe").display(),
            game_dir.display()
        );
        let _ = std::process::Command::new("powershell")
            .args(["-Command", &ps_script])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = (manifest, game_dir);
    }
    Ok(())
}

/// Run interactive setup wizard.
fn run_interactive_setup(
    manifest: &GimManifest,
    output_dir: &mut PathBuf,
    snapshot: &Option<String>,
    track: &mut bool,
) -> GResult<()> {
    use std::io::{self, BufRead, Write};
    println!("=== gim install setup ===");
    println!("game: {}", manifest.game.title);
    println!();

    // Choose snapshot.
    if snapshot.is_none() && manifest.snapshots.len() > 1 {
        println!("available snapshots:");
        for (i, s) in manifest.snapshots.iter().enumerate() {
            println!("  [{}] {} ({})", i, s.id, s.message.as_deref().unwrap_or("(no message)"));
        }
        print!("choose snapshot [0-{}]: ", manifest.snapshots.len() - 1);
        io::stdout().flush()?;
        // Note: we can't easily modify the `snapshot` Option here since
        // it's borrowed. The interactive mode just shows info; the user
        // must pass --snapshot explicitly. This is a limitation.
    }

    print!("output directory [{}]: ", output_dir.display());
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();
    if !input.is_empty() {
        *output_dir = PathBuf::from(input);
    }

    print!("add to gim tracking? [Y/n]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    *track = !input.trim().eq_ignore_ascii_case("n");

    println!();
    Ok(())
}
