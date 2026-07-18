#!/usr/bin/env python3
"""test_compact.py

Python test script for the `gim compact` command.
Creates an isolated test game directory, runs a series of `gim` commands,
logs output, and prints a summary report.

Assumes `gim` executable is available in the system PATH (F:\\Gim).
"""

import os
import sys
import shutil
import subprocess
import json
import time
import tempfile
from pathlib import Path

# ---------------------------------------------------------------------------
# Helper utilities
# ---------------------------------------------------------------------------

def run_gim(args, env=None):
    """Run a `gim` command, capture output, and return a Result dict."""
    env = env or os.environ.copy()
    # Force isolated data directory for the test run
    env["GIM_DATA_DIR"] = str(DATA_DIR)
    cmd = ["gim"] + args
    proc = subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=env,
    )
    return {
        "cmd": " ".join(cmd),
        "returncode": proc.returncode,
        "output": proc.stdout,
    }

def write_repeat_file(path: Path, size: int, pattern: bytes = b"A"):
    """Write a file of *size* bytes by repeating *pattern* (default 'A')."""
    with path.open("wb") as f:
        chunk = pattern * 1024  # 1 KiB chunk
        written = 0
        while written < size:
            to_write = min(len(chunk), size - written)
            f.write(chunk[:to_write])
            written += to_write

# ---------------------------------------------------------------------------
# Test suite definition
# ---------------------------------------------------------------------------

def main():
    # Create a temporary root directory for all artefacts (data dir, game dir, logs)
    root = Path(tempfile.mkdtemp(prefix="gim_test_"))
    global DATA_DIR
    DATA_DIR = root / "gim_data"
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    game_dir = root / "test_game"
    game_dir.mkdir(parents=True, exist_ok=True)
    log_path = root / "compact_test.log"
    summary = []

    def log(msg: str):
        with log_path.open("a", encoding="utf-8") as f:
            f.write(msg + "\n")
        print(msg)

    def record(name: str, success: bool, details: str = ""):
        summary.append({"case": name, "success": success, "details": details})

    # -----------------------------------------------------------------------
    # 0. Prepare test files (<1 GB total)
    # -----------------------------------------------------------------------
    write_repeat_file(game_dir / "large.txt", 200 * 1024)          # 200 KB compressible
    write_repeat_file(game_dir / "tiny.txt", 100)                # 100 B tiny file
    # random binary (non‑compressible) ~200 KB
    (game_dir / "random.bin").write_bytes(os.urandom(200 * 1024))
    # subfolder with an extra file
    sub = game_dir / "subfolder"
    sub.mkdir()
    write_repeat_file(sub / "nested.txt", 50 * 1024)

    # -----------------------------------------------------------------------
    # 1. Register the game
    # -----------------------------------------------------------------------
    res = run_gim(["add", "testgame", str(game_dir)])
    log(res["output"])
    record("add_game", res["returncode"] == 0, res["output"])

    # -----------------------------------------------------------------------
    # 2. Dry‑run (no modifications expected)
    # -----------------------------------------------------------------------
    res = run_gim(["compact", "testgame", "--dry-run"])
    log(res["output"])
    record("dry_run", res["returncode"] == 0, res["output"])

    # -----------------------------------------------------------------------
    # 3. Default compression (LZX) with confirmation
    # -----------------------------------------------------------------------
    res = run_gim(["compact", "testgame", "--confirm"])
    log(res["output"])
    record("compress_default", res["returncode"] == 0, res["output"])

    # -----------------------------------------------------------------------
    # 4. Verify compression via a dry‑run (should show 0 candidates)
    # -----------------------------------------------------------------------
    res = run_gim(["compact", "testgame", "--dry-run"])
    log(res["output"])
    record("verify_compressed", res["returncode"] == 0, res["output"])

    # -----------------------------------------------------------------------
    # 5. Decompression
    # -----------------------------------------------------------------------
    res = run_gim(["compact", "testgame", "--decompress", "--confirm"])
    log(res["output"])
    record("decompress", res["returncode"] == 0, res["output"])

    # -----------------------------------------------------------------------
    # 6. NTFS attribute compression
    # -----------------------------------------------------------------------
    res = run_gim(["compact", "testgame", "-a", "ntfs", "--confirm"])
    log(res["output"])
    record("ntfs_compress", res["returncode"] == 0, res["output"])

    # -----------------------------------------------------------------------
    # 7. Force compression on a low‑savings game (random data)
    # -----------------------------------------------------------------------
    low_dir = root / "low_savings"
    low_dir.mkdir()
    (low_dir / "rand.bin").write_bytes(os.urandom(5 * 1024 * 1024))  # 5 MiB random
    run_gim(["add", "lowgame", str(low_dir)])
    # First run (should warn / skip)
    res_warn = run_gim(["compact", "lowgame", "--confirm"])
    record("low_warn", res_warn["returncode"] == 0, res_warn["output"])
    # Force
    res_force = run_gim(["compact", "lowgame", "--force", "--confirm"])
    record("low_force", res_force["returncode"] == 0, res_force["output"])

    # -----------------------------------------------------------------------
    # 8. Exclude pattern (*.txt)
    # -----------------------------------------------------------------------
    res = run_gim(["compact", "testgame", "--exclude", "*.txt", "--confirm"])
    log(res["output"])
    record("exclude_txt", res["returncode"] == 0, res["output"])

    # -----------------------------------------------------------------------
    # 9. Background execution + status check
    # -----------------------------------------------------------------------
    res_bg = run_gim(["compact", "testgame", "--background", "--confirm"])
    log(res_bg["output"])
    # Give the background task a moment then poll status
    time.sleep(2)
    res_status = run_gim(["compact", "testgame", "--status"])
    log(res_status["output"])
    record("background", res_bg["returncode"] == 0 and res_status["returncode"] == 0, res_status["output"])

    # -----------------------------------------------------------------------
    # 10. Final summary JSON saved alongside the log
    # -----------------------------------------------------------------------
    summary_path = root / "compact_test_summary.json"
    with summary_path.open("w", encoding="utf-8") as f:
        json.dump({"results": summary}, f, indent=2)
    log(f"Summary written to {summary_path}")
    log(f"Full log available at {log_path}")

if __name__ == "__main__":
    main()
