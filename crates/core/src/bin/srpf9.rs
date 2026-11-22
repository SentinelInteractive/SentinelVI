//! `srpf9` — command line front end for `sentinel-rpf9-core`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use sentinel_rpf9_core::awc::Awc;
use sentinel_rpf9_core::img;
use sentinel_rpf9_core::mesh;
use sentinel_rpf9_core::mhiu::{self, Mhiu};
use sentinel_rpf9_core::psin;
use sentinel_rpf9_core::rpf9::{self, CarveOptions, Header, Kind};
use sentinel_rpf9_core::source::{open, Source};
use sentinel_rpf9_core::tpfd::Rds5;

const MMAP_LIMIT: u64 = 8 << 30;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        usage();
        return ExitCode::SUCCESS;
    }
    let cmd = args[0].clone();
    let rest = &args[1..];
    let result = match cmd.as_str() {
        "info" => cmd_info(rest),
        "scan" => cmd_scan(rest),
        "carve" => cmd_carve(rest),
        "mhiu" => cmd_mhiu(rest),
        "tpfd" => cmd_tpfd(rest),
        "model" => cmd_model(rest),
        "psin" => cmd_psin(rest),
        "awc" => cmd_awc(rest),
        "bench" => cmd_bench(rest),
        other => {
            eprintln!("unknown command '{other}'");
            usage();
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!(
        "srpf9 {} — RPF9 / RDS5-ETPF / RAGE9 core\n\
\n\
usage:\n\
  srpf9 info   <archive|ttpf>            parse the container header\n\
  srpf9 scan   <archive>                 census every RPF9 header in the file\n\
  srpf9 carve  <archive> <outdir> [opts] carver; opts: --step N --min-size N --stdout\n\
  srpf9 mhiu   <archive|file> [opts]     decode the world heightmap; --png P --shade P\n\
  srpf9 tpfd   <ttpf> [opts]             parse RDS5/ETPF\n\
                                         --csv P  --tex N  --png P\n\
                                         --dds P [--dds-tex N]  (raw blocks, no decode)\n\
  srpf9 model  <archive> <offset> [opts] decode a drawable; --obj P --json\n\
  srpf9 psin   <file>                    PSIN sections, schemas, instance table\n\
  srpf9 awc    <file> [--offset N]       ADAT bank header and chunk table\n\
  srpf9 bench  <archive>                 throughput for scan and carve\n",
        sentinel_rpf9_core::VERSION
    );
}

fn need<'a>(args: &'a [String], i: usize, name: &str) -> Result<&'a str, String> {
    args.get(i)
        .map(|s| s.as_str())
        .ok_or_else(|| format!("missing argument: {name}"))
}

fn opt<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn opt_num(args: &[String], flag: &str) -> Option<u64> {
    opt(args, flag).and_then(|v| parse_num(v).ok())
}

fn parse_num(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (digits, radix) = if let Some(hex) = s.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(hex) = s.strip_prefix("0X") {
        (hex, 16)
    } else {
        (s, 10)
    };
    u64::from_str_radix(digits, radix).map_err(|_| format!("not a number: {s}"))
}

fn src(path: &str) -> Result<Box<dyn Source>, String> {
    open(path, MMAP_LIMIT).map_err(|e| format!("open {path}: {e}"))
}

fn cmd_info(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "file")?;
    let s = src(path)?;
    let head = s.read_at(0, 0x200).map_err(|e| e.to_string())?;
    if head.len() >= 4 && &head[..4] == b"RDS5" {
        let r = Rds5::parse(&*s).map_err(|e| e.to_string())?;
        let counts = r.codec_census();
        println!("file        {path}");
        println!("size        {}", r.file_len);
        println!("magic       RDS5");
        println!("entries     {}", r.entries);
        println!(
            "header      {} pages -> ETPF at 0x{:x}",
            r.header_pages,
            r.header_pages as u64 * 0x100
        );
        println!(
            "etpf        A={} B={} C={} ({} descriptors)",
            r.etpf.a,
            r.etpf.b,
            r.etpf.c,
            r.descs.len()
        );
        println!("flags       0x{:08X} 0x{:08X}", r.flags[0], r.flags[1]);
        println!("chunks      {}", r.chunks.len());
        let names = ["RAW", "BC1", "BC2", "BC3", "BC4", "BC5", "BC6H", "BC7", "ZLIB", "-"];
        for (i, n) in counts.iter().enumerate() {
            if *n > 0 {
                println!("  codec {i:<2} {:<5} {n}", names[i]);
            }
        }
        println!("virtual     {} bytes ({:.1} MB)", r.virtual_size(), r.virtual_size() as f64 / 1e6);
        return Ok(());
    }
    let Some(h) = Header::parse_at(&head, 0) else {
        return Err(format!("{path}: no RPF9 header at 0"));
    };
    print_header(&h, s.len());
    Ok(())
}

fn print_header(h: &Header, file_len: u64) {
    let (hi, lo) = h.version_pair();
    println!("offset      0x{:x}", h.offset);
    println!("header_len  0x{:x} (payload start)", h.header_len);
    println!("split       0x{:x} (end of table 1)", h.split);
    println!("trailer     0x{:x} bytes", h.trailer_len);
    println!("archive_id  0x{:016x}", h.archive_id);
    println!("version     0x{:08x} (rev {hi}.{lo})", h.version);
    println!("count       {}", h.count);
    println!("key         0x{:08X}", h.key);
    println!("flags       0x{:08X}", h.flags);
    println!(
        "table1      {} bytes / {} records",
        h.table1_bytes,
        h.table1_records()
    );
    println!(
        "table2      {} bytes / {} records",
        h.table2_bytes,
        h.table2_records()
    );
    println!(
        "payload     0x{:x}..0x{:x} ({} bytes)",
        h.header_len as u64,
        file_len.saturating_sub(h.trailer_len as u64),
        file_len.saturating_sub(h.trailer_len as u64) - h.header_len as u64
    );
    println!(
        "prefix      {}",
        if h.has_standard_prefix {
            "standard (enciphered table 1)"
        } else {
            "different from the known constant"
        }
    );
}

fn cmd_scan(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "archive")?;
    let s = src(path)?;
    let t0 = Instant::now();
    let hits = rpf9::scan_headers(&*s, 32 << 20).map_err(|e| e.to_string())?;
    let dt = t0.elapsed();
    println!("file        {path}");
    println!("size        {}", s.len());
    println!("headers     {}", hits.len());
    println!(
        "throughput  {:.0} MB/s",
        s.len() as f64 / 1e6 / dt.as_secs_f64().max(1e-9)
    );
    println!("offset            hdrlen  split   trailer  version   count     key         flags");
    for hit in &hits {
        let h = &hit.header;
        println!(
            "0x{:012x} 0x{:06x} 0x{:06x} 0x{:06x} 0x{:07x} {:7} 0x{:08X} 0x{:08X}",
            hit.offset, h.header_len, h.split, h.trailer_len, h.version, h.count, h.key, h.flags
        );
    }
    Ok(())
}

fn cmd_carve(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "archive")?;
    let outdir = PathBuf::from(need(args, 1, "outdir")?);
    let step = opt_num(args, "--step").unwrap_or(1) as usize;
    let min_size = opt_num(args, "--min-size").unwrap_or(0) as usize;
    let keep = opt_num(args, "--keep-below").unwrap_or(0) as usize;
    let s = src(path)?;
    let t0 = Instant::now();
    let opts = CarveOptions {
        step,
        keep_below: keep,
        max_stream: 512 << 20,
        parallel: true,
    };
    let streams = rpf9::carve(&*s, &opts).map_err(|e| e.to_string())?;
    let dt = t0.elapsed();
    let mut bytes_written = 0usize;
    let mut kept = 0usize;
    if !has(args, "--stdout") {
        std::fs::create_dir_all(&outdir).map_err(|e| e.to_string())?;
    }
    let mut index = Vec::new();
    index.push("offset,compressed_size,uncompressed_size,kind,fingerprint,output".to_string());
    for st in &streams {
        if st.raw_size < min_size {
            continue;
        }
        let mut rel = String::new();
        if !has(args, "--stdout") {
            let dir = outdir.join(st.kind.as_str());
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let name = format!("0x{:09X}_{:08x}{}", st.offset, st.fingerprint as u32, st.kind.ext());
            let target = dir.join(&name);
            if let Some(data) = &st.data {
                std::fs::write(&target, data).map_err(|e| e.to_string())?;
                bytes_written += data.len();
                kept += 1;
            }
            rel = target.display().to_string();
        } else if let Some(data) = &st.data {
            std::io::stdout().write_all(data).ok();
        }
        index.push(format!(
            "0x{:09X},{},{},{},{:016x},{}",
            st.offset,
            st.comp_size,
            st.raw_size,
            st.kind.as_str(),
            st.fingerprint,
            rel
        ));
    }
    if !has(args, "--stdout") {
        std::fs::write(outdir.join("index.csv"), index.join("\n")).map_err(|e| e.to_string())?;
    }
    let mut kinds = std::collections::BTreeMap::new();
    for st in &streams {
        *kinds.entry(st.kind.as_str()).or_insert(0usize) += 1;
    }
    println!("file        {path}");
    println!("streams     {}", streams.len());
    println!("kinds       {kinds:?}");
    println!(
        "xml docs    {}",
        streams.iter().filter(|s| s.is_complete_xml()).count()
    );
    println!("written     {:.1} MB ({kept} payloads retained)", bytes_written as f64 / 1e6);
    println!("elapsed     {:.2}s", dt.as_secs_f64());
    println!(
        "throughput  {:.0} MB/s",
        s.len() as f64 / 1e6 / dt.as_secs_f64().max(1e-9)
    );
    let _: Kind = Kind::Bin;
    Ok(())
}

fn cmd_mhiu(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "file")?;
    let s = src(path)?;
    let head = s.read_at(0, 4).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let map = if head == b"MHIU" {
        let data = s.read_at(0, s.len() as usize).map_err(|e| e.to_string())?;
        Mhiu::parse(&data).ok_or("MHIU parse failed")?
    } else {
        mhiu::find_in_archive(&*s, 0x40).map_err(|e| e.to_string())?
    };
    let dt = t0.elapsed();
    let st = map.stats();
    println!("file        {path}");
    println!("raster      {}x{} ({} rows present)", map.width, map.height, map.rows_present);
    println!(
        "bounds      x {:.0}..{:.0}  y {:.0}..{:.0}  z {:.0}..{:.0} m",
        map.bmin[0], map.bmax[0], map.bmin[1], map.bmax[1], map.bmin[2], map.bmax[2]
    );
    println!("elevation   {:.1}..{:.1} m", st.min_z, st.max_z);
    println!(
        "land        {} / {} texels ({:.1}%)",
        st.land_texels,
        st.texels,
        100.0 * st.land_texels as f64 / st.texels as f64
    );
    println!("decoded in  {:.2}s", dt.as_secs_f64());
    if let Some(p) = opt(args, "--png") {
        let rgba = map.to_rgba();
        img::write_png_rgba(Path::new(p), map.width, map.height, &rgba).map_err(|e| e.to_string())?;
        println!("wrote       {p}");
    }
    if let Some(p) = opt(args, "--shade") {
        let gray = map.hillshade();
        img::write_png_rgba(Path::new(p), map.width, map.height, &img::gray_to_rgba(&gray))
            .map_err(|e| e.to_string())?;
        println!("wrote       {p}");
    }
    Ok(())
}

fn cmd_tpfd(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "ttpf")?;
    let s = src(path)?;
    let t0 = Instant::now();
    let r = Rds5::parse(&*s).map_err(|e| e.to_string())?;
    let dt = t0.elapsed();
    println!("file        {path}");
    println!("entries     {}", r.entries);
    println!(
        "etpf        A={} B={} C={} descriptors={} at 0x{:x}",
        r.etpf.a,
        r.etpf.b,
        r.etpf.c,
        r.descs.len(),
        r.header_pages as u64 * 0x100
    );
    let counts = r.codec_census();
    let names = ["RAW", "BC1", "BC2", "BC3", "BC4", "BC5", "BC6H", "BC7", "ZLIB", "-"];
    let mut line = String::from("codecs     ");
    for (i, n) in counts.iter().enumerate() {
        if *n > 0 {
            line.push_str(&format!("{}={n} ", names[i]));
        }
    }
    println!("{line}");
    println!("chunks      {}", r.chunks.len());
    println!("virtual     {} bytes", r.virtual_size());
    println!("parsed in   {:.3}s", dt.as_secs_f64());

    if let Some(p) = opt(args, "--csv") {
        let mut f = std::fs::File::create(p).map_err(|e| e.to_string())?;
        writeln!(f, "index,width,height,dxgi,format_byte,mips,virt_offset,alloc_hint").ok();
        for d in &r.descs {
            writeln!(
                f,
                "{},{},{},{},{},{},{},{}",
                d.index, d.width, d.height, d.dxgi, d.format_byte, d.mips, d.virt_offset, d.alloc_hint
            )
            .map_err(|e| e.to_string())?;
        }
        println!("wrote       {p}");
    }
    // DDS export is deliberately independent of `--tex`: the formats that most
    // need it (BC6H/BC7 before the BPTC decoders landed, and any format a build
    // cannot decode) fail inside `decode()`, which used to take the export with
    // them.
    if let Some(p) = opt(args, "--dds") {
        let idx = opt_num(args, "--dds-tex")
            .or_else(|| opt_num(args, "--tex"))
            .ok_or("--dds needs a surface: pass --tex N (or --dds-tex N)")?
            as usize;
        let dds = r.dds(&*s, idx).map_err(|e| e.to_string())?;
        std::fs::write(p, dds).map_err(|e| e.to_string())?;
        println!("wrote       {p}");
    }
    // Batch decode: throughput over the whole descriptor table, with a failure
    // breakdown that separates "not decodable" (BCPack) from "not implemented".
    if has(args, "--decode-all") {
        let t1 = Instant::now();
        let mut ok = 0usize;
        let mut texels = 0u64;
        let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
        for d in &r.descs {
            match r.decode(&*s, d.index) {
                Ok(t) => {
                    ok += 1;
                    texels += (t.width * t.height) as u64;
                }
                Err(e) => {
                    let key = e
                        .to_string()
                        .split(|c: char| c == '(' || c == ':' )
                        .next()
                        .unwrap_or("error")
                        .trim()
                        .to_string();
                    *reasons.entry(key).or_insert(0) += 1;
                }
            }
        }
        let dt1 = t1.elapsed();
        let mb = texels as f64 * 4.0 / 1e6;
        println!(
            "decode-all  {ok}/{} surfaces, {:.1} Mtexels ({:.0} MB RGBA) in {:.2}s ({:.1} Mtexels/s, {:.0} MB/s)",
            r.descs.len(),
            texels as f64 / 1e6,
            mb,
            dt1.as_secs_f64(),
            texels as f64 / 1e6 / dt1.as_secs_f64().max(1e-9),
            mb / dt1.as_secs_f64().max(1e-9)
        );
        for (why, n) in reasons {
            println!("  skipped   {n:6} {why}");
        }
    }
    if let Some(n) = opt_num(args, "--tex") {
        let idx = n as usize;
        let t1 = Instant::now();
        let tex = r.decode(&*s, idx).map_err(|e| e.to_string())?;
        println!(
            "texture     {} {}x{} {} mips={} surface=0x{:x} tiles={} packed={} B",
            tex.name,
            tex.width,
            tex.height,
            tex.format_name,
            tex.mips,
            tex.surface_offset,
            if tex.used_64kb_tiles { "64KB" } else { "4KB" },
            tex.packed_len
        );
        // Cheap content sanity: count distinct colours in a sample.
        let mut sample = std::collections::HashSet::new();
        for px in tex.rgba.chunks_exact(4).take(4096) {
            sample.insert((px[0], px[1], px[2]));
        }
        println!("colors      {} distinct in first 4096 texels", sample.len());
        println!("decoded in  {:.3}s", t1.elapsed().as_secs_f64());
        if let Some(p) = opt(args, "--png") {
            img::write_png_rgba(Path::new(p), tex.width, tex.height, &tex.rgba)
                .map_err(|e| e.to_string())?;
            println!("wrote       {p}");
        }
    }
    Ok(())
}

fn cmd_model(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "archive")?;
    let off = parse_num(need(args, 1, "offset")?)?;
    let s = src(path)?;
    let t0 = Instant::now();
    let m = mesh::decode(&*s, off, "resource").map_err(|e| e.to_string())?;
    let dt = t0.elapsed();
    let (lo, hi) = m.bounds();
    println!("resource    0x{off:x} in {path}");
    println!(
        "name        {} ({}){}",
        m.name,
        m.ext,
        if m.is_vehicle { " [vehicle]" } else { "" }
    );
    println!(
        "geometry    {} geoms, {} meshlets, {} verts, {} tris, {} bones",
        m.geom_count,
        m.meshlet_count,
        m.verts.len(),
        m.tris.len(),
        m.bone_count
    );
    println!(
        "bounds      x {:.1}..{:.1}  y {:.1}..{:.1}  z {:.1}..{:.1} m",
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
    );
    println!(
        "extents     {:.1} x {:.1} x {:.1} m",
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2]
    );
    for part in &m.parts {
        let (plo, phi) = m.part_bounds(part);
        println!(
            "  part {:<28} geoms={:<4} tris={:<7} extents {:.1}x{:.1}x{:.1}",
            part.name,
            part.geoms,
            part.tris.len(),
            phi[0] - plo[0],
            phi[1] - plo[1],
            phi[2] - plo[2]
        );
    }
    println!("decoded in  {:.3}s", dt.as_secs_f64());
    if let Some(p) = opt(args, "--obj") {
        m.write_obj(Path::new(p)).map_err(|e| e.to_string())?;
        println!("wrote       {p}");
    }
    Ok(())
}

fn cmd_psin(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "file")?;
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let p = psin::parse(&data);
    println!("file        {path}");
    println!("size        {}", data.len());
    if data.len() >= 0x10 && &data[..4] == b"PSIN" {
        println!(
            "header      flags=0x{:08X} hash=0x{:08X} field=0x{:X}",
            u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            u32::from_le_bytes([data[12], data[13], data[14], data[15]])
        );
    }
    println!("sections    {}", p.sections.len());
    for s in &p.sections {
        println!("  0x{:04X} {} size={}", s.offset, s.tag_str(), s.size);
    }
    println!(
        "schemas     {} structs, {} enums, {} instances, payload {} B",
        p.structs.len(),
        p.enums.len(),
        p.instances.len(),
        p.payload_len
    );
    for st in p.structs.iter().take(12) {
        println!(
            "  struct 0x{:08x} size=0x{:x} fields={}",
            st.hash,
            st.size,
            st.fields.len()
        );
        for f in st.fields.iter().take(6) {
            println!(
                "    +0x{:03x} name=0x{:08x} type={} (0x{:02x}) ref=0x{:08x}",
                f.offset,
                f.name_hash,
                psin::type_kind(f.type_raw),
                f.type_raw & 0xFF,
                f.ref_hash
            );
        }
    }
    for e in p.enums.iter().take(6) {
        println!("  enum 0x{:08x} values={}", e.hash, e.values.len());
        for (v, n) in e.values.iter().take(4) {
            println!("    {v} = 0x{n:08x}");
        }
    }
    for i in p.instances.iter().take(8) {
        println!(
            "  instance type=0x{:08x} size={} payload_off=0x{:x}",
            i.type_hash, i.size, i.offset
        );
    }
    Ok(())
}

fn cmd_awc(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "file")?;
    let off = opt_num(args, "--offset").unwrap_or(0);
    let s = src(path)?;
    let a = Awc::parse(&*s, off).map_err(|e| e.to_string())?;
    println!("file        {path} @ 0x{off:x}");
    println!("flags       0x{:08X}", a.flags);
    println!("streams     {} header={} B bank={} B", a.streams, a.header_len, a.bank_size);
    println!("chunks      {}", a.chunks.len());
    let mut kinds = std::collections::BTreeMap::new();
    for c in &a.chunks {
        *kinds.entry(c.kind).or_insert(0usize) += 1;
    }
    let pretty: Vec<String> = kinds
        .iter()
        .map(|(k, n)| format!("0x{k:02x}={n}"))
        .collect();
    println!("types       {}", pretty.join(" "));
    if let Some(f) = a.format {
        println!(
            "format      {} Hz {} ch codec=0x{:02x} samples={}",
            f.sample_rate, f.channels, f.codec, f.samples
        );
    }
    let payloads = a.payload_chunks().count();
    println!("payloads    {payloads} opus chunks");
    if let Some(first) = a.payload_chunks().next() {
        let blob = s
            .read_at(off + first.offset, first.size.min(1 << 20))
            .map_err(|e| e.to_string())?;
        let packets = Awc::split_opus_packets(&blob);
        println!(
            "frames      {} packets in first payload, {} ch",
            packets.len(),
            Awc::channels(&packets)
        );
    }
    println!("plan        {}", a.export_plan());
    Ok(())
}

fn cmd_bench(args: &[String]) -> Result<(), String> {
    let path = need(args, 0, "archive")?;
    let s = src(path)?;
    println!("file        {path} ({} bytes)", s.len());

    let t0 = Instant::now();
    let head = rpf9::peek_header(&*s).map_err(|e| e.to_string())?;
    println!("header      parsed in {:?}", t0.elapsed());
    if let Some(h) = &head {
        print_header(&h.header, s.len());
    }

    let t0 = Instant::now();
    let hits = rpf9::scan_headers(&*s, 32 << 20).map_err(|e| e.to_string())?;
    let dt = t0.elapsed();
    println!(
        "scan        {} headers in {:.2}s ({:.0} MB/s)",
        hits.len(),
        dt.as_secs_f64(),
        s.len() as f64 / 1e6 / dt.as_secs_f64().max(1e-9)
    );

    let t0 = Instant::now();
    let opts = CarveOptions::default();
    let streams = rpf9::carve(&*s, &opts).map_err(|e| e.to_string())?;
    let dt = t0.elapsed();
    let raw: usize = streams.iter().map(|s| s.raw_size).sum();
    println!(
        "carve       {} streams, {:.1} MB inflated in {:.2}s ({:.0} MB/s in, {:.0} MB/s out)",
        streams.len(),
        raw as f64 / 1e6,
        dt.as_secs_f64(),
        s.len() as f64 / 1e6 / dt.as_secs_f64().max(1e-9),
        raw as f64 / 1e6 / dt.as_secs_f64().max(1e-9)
    );
    Ok(())
}

