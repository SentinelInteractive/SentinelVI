# sentinel-rpf9-core

Rust core for reading GTA VI containers without keys: RPF9 archives, RDS5/ETPF
texture databases and RAGE9 resource pages (drawables, navmeshes), plus MHIU
heightmaps, PSIN metadata and ADAT audio banks.

* Architecture: [`DESIGN.md`](DESIGN.md)
* Port map, verification evidence, gaps: [`PORT_REPORT.md`](PORT_REPORT.md)

## Build

```powershell
cargo build --release --offline     # dependency cache is warm; no network needed
```

Dependencies: `memmap2`, `rayon`, `flate2` (rust backend), `texture2ddecoder`
(BC6H/BC7 only).

## CLI

```powershell
$exe = ".\target\release\srpf9.exe"

& $exe info   ..\common.rpf                     # RPF9 header, or RDS5 if given a .ttpf
& $exe scan   ..\packs.rpf                      # census of every RPF9 header
& $exe carve  ..\common.rpf  out                 # zlib streams + index.csv
& $exe mhiu   ..\common.rpf --png map.png        # 2048x2048 world heightmap
& $exe tpfd   <file>.ttpf --decode-all           # RDS5 table + batch texture decode
& $exe tpfd   <file>.ttpf --tex 15 --png t.png --dds t.dds
& $exe model  <levels.rpf> 8679358480 --obj slod.obj
& $exe psin   <file>.psin
& $exe awc    <audio.rpf> --offset 0x129b000
& $exe bench  <archive>                          # census + carve throughput
```

## Library

```rust
use sentinel_rpf9_core::{mesh, mhiu, rpf9, source, tpfd};

let src = source::open(path, 8 << 30)?;            // mmap below cap, stream above
let streams = rpf9::carve(&*src, &rpf9::CarveOptions::default())?;
let m = mesh::decode(&*src, offset, "fallback")?;  // parts, verts, tris, uvs
let db = tpfd::Rds5::parse(&*src)?;
let tex = db.decode(&*src, idx)?;                  // RGBA + packed blocks
let map = mhiu::find_in_archive(&*src, 0x40)?;     // elevation raster
```

## Tests

Fixtures are opt-in so `cargo test` runs anywhere:

```powershell
$env:SRPF9_COMMON   = "..\common.rpf"
$env:SRPF9_GRAPHICS = "...\data\ttdb\graphics\ttdb_graphics.ttpf"
$env:SRPF9_LEVELS   = "...\levels.rpf"
cargo test --release --offline
```

The fixtures assert the numbers in `PORT_REPORT.md` (954 streams / 213 complete
XML in `common.rpf`, 1550 chunks / 912 ETPF descriptors in the graphics database,
8 parts and 317 geometries for the Brickell SLOD drawable), so a regression fails
the build instead of silently changing the inventory.

## License

MIT OR Apache-2.0. Independent implementation from the file formats; no code was
taken from any third-party viewer. See `PORT_REPORT.md` §7 for the reasoning.
