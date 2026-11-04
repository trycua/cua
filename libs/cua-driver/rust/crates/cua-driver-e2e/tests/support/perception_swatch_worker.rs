//! Deterministic, model-free stand-in for the `cua-perception` worker.
//!
//! The canonical desktop E2E installs this program as a developer-only
//! unsigned extension so Driver's real capture -> parse -> capture-bound action
//! loop runs without a published artifact or model weights. It speaks the
//! same framed `cua-perception/1` protocol as the shipped worker, receives the
//! exact PNG retained by Driver, and derives regions from those pixels: it
//! decodes the PNG, finds the solid color swatches painted by the shared web
//! harness (`#drag-source` and `#drop-target`), and reports each swatch as one
//! icon region in source-image pixels. It also echoes the PNG SHA-256 so
//! Driver proves the worker saw the retained capture bytes.
//!
//! The test compiles this single file with `rustc` and no external crates,
//! because the contained worker may execute only its own statically linked
//! image on Linux. It is test support only and is never shipped.

use std::io::{self, Read, Write};

const PROTOCOL: &str = "cua-perception/1";
/// Stable identity of this detector specification. Driver requires a
/// SHA-256-shaped fixture identity for the `deterministic_fixture` backend.
const DETECTOR_ID_SHA256: &str = "5c0a7c4e3b1f6d2a9e8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b9c8d7e6f";

struct Swatch {
    name: &'static str,
    rgb: [u8; 3],
}

/// Solid backgrounds from `libs/cua-driver/tests/fixtures/shared/web/index.html`.
const SWATCHES: &[Swatch] = &[
    Swatch {
        name: "drag-source",
        rgb: [0x12, 0x68, 0xd6],
    },
    Swatch {
        name: "drop-target",
        rgb: [0x17, 0x8a, 0x38],
    },
];
/// Per-channel tolerance for display color management and compositing.
const TOLERANCE: i32 = 40;
/// Smallest accepted swatch, in pixels. The fixture swatch is 110x48 CSS px.
const MIN_PIXELS: usize = 600;
/// A swatch is a filled rectangle; focus rings and anti-aliased edges are not.
const MIN_FILL: f64 = 0.80;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let argument = |name: &str| -> String {
        args.iter()
            .position(|value| value == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("perception swatch worker requires {name}");
                std::process::exit(2);
            })
    };
    let identity = format!(
        r#"{{"extension":{{"id":"{}","version":"{}"}},"backend":"deterministic_fixture","fixture_sha256":"{DETECTOR_ID_SHA256}"}}"#,
        json_escape(&argument("--extension-id")),
        json_escape(&argument("--extension-version")),
    );

    while let Some(payload) = read_frame() {
        let request_id = string_field(&payload, "request_id").unwrap_or_default();
        let response = match string_field(&payload, "method").as_deref() {
            Some("health") => format!(
                r#"{{"protocol":"{PROTOCOL}","request_id":"{}","status":"ok","result":{{"ready":true,"protocol":"{PROTOCOL}","identity":{identity}}}}}"#,
                json_escape(&request_id)
            ),
            Some("parse") => match parse(&payload) {
                Ok(result) => format!(
                    r#"{{"protocol":"{PROTOCOL}","request_id":"{}","status":"ok","result":{{"runtime":"fixture_only","identity":{identity},{result}}}}}"#,
                    json_escape(&request_id)
                ),
                Err((code, message)) => error_frame(&request_id, code, &message),
            },
            _ => error_frame(&request_id, "invalid_request", "unsupported method"),
        };
        write_frame(&response);
    }
}

fn error_frame(request_id: &str, code: &str, message: &str) -> String {
    format!(
        r#"{{"protocol":"{PROTOCOL}","request_id":"{}","status":"error","error":{{"code":"{code}","message":"{}"}}}}"#,
        json_escape(request_id),
        json_escape(message)
    )
}

fn parse(payload: &str) -> Result<String, (&'static str, String)> {
    let invalid = |message: String| ("invalid_image", message);
    let capture_id = string_field(payload, "capture_id")
        .ok_or_else(|| ("invalid_request", "parse omitted capture_id".to_owned()))?;
    let encoded = string_field(payload, "data_base64")
        .ok_or_else(|| ("invalid_request", "parse omitted image data".to_owned()))?;
    let png = base64_decode(&encoded).map_err(invalid)?;
    let image = decode_png(&png).map_err(invalid)?;

    let mut regions = Vec::new();
    let mut diagnostics = Vec::new();
    for swatch in SWATCHES {
        match find_swatch(&image, swatch.rgb) {
            Ok(bounds) => regions.push((swatch.name, bounds)),
            Err(diagnostic) => diagnostics.push(format!("{}: {diagnostic}", swatch.name)),
        }
    }
    if regions.is_empty() {
        return Err((
            "inference_failed",
            format!(
                "no harness swatch in {}x{} capture; {}",
                image.width,
                image.height,
                diagnostics.join("; ")
            ),
        ));
    }
    let regions = regions
        .iter()
        .enumerate()
        .map(|(order, (name, (x, y, width, height)))| {
            format!(
                r#"{{"id":"swatch-{name}","kind":"icon","bounds":{{"x":{x},"y":{y},"width":{width},"height":{height}}},"label":"{name} swatch","confidence":0.99,"interactive":true,"reading_order":{order}}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        r#""capture_id":"{}","image":{{"sha256":"{}","width":{},"height":{}}},"coordinate_space":"image_pixels","regions":[{regions}]"#,
        json_escape(&capture_id),
        sha256_hex(&png),
        image.width,
        image.height
    ))
}

// ---------------------------------------------------------------- framing

fn read_frame() -> Option<String> {
    let mut stdin = io::stdin().lock();
    let mut length = [0_u8; 4];
    stdin.read_exact(&mut length).ok()?;
    let mut payload = vec![0_u8; u32::from_be_bytes(length) as usize];
    stdin.read_exact(&mut payload).ok()?;
    String::from_utf8(payload).ok()
}

fn write_frame(payload: &str) {
    let bytes = payload.as_bytes();
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .and_then(|()| stdout.write_all(bytes))
        .and_then(|()| stdout.flush())
        .unwrap_or_else(|_| std::process::exit(1));
}

/// Extract one JSON string value by key. Driver serializes requests without
/// whitespace, and every key read here is unique in the request.
fn string_field(payload: &str, name: &str) -> Option<String> {
    let marker = format!("\"{name}\":\"");
    let start = payload.find(&marker)? + marker.len();
    let tail = &payload[start..];
    let mut value = String::new();
    let mut chars = tail.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                other => value.push(other),
            },
            other => value.push(other),
        }
    }
    None
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if (ch as u32) < 0x20 => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

// ---------------------------------------------------------------- detection

struct Rgb {
    width: usize,
    height: usize,
    pixels: Vec<[u8; 3]>,
}

type Bounds = (usize, usize, usize, usize);

/// Largest filled 4-connected component near `target`, as `(x, y, w, h)`.
fn find_swatch(image: &Rgb, target: [u8; 3]) -> Result<Bounds, String> {
    let matches = |pixel: &[u8; 3]| {
        pixel
            .iter()
            .zip(target.iter())
            .all(|(actual, wanted)| (*actual as i32 - *wanted as i32).abs() <= TOLERANCE)
    };
    let mut visited = vec![false; image.pixels.len()];
    let mut best: Option<(usize, Bounds)> = None;
    let mut largest_rejected: Option<(usize, Bounds, f64)> = None;
    let mut stack = Vec::new();
    for start in 0..image.pixels.len() {
        if visited[start] || !matches(&image.pixels[start]) {
            continue;
        }
        visited[start] = true;
        stack.push(start);
        let (mut count, mut min_x, mut min_y, mut max_x, mut max_y) =
            (0_usize, usize::MAX, usize::MAX, 0_usize, 0_usize);
        while let Some(index) = stack.pop() {
            let (x, y) = (index % image.width, index / image.width);
            count += 1;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            let mut visit = |neighbor: usize| {
                if !visited[neighbor] && matches(&image.pixels[neighbor]) {
                    visited[neighbor] = true;
                    stack.push(neighbor);
                }
            };
            if x > 0 {
                visit(index - 1);
            }
            if x + 1 < image.width {
                visit(index + 1);
            }
            if y > 0 {
                visit(index - image.width);
            }
            if y + 1 < image.height {
                visit(index + image.width);
            }
        }
        let bounds = (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1);
        let fill = count as f64 / (bounds.2 * bounds.3) as f64;
        if count >= MIN_PIXELS && fill >= MIN_FILL {
            if best.is_none_or(|(size, _)| count > size) {
                best = Some((count, bounds));
            }
        } else if largest_rejected.is_none_or(|(size, _, _)| count > size) {
            largest_rejected = Some((count, bounds, fill));
        }
    }
    best.map(|(_, bounds)| bounds)
        .ok_or_else(|| match largest_rejected {
            Some((count, (x, y, w, h), fill)) => format!(
                "largest near-color component had {count} px at ({x},{y}) {w}x{h}, fill {fill:.2}"
            ),
            None => "no near-color pixels".to_owned(),
        })
}

// ---------------------------------------------------------------- PNG

fn decode_png(bytes: &[u8]) -> Result<Rgb, String> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return Err("image is not a PNG".to_owned());
    }
    let mut offset = 8;
    let mut header = None;
    let mut compressed = Vec::new();
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let data = bytes
            .get(offset + 8..offset + 8 + length)
            .ok_or("truncated PNG chunk")?;
        match kind {
            b"IHDR" if data.len() == 13 => header = Some(data.to_vec()),
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        offset += 12 + length;
    }
    let header = header.ok_or("PNG has no IHDR")?;
    let width = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
    let (depth, color, interlace) = (header[8], header[9], header[12]);
    let channels = match (depth, color, interlace) {
        (8, 2, 0) => 3,
        (8, 6, 0) => 4,
        _ => {
            return Err(format!(
                "unsupported PNG format depth={depth} color={color} interlace={interlace}"
            ))
        }
    };
    if width == 0 || height == 0 || compressed.len() < 2 || compressed[0] & 0x0f != 8 {
        return Err("PNG has no zlib image data".to_owned());
    }
    let raw = inflate(&compressed[2..])?;
    let stride = width * channels;
    if raw.len() < height * (stride + 1) {
        return Err("PNG image data is truncated".to_owned());
    }
    let mut previous = vec![0_u8; stride];
    let mut current = vec![0_u8; stride];
    let mut pixels = Vec::with_capacity(width * height);
    for row in 0..height {
        let line = &raw[row * (stride + 1)..(row + 1) * (stride + 1)];
        let filter = line[0];
        for i in 0..stride {
            let a = if i >= channels {
                current[i - channels]
            } else {
                0
            };
            let b = previous[i];
            let c = if i >= channels {
                previous[i - channels]
            } else {
                0
            };
            let predictor = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                other => return Err(format!("unsupported PNG filter {other}")),
            };
            current[i] = line[i + 1].wrapping_add(predictor);
        }
        pixels.extend(
            current
                .chunks_exact(channels)
                .map(|pixel| [pixel[0], pixel[1], pixel[2]]),
        );
        std::mem::swap(&mut previous, &mut current);
    }
    Ok(Rgb {
        width,
        height,
        pixels,
    })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = (
        (p - a as i16).abs(),
        (p - b as i16).abs(),
        (p - c as i16).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// ---------------------------------------------------------------- inflate (RFC 1951)

struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
    buffer: u32,
    count: u32,
}

impl BitReader<'_> {
    fn bits(&mut self, need: u32) -> Result<u32, String> {
        while self.count < need {
            let byte = *self
                .data
                .get(self.position)
                .ok_or("deflate stream is truncated")?;
            self.position += 1;
            self.buffer |= (byte as u32) << self.count;
            self.count += 8;
        }
        let value = self.buffer & ((1_u32 << need) - 1);
        self.buffer >>= need;
        self.count -= need;
        Ok(value)
    }

    fn decode(&mut self, table: &Huffman) -> Result<usize, String> {
        let (mut code, mut first, mut index) = (0_i32, 0_i32, 0_i32);
        for length in 1..16 {
            code |= self.bits(1)? as i32;
            let count = table.counts[length] as i32;
            if code - count < first {
                return Ok(table.symbols[(index + code - first) as usize] as usize);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err("invalid deflate Huffman code".to_owned())
    }
}

struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Self {
        let mut counts = [0_u16; 16];
        for &length in lengths {
            counts[length as usize] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0_u16; 16];
        for length in 1..15 {
            offsets[length + 1] = offsets[length] + counts[length];
        }
        let mut symbols = vec![0_u16; lengths.len()];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                symbols[offsets[length as usize] as usize] = symbol as u16;
                offsets[length as usize] += 1;
            }
        }
        Self { counts, symbols }
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = BitReader {
        data,
        position: 0,
        buffer: 0,
        count: 0,
    };
    let mut output = Vec::new();
    loop {
        let last = input.bits(1)? == 1;
        match input.bits(2)? {
            0 => {
                input.buffer = 0;
                input.count = 0;
                let header = data
                    .get(input.position..input.position + 4)
                    .ok_or("stored deflate block is truncated")?;
                let length = u16::from_le_bytes([header[0], header[1]]) as usize;
                if length != !u16::from_le_bytes([header[2], header[3]]) as usize {
                    return Err("stored deflate block length is corrupt".to_owned());
                }
                input.position += 4;
                let block = data
                    .get(input.position..input.position + length)
                    .ok_or("stored deflate block is truncated")?;
                output.extend_from_slice(block);
                input.position += length;
            }
            1 => {
                let mut lengths = [0_u8; 288];
                lengths[..144].fill(8);
                lengths[144..256].fill(9);
                lengths[256..280].fill(7);
                lengths[280..].fill(8);
                let literals = Huffman::new(&lengths);
                let distances = Huffman::new(&[5_u8; 30]);
                inflate_block(&mut input, &mut output, &literals, &distances)?;
            }
            2 => {
                let (literals, distances) = dynamic_tables(&mut input)?;
                inflate_block(&mut input, &mut output, &literals, &distances)?;
            }
            _ => return Err("invalid deflate block type".to_owned()),
        }
        if last {
            return Ok(output);
        }
    }
}

fn dynamic_tables(input: &mut BitReader<'_>) -> Result<(Huffman, Huffman), String> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let literal_count = input.bits(5)? as usize + 257;
    let distance_count = input.bits(5)? as usize + 1;
    let code_count = input.bits(4)? as usize + 4;
    let mut code_lengths = [0_u8; 19];
    for &position in ORDER.iter().take(code_count) {
        code_lengths[position] = input.bits(3)? as u8;
    }
    let codes = Huffman::new(&code_lengths);
    let mut lengths = vec![0_u8; literal_count + distance_count];
    let mut index = 0;
    while index < lengths.len() {
        let symbol = input.decode(&codes)?;
        let (value, repeat) = match symbol {
            0..=15 => (symbol as u8, 1),
            16 => {
                let previous = *lengths
                    .get(index.wrapping_sub(1))
                    .ok_or("deflate repeat has no previous length")?;
                (previous, 3 + input.bits(2)? as usize)
            }
            17 => (0, 3 + input.bits(3)? as usize),
            _ => (0, 11 + input.bits(7)? as usize),
        };
        if index + repeat > lengths.len() {
            return Err("deflate code lengths overflow".to_owned());
        }
        lengths[index..index + repeat].fill(value);
        index += repeat;
    }
    Ok((
        Huffman::new(&lengths[..literal_count]),
        Huffman::new(&lengths[literal_count..]),
    ))
}

fn inflate_block(
    input: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    literals: &Huffman,
    distances: &Huffman,
) -> Result<(), String> {
    loop {
        let symbol = input.decode(literals)?;
        match symbol {
            0..=255 => output.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let code = symbol - 257;
                let length =
                    LENGTH_BASE[code] as usize + input.bits(LENGTH_EXTRA[code] as u32)? as usize;
                let code = input.decode(distances)?;
                if code >= 30 {
                    return Err("invalid deflate distance code".to_owned());
                }
                let distance = DISTANCE_BASE[code] as usize
                    + input.bits(DISTANCE_EXTRA[code] as u32)? as usize;
                if distance > output.len() {
                    return Err("deflate distance is too far back".to_owned());
                }
                let start = output.len() - distance;
                for offset in 0..length {
                    let byte = output[start + offset];
                    output.push(byte);
                }
            }
            _ => return Err("invalid deflate literal/length code".to_owned()),
        }
    }
}

// ---------------------------------------------------------------- base64, SHA-256

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(text.len() / 4 * 3);
    let (mut buffer, mut bits) = (0_u32, 0_u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return Err("image data is not base64".to_owned()),
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Ok(output)
}

fn sha256_hex(message: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64) * 8).to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}
