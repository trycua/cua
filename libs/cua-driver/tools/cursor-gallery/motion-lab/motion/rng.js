// Seeded randomness. Every candidate draws from an Rng built from
// (seed, candidate id, segment index), so a plan is a pure function of its
// inputs and identical in the browser, in tests, and in a future Rust port.

export function hashString(text) {
  // FNV-1a, 32-bit.
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

export class Rng {
  constructor(seed) {
    this.state = (typeof seed === 'number' ? seed : hashString(String(seed))) >>> 0 || 1;
    this.spare = null;
  }

  // mulberry32
  next() {
    let t = (this.state = (this.state + 0x6d2b79f5) >>> 0);
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }

  range(lo, hi) {
    return lo + (hi - lo) * this.next();
  }

  sign() {
    return this.next() < 0.5 ? -1 : 1;
  }

  // Standard normal via Box-Muller.
  normal(mean = 0, sd = 1) {
    if (this.spare !== null) {
      const v = this.spare;
      this.spare = null;
      return mean + sd * v;
    }
    let u = 0;
    let v = 0;
    while (u <= Number.EPSILON) u = this.next();
    v = this.next();
    const mag = Math.sqrt(-2 * Math.log(u));
    this.spare = mag * Math.sin(2 * Math.PI * v);
    return mean + sd * mag * Math.cos(2 * Math.PI * v);
  }

  // Normal clipped to +/- k standard deviations, so tails never blow up a path.
  clippedNormal(mean, sd, k = 2.5) {
    const z = Math.max(-k, Math.min(k, this.normal()));
    return mean + sd * z;
  }

  fork(label) {
    return new Rng(hashString(`${this.state}:${label}`));
  }
}
