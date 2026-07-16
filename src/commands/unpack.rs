//! `gim unpack` / `gim install` — unpack a .gim archive.
//!
//! Pipeline (reverse of repack):
//! 1. Read .gim manifest
//! 2. Layer 2 decompress: objects.bin → temp (zstd/lzma/lz4 decompress)
//! 3. Layer 1 decode: temp → temp2 (xtool decode precompressed data)
//! 4. Restore files from decoded object data by offset+size
//! 5. Cleanup temps
//! 6. (Optional) register game + create shortcut

use crate::config::{env_data_dir_override, Paths};
use crate::db::{GamesDb, SnapsDb};
use crate::error::{GError, GResult};
use crate::output::{Colorizer, ProgressReporter};
use crate::repack::{decompress_file, CompressAlgorithm, GimManifest, Xtool};
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

    if interactive && !dry_run {
        run_interactive_setup(&manifest, &mut output_dir.clone(), &snapshot, &mut track.clone())?;
    }

    let mut paths = Paths::from_env()?;
    if let Some(o) = env_data_dir_override() { paths = paths.with_data_dir(o); }
    let xtool = Xtool::find(&paths.binary_dir)?;

    // Parse compression algorithm from manifest.
    let compress_algo = CompressAlgorithm::parse(&manifest.compression.algorithm)?;

    // Build xtool decode args.
    let threads_str = match threads {
        Some(t) => t.to_string(),
        None => "0".to_string(),
    };
    let mut decode_args = vec![
        "decode".to_string(),
        format!("-t{}", threads_str),
    ];
    let manifest_dir = gim_file.parent().ok_or_else(|| GError::Other("cannot determine manifest directory".into()))?;
    if manifest.compression.dedup {
        let dedup_file = manifest_dir.join("dedup.bin");
        if dedup_file.exists() {
            decode_args.push("--dedup=dedup.bin".to_string());
        }
    }

    if dry_run {
        println!("dry run: would unpack \"{}\" from {}", c.green(&manifest.game.title), gim_file.display());
        println!("  snapshot: {} ({}, {} files)", snap.id, snap.message.as_deref().unwrap_or("(no message)"), snap.file_count);
        println!("  output: {}", output_dir.display());
        println!("  objects: {} unique", manifest.objects.entries.len());
        println!("  compression: {}@{}", manifest.compression.algorithm, manifest.compression.level);
        println!("  precomp: {}", manifest.compression.precomp_codecs);
        if track { println!("  tracking: will add to gim registry"); }
        if is_install { println!("  install: will register game + create shortcut"); }
        return Ok(());
    }

    fs::create_dir_all(&output_dir)?;

    // ── Phase 1: Layer 2 decompress objects.bin ────────────────────
    progress.phase_start("decompressing", 0);
    let objects_bin = manifest_dir.join(&manifest.objects.file);
    if !objects_bin.exists() {
        progress.phase_cancel();
        return Err(GError::Other(format!("objects file not found: {}", objects_bin.display())));
    }
    let objects_precomp = output_dir.join(".objects.precomp.tmp");
    decompress_file(&objects_bin, &objects_precomp, compress_algo)?;
    progress.phase_cancel();

    // ── Phase 2: Layer 1 xtool decode ──────────────────────────────
    progress.phase_start("decoding (xtool)", 0);
    let objects_decoded = output_dir.join(".objects.decoded.tmp");
    xtool.decode(&objects_precomp, &objects_decoded, &decode_args)?;
    let _ = fs::remove_file(&objects_precomp);
    progress.phase_cancel();

    // ── Phase 3: Restore files from objects ────────────────────────
    progress.phase_start("unpacking files", snap.files.len() as usize);

    let obj_map: std::collections::HashMap<String, (u64, u64)> = manifest.objects.entries.iter()
        .map(|e| (e.hash.clone(), (e.offset, e.orig_size)))
        .collect();

    let objects_file = fs::File::open(&objects_decoded)?;

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

        use std::io::{Seek, SeekFrom};
        let mut obj_file = objects_file.try_clone().map_err(|e| format!("clone: {e}"))?;
        obj_file.seek(SeekFrom::Start(offset)).map_err(|e| format!("seek: {e}"))?;
        let mut data = vec![0u8; size as usize];
        obj_file.read_exact(&mut data).map_err(|e| format!("read: {e}"))?;

        let mut dst = fs::File::create(&abs).map_err(|e| format!("create {}: {e}", f.path))?;
        dst.write_all(&data).map_err(|e| format!("write {}: {e}", f.path))?;

        if f.mtime > 0 {
            let _ = filetime::set_file_mtime(&abs, filetime::FileTime::from_unix_time(f.mtime, 0));
        }

        progress.phase_tick();
        Ok(())
    }).collect();

    progress.phase_cancel();

    let _ = fs::remove_file(&objects_decoded);

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

    if track {
        progress.phase_start("registering game", 0);
        register_game(&manifest, &output_dir, &paths)?;
        progress.phase_cancel();
        println!("  registered as \"{}\"", c.green(&manifest.game.alias));
    }

    if is_install {
        progress.phase_start("creating shortcut", 0);
        create_shortcut(&manifest, &output_dir)?;
        progress.phase_cancel();
        println!("  shortcut created");
    }

    Ok(())
}

fn register_game(manifest: &GimManifest, game_dir: &Path, paths: &Paths) -> GResult<()> {
    let gdb = GamesDb::open(&paths.games_db)?;
    if gdb.get(&manifest.game.alias)?.is_some() { return Ok(()); }
    let data_dir = paths.game_data_dir(&manifest.game.alias);
    gdb.add(&manifest.game.alias, &manifest.game.title, game_dir, &data_dir)?;
    fs::create_dir_all(data_dir.join("objects"))?;
    let _ = SnapsDb::open(&data_dir.join("snaps.db"))?;
    Ok(())
}

fn create_shortcut(manifest: &GimManifest, game_dir: &Path) -> GResult<()> {
    #[cfg(windows)]
    {
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
    { let _ = (manifest, game_dir); }
    Ok(())
}

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

    if snapshot.is_none() && manifest.snapshots.len() > 1 {
        println!("available snapshots:");
        for (i, s) in manifest.snapshots.iter().enumerate() {
            println!("  [{}] {} ({})", i, s.id, s.message.as_deref().unwrap_or("(no message)"));
        }
        print!("choose snapshot [0-{}]: ", manifest.snapshots.len() - 1);
        io::stdout().flush()?;
    }

    print!("output directory [{}]: ", output_dir.display());
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();
    if !input.is_empty() { *output_dir = PathBuf::from(input); }

    print!("add to gim tracking? [Y/n]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    *track = !input.trim().eq_ignore_ascii_case("n");

    println!();
    Ok(())
}
