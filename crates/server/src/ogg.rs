/// Parameters for the OpusHead packet. `sample_rate` is the original input rate;
/// Ogg Opus granule positions are always counted at 48 kHz.
pub struct OpusStream {
    pub channels: u8,
    pub sample_rate: u32,
    pub pre_skip: u16,
}

const SERIAL: u32 = 0x5345_4E54;
const PAGE_BUDGET: usize = 4000;
const CRC_POLY: u32 = 0x04C1_1DB7;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ CRC_POLY
            } else {
                c << 1
            };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// MSB-first Ogg CRC: polynomial 0x04C11DB7, init 0, no reflection, no final
/// xor. The page is passed with its CRC field zeroed.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in bytes {
        let idx = ((crc >> 24) as u8 ^ b) as usize;
        crc = (crc << 8) ^ CRC_TABLE[idx];
    }
    crc
}

fn lacing(packet: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(packet.len() / 255 + 1);
    let mut left = packet.len();
    while left >= 255 {
        out.push(255);
        left -= 255;
    }
    out.push(left as u8);
    out
}

fn page(serial: u32, seq: u32, granule: u64, flags: u8, lace: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(27 + lace.len() + body.len());
    out.extend_from_slice(b"OggS");
    out.push(0);
    out.push(flags);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(lace.len() as u8);
    out.extend_from_slice(lace);
    out.extend_from_slice(body);
    let crc = crc32(&out);
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}

fn opus_head(stream: &OpusStream) -> Vec<u8> {
    let channels = stream.channels.clamp(1, 2);
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(channels);
    head.extend_from_slice(&stream.pre_skip.to_le_bytes());
    head.extend_from_slice(&stream.sample_rate.to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes());
    head.push(0);
    head
}

fn opus_tags() -> Vec<u8> {
    let comment = b"ENCODER=SentinelVI";
    let mut tags = Vec::with_capacity(8 + 4 + 4 + 4 + comment.len());
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags.extend_from_slice(&1u32.to_le_bytes());
    tags.extend_from_slice(&(comment.len() as u32).to_le_bytes());
    tags.extend_from_slice(comment);
    tags
}

/// Granule is cumulative 48 kHz samples including pre-skip. The declared
/// duration is spread evenly across packets so the player reports the same
/// length even when packet durations are not an exact multiple of 20 ms.
fn granule(duration: f32, pre_skip: u16, index: usize, count: usize) -> u64 {
    let total = (duration.max(0.0) as f64 * 48_000.0).round() as u64;
    let numerator = total as u128 * (index as u128 + 1) + count as u128 / 2;
    (numerator / count as u128) as u64 + pre_skip as u64
}

/// Mux raw Opus packets into a playable Ogg Opus stream.
pub fn mux_opus(stream: &OpusStream, packets: &[&[u8]], duration_seconds: f32) -> Vec<u8> {
    let head = opus_head(stream);
    let head_lace = lacing(&head);
    let mut out = page(SERIAL, 0, 0, 0x02, &head_lace, &head);

    let tags = opus_tags();
    let tags_lace = lacing(&tags);
    let tags_flags = if packets.is_empty() { 0x04 } else { 0x00 };
    out.extend(page(SERIAL, 1, 0, tags_flags, &tags_lace, &tags));
    if packets.is_empty() {
        return out;
    }

    let mut seq = 2u32;
    let mut lace: Vec<u8> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut last_index = 0usize;

    for (i, packet) in packets.iter().enumerate() {
        let seg = lacing(packet);
        let used = 27 + lace.len() + body.len();
        let added = seg.len() + packet.len();
        if !lace.is_empty() && (used + added > PAGE_BUDGET || lace.len() + seg.len() > 255) {
            let g = granule(duration_seconds, stream.pre_skip, last_index, packets.len());
            out.extend(page(SERIAL, seq, g, 0x00, &lace, &body));
            seq += 1;
            lace.clear();
            body.clear();
        }
        lace.extend_from_slice(&seg);
        body.extend_from_slice(packet);
        last_index = i;
    }

    let g = granule(duration_seconds, stream.pre_skip, last_index, packets.len());
    out.extend(page(SERIAL, seq, g, 0x04, &lace, &body));
    out
}
