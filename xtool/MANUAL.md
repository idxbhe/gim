# XTool Manual

> Berdasarkan analisis source code (https://github.com/Razor12911/xtool),
> release binary xtool_0.7.9_hotfix.zip, dan klarifikasi komunitas.

## Apa itu xtool?

xtool adalah **precompressor**, BUKAN compressor.

xtool mendekompress stream yang sudah ter-compress di dalam file game
(zlib, zstd, lz4, oodle, dll) agar kompresi luar (Layer 2) bisa bekerja
lebih efektif. Output xtool bisa lebih besar dari input karena stream
di-inflate.

## Arsitektur 2-Layer di gim

```
File game → [Layer 1: xtool precomp] → [Layer 2: compression] → .bin
           decompress streams           compress output
```

### Layer 1: xtool precomp
- **Apa:** Decompress stream yang sudah ter-compress di game files
- **Codecs:** auto (zstd+zlib+lz4+kraken+mermaid+preflate)
- **Output:** Data dengan stream di-inflate (bisa lebih besar)

### Layer 2: compression (Rust native)
- **Apa:** Kompres output dari Layer 1
- **Algorithms:** zstd (1-22), lzma (1-9), lz4 (1-12)
- **Output:** File .bin terkompresi

## Parameter xtool precomp (yang dipakai gim)

| Parameter | Fungsi | Default |
|-----------|--------|---------|
| `-m#` | Codecs. "auto" = semua | (wajib) |
| `-c#` | Chunk size | 64mb |
| `-t#` | Threads | auto |
| `-d#` | Scan depth | 0 |
| `--dbase` | Stream database | on |
| `--dedup=#` | Deduplication | dedup.bin |
| `-lm` | Low memory mode | off |
| `-s` | Skip verification | off |

## Codecs tersedia

### Scanner codecs (auto-detect streams)
- `zlib` — Deflate/ZIP (1-9)
- `zstd` — ZStandard (1-22)
- `lz4` — LZ4 (1-12)
- `kraken` — Oodle Kraken (1-8)
- `mermaid` — Oodle Mermaid (1-8)
- `preflate` — Advanced deflate (no level)

### Non-scanner codecs (depth only)
- `lz4hc`, `lzo`, `selkie`, `hydra`, `reflate`, `grittibanzli`

### Bundled libraries (dari xtool release)
- fast-lzma2.dll, liblz4.dll, libzstd.dll, zlibwapi.dll
- oo2core_9_win64.dll, preflate_dll.dll
- hif2raw_dll.dll, raw2hif_dll.dll, lzo2.dll
- brunsli.dll, libFLAC_dynamic.dll, jojpeg_dll.dll, packjpg_dll.dll

## Commands

| Command | Status |
|---------|--------|
| `precomp` | ✅ Precompress |
| `decode` | ✅ Restore |
| `execute` | ✅ Parallel external execution |
| `archive` | ❌ Dihapus 0.7.9 |
| `patch` | ❌ Dihapus 0.7.9 |

## Decode

```
xtool decode -t<threads> [--dedup=dedup.bin] - -
```

Auto-detect format dari magic number.
