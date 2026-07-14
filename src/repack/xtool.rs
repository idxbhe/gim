//! xtool wrapper — invoke xtool.exe for precompression/decoding.
//!
//! xtool is a precompressor that finds and recompresses already-compressed
//! streams (zlib, lz4, zstd, oodle, etc.) so that the outer compression
//! (e.g. lzma2) can achieve much better ratios on game files.
//!
//! xtool reads from stdin and writes to stdout when given `- -` as
//! input/output. We pipe data through it.

use crate::error::{GError, GResult};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Xtool {
    /// Path to xtool.exe (or xtool on Unix).
    exe_path: PathBuf,
}

impl Xtool {
    /// Locate xtool executable. Checks `[bin_dir]/xtool/xtool.exe`
    /// (Windows) or `[bin_dir]/xtool/xtool` (Unix).
    pub fn find(bin_dir: &Path) -> GResult<Self> {
        let xtool_dir = bin_dir.join("xtool");
        let exe_name = if cfg!(windows) { "xtool.exe" } else { "xtool" };
        let exe_path = xtool_dir.join(exe_name);
        if !exe_path.exists() {
            return Err(GError::Other(format!(
                "xtool not found at {}. Download xtool and place it in the xtool/ subdirectory next to gim. See: https://github.com/Razor12911/xtool",
                exe_path.display()
            )));
        }
        Ok(Self { exe_path })
    }

    /// Precompress (encode) data: read from `input`, write precompressed
    /// data to `output`. Returns bytes written.
    ///
    /// `args` is the full xtool argument list (e.g. ["precomp", "-mzlib:l5", "-c64mb", "-t7"]).
    pub fn encode(&self, input: &Path, output: &Path, args: &[String]) -> GResult<u64> {
        let mut child = Command::new(&self.exe_path)
            .args(args)
            .arg("-")      // stdin
            .arg("-")      // stdout
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GError::Other(format!("failed to spawn xtool: {e}")))?;

        // Pipe input file to xtool stdin.
        let mut input_file = std::fs::File::open(input)?;
        let child_stdin = child.stdin.take().ok_or_else(|| GError::Other("xtool stdin not captured".into()))?;

        // Spawn a thread to write input to xtool.
        let input_thread = std::thread::spawn(move || -> GResult<()> {
            let mut stdin = child_stdin;
            let mut buf = vec![0u8; 1024 * 1024];
            loop {
                let n = input_file.read(&mut buf)?;
                if n == 0 { break; }
                stdin.write_all(&buf[..n])?;
            }
            Ok(())
        });

        // Read xtool stdout to output file.
        let mut output_file = std::fs::File::create(output)?;
        let child_stdout = child.stdout.take().ok_or_else(|| GError::Other("xtool stdout not captured".into()))?;
        let mut reader = std::io::BufReader::new(child_stdout);
        let bytes_written = std::io::copy(&mut reader, &mut output_file)?;

        // Wait for input thread to finish.
        input_thread.join().map_err(|_| GError::Other("xtool input thread panicked".into()))??;

        // Wait for xtool to exit.
        let status = child.wait().map_err(|e| GError::Other(format!("xtool wait failed: {e}")))?;
        if !status.success() {
            let mut stderr = String::new();
            if let Some(mut stderr_handle) = child.stderr.take() {
                let _ = stderr_handle.read_to_string(&mut stderr);
            }
            return Err(GError::Other(format!(
                "xtool encode failed (exit {:?}): {}",
                status.code(),
                stderr.trim()
            )));
        }

        Ok(bytes_written)
    }

    /// Decode data: read precompressed `input`, write decoded to `output`.
    pub fn decode(&self, input: &Path, output: &Path, args: &[String]) -> GResult<u64> {
        let mut child = Command::new(&self.exe_path)
            .args(args)
            .arg("-")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| GError::Other(format!("failed to spawn xtool: {e}")))?;

        let mut input_file = std::fs::File::open(input)?;
        let child_stdin = child.stdin.take().ok_or_else(|| GError::Other("xtool stdin not captured".into()))?;

        let input_thread = std::thread::spawn(move || -> GResult<()> {
            let mut stdin = child_stdin;
            let mut buf = vec![0u8; 1024 * 1024];
            loop {
                let n = input_file.read(&mut buf)?;
                if n == 0 { break; }
                stdin.write_all(&buf[..n])?;
            }
            Ok(())
        });

        let mut output_file = std::fs::File::create(output)?;
        let child_stdout = child.stdout.take().ok_or_else(|| GError::Other("xtool stdout not captured".into()))?;
        let mut reader = std::io::BufReader::new(child_stdout);
        let bytes_written = std::io::copy(&mut reader, &mut output_file)?;

        input_thread.join().map_err(|_| GError::Other("xtool input thread panicked".into()))??;

        let status = child.wait().map_err(|e| GError::Other(format!("xtool wait failed: {e}")))?;
        if !status.success() {
            return Err(GError::Other(format!("xtool decode failed (exit {:?})", status.code())));
        }

        Ok(bytes_written)
    }

    /// Check if xtool is available.
    pub fn is_available(bin_dir: &Path) -> bool {
        let xtool_dir = bin_dir.join("xtool");
        let exe_name = if cfg!(windows) { "xtool.exe" } else { "xtool" };
        xtool_dir.join(exe_name).exists()
    }
}
