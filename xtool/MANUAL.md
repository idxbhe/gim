# XTool Manual — Berdasarkan Analisis Source Code

> Dokumentasi ini dibuat dari analisis langsung source code xtool
> (https://github.com/Razor12911/xtool) dan dokumentasi resmi 0.7.9.
> Beberapa parameter tidak terdokumentasi resmi tapi ditemukan di source.

## Commands

| Command | Status | Fungsi |
|---------|--------|--------|
| `precomp` | ✅ Aktif | Precompress data + optional output compression |
| `decode` | ✅ Aktif | Auto-detect format & restore data |
| `execute` | ✅ Aktif | Parallel external program execution pada data chunks |
| `extract` | ✅ Aktif | Extract sectors/streams dari files |
| `find` | ✅ Aktif | Cari data dalam files |
| `erase` | ✅ Aktif | Hapus data dari files |
| `replace` | ✅ Aktif | Ganti data dalam files |
| `generate` | ✅ Aktif | Generate database untuk external codecs |
| `archive` | ❌ Tidak diimplementasi | Listed di source tapi tidak ada handler |
| `patch` | ❌ Tidak diimplementasi | Listed di source tapi tidak ada handler |

## `precomp` — Precompressor

### Cara kerja

xtool precomp bekerja dalam 2 layer yang bisa aktif bersamaan:

```
Input data
    │
    ▼
┌─────────────────────────────────────────────┐
│ Layer 1: PRECOMPRESSION                     │
│ Scan data untuk stream yang sudah ter-      │
│ compress (zlib, zstd, lz4, oodle, dll).    │
│ Inflate (decompress) stream tersebut.       │
│ Output: data dengan stream di-inflate.      │
│                                             │
│ Parameter: -m, -c, -d, --dbase, --dedup,   │
│            --mem, --diff, -lm, -s, -f, dll  │
└─────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────┐
│ Layer 2: OUTPUT COMPRESSION                 │
│ Kompres output dari Layer 1.                │
│                                             │
│ Opsi A: Built-in fast LZMA2 (-l parameter) │
│   Compression level 1-10 dengan dictionary  │
│   size yang bisa diatur.                    │
│                                             │
│ Opsi B: External compressor (-e parameter) │
│   Pipe output ke program external seperti   │
│   7z, FreeArc, dll.                         │
│                                             │
│ Jika Layer 2 tidak aktif (-l0 dan tidak     │
│ ada -e), output adalah precompressed data   │
│ tanpa kompresi tambahan (bisa lebih besar   │
│ dari input karena stream di-inflate).       │
└─────────────────────────────────────────────┘
    │
    ▼
Output file (.bin)
```

### Input/Output

```
xtool precomp [parameters] input output
xtool precomp [parameters] - -        (stdin/stdout)
```

- Input: file path, `-` (stdin), atau URL (download langsung)
- Output: file path atau `-` (stdout)

### Parameter Lengkap

#### Precompression (Layer 1)

| Parameter | Fungsi | Default | Catatan |
|-----------|--------|---------|---------|
| `-m#` | Codecs untuk precompression | (wajib) | Multiple dipisah `+`, contoh: `-mzstd+preflate` |
| `-c#` | Chunk size untuk scanning | `16mb` | Range: 4mb-2gb. Lebih besar = deteksi stream lebih baik, memory lebih besar |
| `-t#` | Jumlah thread | `50p` | Angka eksak atau persentase dengan `p`. Contoh: `4`, `75p`, `100p-1` |
| `-T#` | Prioritas thread | `normal` | idle, normal, high, timecritical |
| `-d#` | Scan depth | `0` | 0=none, 1-10=berapa level stream-in-stream dicari |
| `-lm` | Low memory mode | off | Satu chunk scanned pada satu waktu (lebih lambat, hemat memory) |
| `-s` | Skip stream verification | off | Lewati verifikasi CRC (lebih cepat, berisiko) |
| `-f` | Full scan | off | Scan lebih thorough |
| `-o` | Optimize decode | off | Optimalkan output untuk decode lebih cepat |
| `-v` | Verbose output | off | Tampilkan info detail saat proses |
| `-x#` | Extract streams ke direktori | off | Untuk debugging/analisis |
| `-r#` | Recompress streams | off | Recompress stream yang terdeteksi dengan codec lain |
| `-a#` | Reassign streams | off | Assign stream yang terdeteksi ke codec lain |
| `-p#` | Prefetch cache | `0mb` | Pre-load data ke memory untuk I/O lebih cepat |

#### Stream Database & Deduplication

| Parameter | Fungsi | Default | Catatan |
|-----------|--------|---------|---------|
| `--dbase` | Stream database | off | Cache stream yang sudah diproses untuk speed boost |
| `--dedup=#` | Stream deduplication | off | Filename untuk dedup database. Contoh: `--dedup=dedup.bin` |
| `--mem=#` | Dedup memory limit | `75p` | Memory untuk dedup. Contoh: `4096mb`, `75p`, `75p-600mb` |
| `--diff=#` | Delta threshold | `5p` | Threshold untuk stream yang tidak bisa di-restore sempurna |

**Catatan tentang `--` vs `-d` parameters:**
Dari source code, parameter `--dbase`, `--dedup`, `--mem`, `--diff` adalah
long-form aliases. Source code juga mendukung bentuk pendek via `-d`:
- `-dd` = deduplication
- `-df5p` = diff threshold
- `-dm75p` = dedup memory
- `-db512mb` = decode block size

#### Output Compression (Layer 2)

| Parameter | Fungsi | Default | Catatan |
|-----------|--------|---------|---------|
| `-l#` | LZMA2 compression level | `0` (off) | 1-10 = aktif. Butuh FLZMA2DLL. |
| `-l#:d#` | LZMA2 dengan dictionary | - | Contoh: `-l5:d128mb` |
| `-l#x` | LZMA2 high compression | - | Contoh: `-l5x` |
| `-e#` | External compressor | off | Pipe output ke program external |

**LZMA2 built-in (`-l` parameter):**
- Level 0 = tidak ada kompresi (precompression only)
- Level 1-10 = kompresi LZMA2 dengan level tersebut
- Butuh library FLZMA2DLL (fast LZMA2)
- Dictionary size default tergantung level
- High compression mode (`x` suffix) = rasio lebih baik, encode lebih lambat

**External compressor (`-e` parameter):**
- xtool pipe output precompression ke program external
- Program harus support stdin/stdout
- Override built-in LZMA2 jika keduanya diset

#### Custom Libraries

| Parameter | Fungsi | Default |
|-----------|--------|---------|
| `-lz4#` | Custom LZ4 library filename | `liblz4.dll` |
| `-zstd#` | Custom ZSTD library filename | `libzstd.dll` |
| `-oodle#` | Custom Oodle library filename | auto-detect |

### Codecs Tersedia

#### Internal Codecs (scanner + processor)

| Codec | Scan Streams? | Level Range | Butuh Library | Keterangan |
|-------|---------------|-------------|---------------|------------|
| `zlib` | ✅ Ya | 1-9 | zlib1.dll / zlibwapi.dll | Deflate/ZIP streams |
| `zstd` | ✅ Ya | 1-22 | libzstd.dll | ZStandard streams (modern games) |
| `lz4` | ❌ Tidak* | 1-12 | liblz4.dll | LZ4 streams |
| `lz4hc` | ❌ Tidak* | 1-12 | liblz4.dll | LZ4 High Compression |
| `lzo` | ❌ Tidak* | N/A | lzo2.dll | LZO streams (legacy) |
| `kraken` | ✅ Ya | 1-8 | oo2core_*.dll | Oodle Kraken (Unreal Engine) |
| `mermaid` | ✅ Ya | 1-8 | oo2core_*.dll | Oodle Mermaid |
| `selkie` | ✅ Ya | 1-8 | oo2core_*.dll | Oodle Selkie |
| `hydra` | ✅ Ya | 1-8 | oo2core_*.dll | Oodle Hydra (kraken+mermaid) |

\* Codec tanpa scanner hanya dipakai oleh external codecs atau saat depth > 0.

#### Deflate Processors (dipakai bersama zlib)

| Codec | Level | Butuh Library | Keterangan |
|-------|-------|---------------|------------|
| `preflate` | N/A | preflate_dll.dll | Advanced deflate scanner, catches what zlib misses |
| `reflate` | N/A | hif2raw_dll.dll, raw2hif_dll.dll | Re-compress deflate streams (slow decode) |
| `grittibanzli` | N/A | grittibanzli_dll.dll | Alternative deflate processor (slow) |

**Penggunaan kombinasi:** `-mzlib+preflate` (zlib scan + preflate backup)

#### Additional Codecs (dari source code, kurang terdokumentasi)

| Codec | Library | Keterangan |
|-------|---------|------------|
| `brunsli` | BrunsliDLL | JPEG recompression |
| `flac` | FLACDLL | FLAC audio recompression |
| `jojpeg` | JoJpegDLL | JPEG recompression |
| `packjpg` | PackJPGDLL | JPEG recompression |
| `flzma2` | FLZMA2DLL | Fast LZMA2 (built-in compression via -l) |

#### External Codecs

External codecs dibuat user dalam 3 bentuk:
1. **Configuration** (`.ini`) — INI file dengan stream signature info
2. **Database** (`.xtl`) — Generated database dengan stream info
3. **Library** (`.dll`) — Custom DLL dengan 7 exported functions

## `decode` — Restore Data

```
xtool decode [parameters] input [dedup_data] output
```

Auto-detect format (precomp/extract/execute) dari magic number di input file.

| Parameter | Fungsi | Default |
|-----------|--------|---------|
| `-t#` | Threads | `Threads/2` |
| `--dedup=#` | Dedup file untuk restore | (wajib jika dipakai saat encode) |
| `--mem=#` | Dedup memory | `75p` |

## `execute` — Parallel Program Execution

```
xtool execute [parameters] input output [exec_syntax]
```

Memecah data menjadi chunks, jalankan external program pada setiap chunk
secara paralel via stdin/stdout.

| Parameter | Fungsi | Default |
|-----------|--------|---------|
| `-c#` | Chunk size | `64mb` |
| `-t#` | Threads | `50p` |

**Exec syntax:**
- `{stdin}` / `<stdin>` / `[stdin]` — pipe input via stdin
- `{stdout}` / `<stdout>` / `[stdout]` — pipe output via stdout
- `{filein}` / `<filein>` — gunakan file untuk input
- `{fileout}` / `<fileout>` — gunakan file untuk output

Contoh:
```
xtool execute -t4 input.bin output.bin "7z a -txz -mx=9 {stdin} {stdout}"
```

## Contoh Penggunaan

### Precompression only (Layer 1)
```bash
xtool precomp -mzstd+preflate -c64mb -t100p-1 --dbase --dedup=dedup.bin - -
```

### Precompression + built-in LZMA2 (Layer 1 + 2)
```bash
xtool precomp -mzstd+preflate -c64mb -t100p-1 -l5:d128mb --dbase --dedup=dedup.bin - -
```

### Precompression + external compressor (Layer 1 + 2)
```bash
xtool precomp -mzstd+preflate -c64mb -t100p-1 -e"7z a -txz -mx=9 {stdin} {stdout}" --dbase - -
```

### Decode
```bash
xtool decode -t100p-1 --dedup=dedup.bin - - 
```
