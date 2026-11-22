# Resume notes

State at the first published commit. Everything below is measured, not planned.

## Working

- `cargo build --release --offline` succeeds for the whole workspace.
- `sentinel info`, `sentinel toc stats`, `sentinel toc search`, `sentinel toc archives`, `sentinel export`, `sentinel textures list`, `sentinel anim list`, `sentinel audio list`, `sentinel ui`.
- Library: 2,067,172 payloads, 538,772 inferred names, 4,325 packs in 18 archives. Search returns real names with the confidence class.
- Asset: metadata, first 512 bytes as hex, export through the bin store (writes a job).
- Map: `sentinel\placement.sqlite` built from `levels.rpf`: 183,992 resources, 2,244,675 entity records read, 2,222,669 stored, 1,458,675 prop instances stored (one in four), 49,523 interior points.
- Textures: 58 `.ttpf` databases, descriptor tables, decoded previews (BC1, BC3, BC5, BC7 and uncompressed), PNG export.
- Audio: `sentinel\audio.sqlite` holds 5,235 ADAT banks; a bank is muxed into Ogg Opus at request time and plays in the browser.
- Animation: 13,199 clip names from the directory index.
- Jobs: export and index builds with state, progress and output path.

## Deferred, with the reason

- **LOD filter on the map.** No per-record LOD field is decoded, so the screen offers record type (entity, prop) and interior resources instead. Inventing a LOD control would be a fake filter.
- **Animation frame counts and durations.** The clip record at virtual `0x740` (stride `0x70`) parses, but no resource sampled in the first 400 MB of `anim.rpf` carries a `.clip` name string in its virtual pages, so no record can be paired with a name. The screen lists the 13,199 clip names the directory index already holds and states that frame data is not decoded. Next step: locate the metadata payload family (the 737 streams that hold `.clip` strings) and read records from there.
- **Prop coverage.** 6,186,628 prop records were read and 1,458,675 stored, against 24,886,645 in the research census. Most prop blocks live in physical pages that the carver does not resolve for every resource. The screen states what is stored and the ratio it applies.
- **Map sampling.** A view that holds more than 20,000 placements shows 20,000 of them, in insertion order, not a random sample. The caption reports the totals; it does not claim the drawn subset is representative.
- **Wide-view count latency.** The `count(*)` in `/api/map/points` takes about three seconds on the full world bound. Panning inside the city is faster because the index range is smaller.
- **3D map tab.** Not started.

## Screenshots

`docs/library-1280.png`, `docs/library-375.png`, `docs/map-1280.png`, `docs/map-375.png`, `docs/asset-1280.png`, `docs/textures-1280.png`, `docs/audio-1280.png`, `docs/animation-1280.png`, `docs/jobs-1280.png`.

The 375 px screenshots are rendered in a 375 px iframe because headless Chrome on Windows refuses a window narrower than about 500 px. The iframe viewport is a real 375 px layout viewport.
