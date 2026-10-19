// Generated from ../typescript-node/pixels.ts (typescript transpileModule);
// keep the two in sync.
// Pure helpers shared by the scenario: FNV-1a 64, the bench timecode reader,
// the WAV writer and the client wall clock. No SDK imports, so they can be
// unit-tested without the native library (`node --test pixels.test.ts`).
/** FNV-1a 64 over `bytes`, as 16 lowercase hex digits. */
export function fnv1a64(bytes) {
    // 64-bit state as two u32 halves; prime = 2^40 + 0x1b3.
    let hi = 0xcbf29ce4;
    let lo = 0x84222325;
    for (let i = 0; i < bytes.length; i++) {
        lo = (lo ^ bytes[i]) >>> 0;
        const loMul = lo * 0x1b3; // < 2^41, exact in a double
        const carry = Math.floor(loMul / 4294967296);
        const hiMul = hi * 0x1b3 + carry + ((lo << 8) >>> 0);
        lo = loMul >>> 0;
        hi = hiMul % 4294967296;
    }
    return hi.toString(16).padStart(8, "0") + lo.toString(16).padStart(8, "0");
}
/** Client wall clock in unix nanoseconds (sub-microsecond resolution). */
export function unixNs() {
    const ms = performance.timeOrigin + performance.now();
    return BigInt(Math.floor(ms * 1000)) * 1000n;
}
export const BGRA = { r: 2, g: 1, b: 0 };
export const RGBA = { r: 0, g: 1, b: 2 };
/** The pixel at (x, y) of a packed 4-byte-per-pixel image, as [r, g, b]. */
export function pixelAt(data, width, stride, x, y, layout) {
    const px = Math.max(0, Math.min(width - 1, Math.round(x)));
    const i = Math.round(y) * stride + px * 4;
    return [data[i + layout.r], data[i + layout.g], data[i + layout.b]];
}
const CELL = 16;
const CELLS = 48;
/**
 * Reads the bench timecode strip (SCENARIO.md "Bench timecode") at frame
 * pixel (0, 0) with `scale` = frame width / window content width. Returns
 * the full unix-ms value rebuilt around `clientUnixMs`, or null when either
 * sync pattern or the checksum does not match.
 */
export function readTimecode(data, width, height, stride, scale, layout, clientUnixMs) {
    if (width < Math.ceil(CELL * CELLS * scale) || height < Math.ceil(10 * scale))
        return null;
    const bits = [];
    for (let c = 0; c < CELLS; c++) {
        let sum = 0;
        for (let dy = 6; dy <= 9; dy++) {
            const y = Math.min(height - 1, Math.round(dy * scale));
            for (let dx = 6; dx <= 9; dx++) {
                const x = Math.min(width - 1, Math.round((CELL * c + dx) * scale));
                const i = y * stride + x * 4;
                sum += data[i + layout.r] + data[i + layout.g] + data[i + layout.b];
            }
        }
        bits.push(sum / 48 > 128 ? 1 : 0);
    }
    const syncA = [1, 0, 1, 0];
    const syncB = [0, 1, 0, 1];
    for (let i = 0; i < 4; i++) {
        if (bits[i] !== syncA[i] || bits[44 + i] !== syncB[i])
            return null;
    }
    let value = 0;
    for (let i = 4; i < 36; i++)
        value = value * 2 + bits[i];
    let checksum = 0;
    for (let i = 36; i < 44; i++)
        checksum = checksum * 2 + bits[i];
    const xor = ((value >>> 24) ^ (value >>> 16) ^ (value >>> 8) ^ value) & 0xff;
    if (xor !== checksum)
        return null;
    const window = 4294967296;
    const base = clientUnixMs - (clientUnixMs % window);
    let best = base + value;
    for (const candidate of [base + value - window, base + value + window]) {
        if (Math.abs(candidate - clientUnixMs) < Math.abs(best - clientUnixMs))
            best = candidate;
    }
    return best;
}
/** A 16-bit PCM WAV file (little endian) from interleaved samples. */
export function wavBytes(chunks, sampleRate, channels) {
    const total = chunks.reduce((n, c) => n + c.length, 0);
    const dataBytes = total * 2;
    const out = new Uint8Array(44 + dataBytes);
    const v = new DataView(out.buffer);
    const ascii = (at, s) => {
        for (let i = 0; i < s.length; i++)
            out[at + i] = s.charCodeAt(i);
    };
    ascii(0, "RIFF");
    v.setUint32(4, 36 + dataBytes, true);
    ascii(8, "WAVE");
    ascii(12, "fmt ");
    v.setUint32(16, 16, true);
    v.setUint16(20, 1, true);
    v.setUint16(22, channels, true);
    v.setUint32(24, sampleRate, true);
    v.setUint32(28, sampleRate * channels * 2, true);
    v.setUint16(32, channels * 2, true);
    v.setUint16(34, 16, true);
    ascii(36, "data");
    v.setUint32(40, dataBytes, true);
    let at = 44;
    for (const chunk of chunks) {
        for (let i = 0; i < chunk.length; i++, at += 2)
            v.setInt16(at, chunk[i], true);
    }
    return out;
}
