import os
import sys
import shutil
import subprocess
import tempfile
import time
import random
from pathlib import Path

# Setup executable name
GIM_EXE = "gim"  # Should be in PATH or environment, user mentioned it's in env PATH from F:\Gim

# Define folders in a local testing directory
TEST_DIR = Path("e:/Projects/gim/gim_testing")
GIM_DATA_DIR = TEST_DIR / "data_dir"
GAME_DIR = TEST_DIR / "game_dir"
LOW_SAVINGS_GAME_DIR = TEST_DIR / "low_savings_game_dir"
LOG_FILE = TEST_DIR / "compact_test.log"

# Define test summary registry
test_results = {}

def log(message, print_console=True):
    timestamp = time.strftime("%Y-%m-%d %H:%M:%S")
    formatted = f"[{timestamp}] {message}"
    if print_console:
        print(message)
    with open(LOG_FILE, "a", encoding="utf-8") as f:
        f.write(formatted + "\n")

def run_gim_cmd(args, check=True):
    env = os.environ.copy()
    env["GIM_DATA_DIR"] = str(GIM_DATA_DIR)
    env["GIM_NO_PROGRESS"] = "1"  # Disable progress bar for clean logs
    
    cmd = [GIM_EXE] + args
    log(f"Executing: {' '.join(cmd)}")
    
    res = subprocess.run(cmd, capture_output=True, text=True, env=env)
    
    log(f"Exit code: {res.returncode}", print_console=False)
    if res.stdout:
        log("--- STDOUT ---", print_console=False)
        log(res.stdout, print_console=False)
    if res.stderr:
        log("--- STDERR ---", print_console=False)
        log(res.stderr, print_console=False)
        
    if check and res.returncode != 0:
        raise RuntimeError(f"Command failed: {' '.join(cmd)}\nError: {res.stderr}")
    return res

def check_file_compressed_native(filepath):
    """
    Check if a file is compressed using native Windows compact.exe.
    Returns:
        bool: True if compressed (ratio > 1.0 or marked compressed), False otherwise.
    """
    if not sys.platform.startswith("win"):
        return False
    res = subprocess.run(["compact.exe", str(filepath)], capture_output=True, text=True)
    if res.returncode == 0:
        # Check if the output contains "1 are compressed" or "0 are not compressed"
        # Or look for 'l' / 'c' tag in the output line
        # e.g., "1 are compressed and 0 are not compressed."
        # Or "0 are compressed and 1 are not compressed."
        if "1 are compressed" in res.stdout:
            return True
        if "0 are compressed" in res.stdout:
            return False
    return False

def generate_test_files():
    log("Generating test folders and files...")
    if TEST_DIR.exists():
        shutil.rmtree(TEST_DIR)
    TEST_DIR.mkdir(parents=True, exist_ok=True)
    GAME_DIR.mkdir(parents=True, exist_ok=True)
    LOW_SAVINGS_GAME_DIR.mkdir(parents=True, exist_ok=True)
    
    # 1. Compressible files
    with open(GAME_DIR / "file_compressible_1.txt", "wb") as f:
        f.write(b"A" * 10 * 1024 * 1024)  # 10 MB
        
    with open(GAME_DIR / "file_compressible_2.txt", "wb") as f:
        f.write(b"B" * 5 * 1024 * 1024)   # 5 MB
        
    # Subdirectory compressible file
    subdir = GAME_DIR / "subdir"
    subdir.mkdir(parents=True, exist_ok=True)
    with open(subdir / "file_compressible_3.txt", "wb") as f:
        f.write(b"C" * 2 * 1024 * 1024)   # 2 MB
        
    # 2. Incompressible files (random binary data)
    random.seed(42)
    random_bytes = bytearray(random.getrandbits(8) for _ in range(5 * 1024 * 1024)) # 5 MB
    with open(GAME_DIR / "file_incompressible.bin", "wb") as f:
        f.write(random_bytes)
        
    # Same incompressible file for the low savings test game
    with open(LOW_SAVINGS_GAME_DIR / "file_incompressible.bin", "wb") as f:
        f.write(random_bytes)
        
    # 3. Tiny files (< 4 KB)
    with open(GAME_DIR / "file_tiny.txt", "wb") as f:
        f.write(b"Tiny file contents that should be skipped by compaction." * 10) # ~550 bytes
        
    with open(subdir / "file_tiny2.txt", "wb") as f:
        f.write(b"Another tiny file." * 5) # ~90 bytes
        
    log("Test files successfully generated.")

def test_add_game():
    log("\n=== Test Case 1: Registering Test Games ===")
    try:
        run_gim_cmd(["add", "testgame", str(GAME_DIR), "--title", "Main Test Game"])
        run_gim_cmd(["add", "testgame_low", str(LOW_SAVINGS_GAME_DIR), "--title", "Low Savings Test Game"])
        
        # Verify listing
        res = run_gim_cmd(["list"])
        if "testgame" in res.stdout and "testgame_low" in res.stdout:
            test_results["Add Games"] = "PASS"
            log("SUCCESS: Games registered successfully.")
        else:
            test_results["Add Games"] = "FAIL (not in list)"
            log("ERROR: Games not found in list.")
    except Exception as e:
        test_results["Add Games"] = f"FAIL ({str(e)})"
        log(f"ERROR: Add Games failed: {e}")

def test_dry_run():
    log("\n=== Test Case 2: Dry Run Compaction ===")
    try:
        res = run_gim_cmd(["compact", "testgame", "--dry-run"])
        # Check if dry run prints estimates but makes no changes
        if "dry run — no changes made" in res.stdout:
            # Verify file is not compressed
            is_comp = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
            if not is_comp:
                test_results["Dry Run"] = "PASS"
                log("SUCCESS: Dry run completed and made no changes as expected.")
            else:
                test_results["Dry Run"] = "FAIL (file was compressed)"
                log("ERROR: File was compressed during dry run.")
        else:
            test_results["Dry Run"] = "FAIL (no dry run message)"
            log("ERROR: Dry run did not print expected notice.")
    except Exception as e:
        test_results["Dry Run"] = f"FAIL ({str(e)})"
        log(f"ERROR: Dry Run failed: {e}")

def test_foreground_wof_lzx():
    log("\n=== Test Case 3: Foreground WOF LZX Compaction (Default) ===")
    try:
        res = run_gim_cmd(["compact", "testgame", "--confirm"])
        if "compacted" in res.stdout or "✓" in res.stdout:
            # Verify compression status natively
            is_comp1 = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
            is_comp3 = check_file_compressed_native(GAME_DIR / "subdir" / "file_compressible_3.txt")
            is_comp_tiny = check_file_compressed_native(GAME_DIR / "file_tiny.txt")
            
            if is_comp1 and is_comp3 and not is_comp_tiny:
                test_results["WOF LZX Compaction"] = "PASS"
                log("SUCCESS: Compressible files compacted and tiny files skipped.")
            else:
                test_results["WOF LZX Compaction"] = f"FAIL (comp1: {is_comp1}, comp3: {is_comp3}, tiny: {is_comp_tiny})"
                log(f"ERROR: File compression attributes mismatch: large={is_comp1}, tiny={is_comp_tiny}")
        else:
            test_results["WOF LZX Compaction"] = "FAIL (command output mismatch)"
            log("ERROR: Compaction command returned success but output doesn't match.")
    except Exception as e:
        test_results["WOF LZX Compaction"] = f"FAIL ({str(e)})"
        log(f"ERROR: WOF LZX Compaction failed: {e}")

def test_dry_run_after_compaction():
    log("\n=== Test Case 4: Dry Run After Compaction (No Candidates) ===")
    try:
        res = run_gim_cmd(["compact", "testgame", "--dry-run"])
        # Should show 0 candidates and skip already compressed
        if "candidates: 0" in res.stdout or "already compressed" in res.stdout:
            test_results["Dry Run After Compaction"] = "PASS"
            log("SUCCESS: Dry run recognized that files were already compressed.")
        else:
            test_results["Dry Run After Compaction"] = "FAIL (showed candidates)"
            log("ERROR: Dry run after compaction showed candidates to compress.")
    except Exception as e:
        test_results["Dry Run After Compaction"] = f"FAIL ({str(e)})"
        log(f"ERROR: Dry Run After Compaction failed: {e}")

def test_decompression():
    log("\n=== Test Case 5: WOF Decompression ===")
    try:
        res = run_gim_cmd(["compact", "testgame", "--decompress", "--confirm"])
        if "decompressed" in res.stdout or "✓" in res.stdout:
            is_comp1 = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
            if not is_comp1:
                test_results["WOF Decompression"] = "PASS"
                log("SUCCESS: WOF decompressed file restored to normal state.")
            else:
                test_results["WOF Decompression"] = "FAIL (file remains compressed)"
                log("ERROR: File is still compressed after decompression.")
        else:
            test_results["WOF Decompression"] = "FAIL (command output mismatch)"
            log("ERROR: Decompression command returned success but output doesn't match.")
    except Exception as e:
        test_results["WOF Decompression"] = f"FAIL ({str(e)})"
        log(f"ERROR: WOF Decompression failed: {e}")

def test_ntfs_compression():
    log("\n=== Test Case 6: NTFS Compression & Decompression ===")
    try:
        # Compress using NTFS
        res_comp = run_gim_cmd(["compact", "testgame", "-a", "ntfs", "--confirm"])
        is_comp1 = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
        
        # Decompress NTFS
        res_decomp = run_gim_cmd(["compact", "testgame", "--decompress", "--confirm"])
        is_decomp1 = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
        
        if is_comp1 and not is_decomp1:
            test_results["NTFS Comp & Decomp"] = "PASS"
            log("SUCCESS: NTFS compaction and decompression verified successfully.")
        else:
            test_results["NTFS Comp & Decomp"] = f"FAIL (comp: {is_comp1}, decomp: {is_decomp1})"
            log(f"ERROR: NTFS verify mismatch: compressed={is_comp1}, decompressed={not is_decomp1}")
    except Exception as e:
        test_results["NTFS Comp & Decomp"] = f"FAIL ({str(e)})"
        log(f"ERROR: NTFS Comp & Decomp failed: {e}")

def test_xpress_algorithms():
    log("\n=== Test Case 7: Alternate WOF Algorithms (XPRESS8K / XPRESS16K) ===")
    try:
        # Xpress8K
        run_gim_cmd(["compact", "testgame", "-a", "xpress8k", "--confirm"])
        is_comp_x8 = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
        run_gim_cmd(["compact", "testgame", "--decompress", "--confirm"])
        
        # Xpress16K
        run_gim_cmd(["compact", "testgame", "-a", "xpress16k", "--confirm"])
        is_comp_x16 = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
        run_gim_cmd(["compact", "testgame", "--decompress", "--confirm"])
        
        if is_comp_x8 and is_comp_x16:
            test_results["XPRESS Algorithms"] = "PASS"
            log("SUCCESS: XPRESS8K and XPRESS16K compaction runs completed successfully.")
        else:
            test_results["XPRESS Algorithms"] = f"FAIL (x8: {is_comp_x8}, x16: {is_comp_x16})"
            log("ERROR: Alternate WOF algorithm check failed.")
    except Exception as e:
        test_results["XPRESS Algorithms"] = f"FAIL ({str(e)})"
        log(f"ERROR: Xpress algorithms failed: {e}")

def test_low_savings_and_force():
    log("\n=== Test Case 8: Low Savings Verification & Force Option ===")
    try:
        # Compressing low savings game should warn and exit without compressing
        res = run_gim_cmd(["compact", "testgame_low", "--confirm"])
        is_comp = check_file_compressed_native(LOW_SAVINGS_GAME_DIR / "file_incompressible.bin")
        
        if "low" in res.stdout and not is_comp:
            # Force compression
            run_gim_cmd(["compact", "testgame_low", "--confirm", "--force"])
            is_comp_force = check_file_compressed_native(LOW_SAVINGS_GAME_DIR / "file_incompressible.bin")
            
            if is_comp_force:
                test_results["Low Savings & Force"] = "PASS"
                log("SUCCESS: Low savings safety check and --force override behave correctly.")
            else:
                test_results["Low Savings & Force"] = "FAIL (force did not compress)"
                log("ERROR: Force option was passed but file was not compressed.")
        else:
            test_results["Low Savings & Force"] = f"FAIL (warn: {'low' in res.stdout}, comp: {is_comp})"
            log("ERROR: Low savings safety check failed to block compression.")
    except Exception as e:
        test_results["Low Savings & Force"] = f"FAIL ({str(e)})"
        log(f"ERROR: Low Savings & Force failed: {e}")

def test_exclude_pattern():
    log("\n=== Test Case 9: Exclude Compaction Patterns ===")
    try:
        # Restore first
        run_gim_cmd(["compact", "testgame", "--decompress", "--confirm"])
        
        # Compact excluding *.txt files
        run_gim_cmd(["compact", "testgame", "-a", "lzx", "--exclude", "*.txt", "--confirm"])
        
        # check that .txt files are not compressed but the binary file is (if it is a candidate?)
        is_comp_txt = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
        is_comp_bin = check_file_compressed_native(GAME_DIR / "file_incompressible.bin")
        
        if not is_comp_txt and is_comp_bin:
            test_results["Exclude Patterns"] = "PASS"
            log("SUCCESS: Exclude patterns successfully respected.")
        else:
            test_results["Exclude Patterns"] = f"FAIL (txt_compressed: {is_comp_txt}, bin_compressed: {is_comp_bin})"
            log(f"ERROR: Exclude verification failed. txt_comp={is_comp_txt}, bin_comp={is_comp_bin}")
    except Exception as e:
        test_results["Exclude Patterns"] = f"FAIL ({str(e)})"
        log(f"ERROR: Exclude Patterns failed: {e}")

def test_target_folder_and_snapshot():
    log("\n=== Test Case 10: Snapshot Data Compaction via Target Option ===")
    try:
        # Decompress main game first
        run_gim_cmd(["compact", "testgame", "--decompress", "--confirm"])
        
        # Create a snapshot to populate the data directory (objects/ CAS)
        run_gim_cmd(["snap", "testgame", "-m", "Test snapshot to generate CAS objects"])
        
        # Verify objects directory exists and has files
        objects_dir = GIM_DATA_DIR / "testgame" / "objects"
        if not objects_dir.exists() or len(list(objects_dir.glob("**/*"))) == 0:
            raise RuntimeError("Objects folder was not created or empty after snap.")
            
        # Compact snapshot data directory only
        res = run_gim_cmd(["compact", "testgame", "--target", "data", "--confirm"])
        
        # Check if the files in the objects directory are WOF compressed
        obj_files = [p for p in objects_dir.glob("**/*") if p.is_file() and p.stat().st_size >= 4096]
        if not obj_files:
            test_results["Target Snapshot Data"] = "FAIL (no large object files)"
            log("ERROR: No files in objects folder qualified for WOF (>4KB) to verify.")
            return
            
        compressed_objs = [check_file_compressed_native(f) for f in obj_files]
        is_game_compressed = check_file_compressed_native(GAME_DIR / "file_compressible_1.txt")
        
        if all(compressed_objs) and not is_game_compressed:
            test_results["Target Snapshot Data"] = "PASS"
            log("SUCCESS: Snapshot data (objects) compacted successfully while game folder was untouched.")
        else:
            test_results["Target Snapshot Data"] = f"FAIL (objs: {compressed_objs}, game_comp: {is_game_compressed})"
            log(f"ERROR: Target verification mismatch. Objs compressed: {compressed_objs}, Game folder compressed: {is_game_compressed}")
    except Exception as e:
        test_results["Target Snapshot Data"] = f"FAIL ({str(e)})"
        log(f"ERROR: Target Snapshot Data failed: {e}")

def test_background_and_status():
    log("\n=== Test Case 11: Background Compaction & Status Checking ===")
    try:
        # Decompress everything
        run_gim_cmd(["compact", "testgame", "--decompress", "--confirm"])
        
        # Start compaction in background
        res_bg = run_gim_cmd(["compact", "testgame", "--background", "--confirm"])
        if "background compaction started" in res_bg.stdout or "starting background compaction..." in res_bg.stdout:
            # Query status
            res_status = run_gim_cmd(["compact", "testgame", "--status"])
            if "compaction testgame" in res_status.stdout:
                test_results["Background & Status"] = "PASS"
                log("SUCCESS: Background compaction started and status report retrieved.")
            else:
                test_results["Background & Status"] = "FAIL (status command mismatch)"
                log("ERROR: Status query output format mismatch.")
        else:
            test_results["Background & Status"] = "FAIL (background command mismatch)"
            log("ERROR: Background compaction command failed to report start.")
    except Exception as e:
        test_results["Background & Status"] = f"FAIL ({str(e)})"
        log(f"ERROR: Background & Status failed: {e}")

def print_final_summary():
    log("\n" + "=" * 60)
    log("                       TEST CASES SUMMARY")
    log("=" * 60)
    
    passes = 0
    fails = 0
    
    for case, result in test_results.items():
        if "PASS" in result:
            passes += 1
            status_str = "PASS"
        else:
            fails += 1
            status_str = result
            
        log(f"  {case:<35} : {status_str}")
        
    log("=" * 60)
    log(f"  TOTAL COMPLETED: {len(test_results)} | PASSED: {passes} | FAILED: {fails}")
    log("=" * 60)
    log(f"All logs saved to: {LOG_FILE}")
    
    if fails > 0:
        sys.exit(1)
    else:
        sys.exit(0)

def main():
    log("============================================================")
    log("              GIM COMPACT COMMAND TEST SUITE")
    log("============================================================")
    
    try:
        generate_test_files()
        test_add_game()
        test_dry_run()
        test_foreground_wof_lzx()
        test_dry_run_after_compaction()
        test_decompression()
        test_ntfs_compression()
        test_xpress_algorithms()
        test_low_savings_and_force()
        test_exclude_pattern()
        test_target_folder_and_snapshot()
        test_background_and_status()
    finally:
        print_final_summary()

if __name__ == "__main__":
    main()
