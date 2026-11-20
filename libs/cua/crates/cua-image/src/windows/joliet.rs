//! A small ISO 9660 + Joliet writer for the data disc that carries the
//! answer file. Windows Setup looks for `Autounattend.xml` by its long name on
//! every drive it can read, so the disc needs Joliet (UCS-2 names); the plain
//! ISO 9660 names are the 8.3 form beside them.
//!
//! One flat root directory; files only; deterministic (a fixed date).

const SECTOR: usize = 2048;

/// A file on the disc.
pub struct DiscFile<'a> {
    /// The long name (`Autounattend.xml`).
    pub name: &'a str,
    pub data: &'a [u8],
}

/// ISO 9660 level 1 name for `name`: upper case, 8.3, `;1`.
fn short_name(name: &str, taken: &[String]) -> String {
    let clean = |s: &str, max: usize| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .take(max)
            .collect()
    };
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    let mut n = 0;
    loop {
        let suffix = if n == 0 { String::new() } else { n.to_string() };
        let stem = clean(base, 8 - suffix.len().min(7)) + &suffix;
        let candidate = format!("{stem}.{};1", clean(ext, 3));
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn ucs2(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
}

fn both32(dst: &mut [u8], v: u32) {
    dst[..4].copy_from_slice(&v.to_le_bytes());
    dst[4..8].copy_from_slice(&v.to_be_bytes());
}

fn both16(dst: &mut [u8], v: u16) {
    dst[..2].copy_from_slice(&v.to_le_bytes());
    dst[2..4].copy_from_slice(&v.to_be_bytes());
}

fn record(ident: &[u8], lba: u32, len: u32, dir: bool) -> Vec<u8> {
    let base = 33 + ident.len();
    let rec_len = base + (base % 2);
    let mut r = vec![0u8; rec_len];
    r[0] = rec_len as u8;
    both32(&mut r[2..10], lba);
    both32(&mut r[10..18], len);
    // 2024-01-01 00:00:00, GMT offset 0: the disc is reproducible.
    r[18..25].copy_from_slice(&[124, 1, 1, 0, 0, 0, 0]);
    r[25] = if dir { 2 } else { 0 };
    both16(&mut r[28..32], 1);
    r[32] = ident.len() as u8;
    r[33..33 + ident.len()].copy_from_slice(ident);
    r
}

fn path_table(root_lba: u32, big: bool) -> Vec<u8> {
    let mut t = vec![1u8, 0];
    if big {
        t.extend(root_lba.to_be_bytes());
        t.extend(1u16.to_be_bytes());
    } else {
        t.extend(root_lba.to_le_bytes());
        t.extend(1u16.to_le_bytes());
    }
    t.extend([0, 0]);
    t
}

fn put(img: &mut [u8], off: usize, data: &[u8]) {
    img[off..off + data.len()].copy_from_slice(data);
}

/// Writes the disc. `volume` is the volume label (at most 16 characters).
pub fn write_disc(volume: &str, files: &[DiscFile<'_>]) -> Vec<u8> {
    // Fixed layout: 16 PVD, 17 Joliet SVD, 18 end; 19/20 and 21/22 the path
    // tables (ISO L/M, Joliet L/M); 23 ISO root; 24 Joliet root; files after.
    let (iso_root, jol_root) = (23u32, 24u32);
    let mut next = 25u32;
    let mut taken = Vec::new();
    let mut placed = Vec::new();
    for f in files {
        let short = short_name(f.name, &taken);
        taken.push(short.clone());
        let lba = next;
        next += f.data.len().div_ceil(SECTOR).max(1) as u32;
        placed.push((short, f.name, lba, f.data));
    }
    let total = next;
    let mut img = vec![0u8; total as usize * SECTOR];

    let mut iso_dir = Vec::new();
    iso_dir.extend(record(&[0], iso_root, SECTOR as u32, true));
    iso_dir.extend(record(&[1], iso_root, SECTOR as u32, true));
    let mut by_short: Vec<_> = placed.iter().collect();
    by_short.sort_by(|a, b| a.0.cmp(&b.0));
    for (short, _, lba, data) in by_short {
        iso_dir.extend(record(short.as_bytes(), *lba, data.len() as u32, false));
    }
    let mut jol_dir = Vec::new();
    jol_dir.extend(record(&[0], jol_root, SECTOR as u32, true));
    jol_dir.extend(record(&[1], jol_root, SECTOR as u32, true));
    let mut by_long: Vec<_> = placed.iter().collect();
    by_long.sort_by_key(|p| ucs2(p.1));
    for (_, long, lba, data) in by_long {
        jol_dir.extend(record(&ucs2(long), *lba, data.len() as u32, false));
    }
    assert!(
        iso_dir.len() <= SECTOR && jol_dir.len() <= SECTOR,
        "the data disc holds a handful of files"
    );
    put(&mut img, iso_root as usize * SECTOR, &iso_dir);
    put(&mut img, jol_root as usize * SECTOR, &jol_dir);
    for (_, _, lba, data) in &placed {
        put(&mut img, *lba as usize * SECTOR, data);
    }
    let (l_iso, m_iso) = (path_table(iso_root, false), path_table(iso_root, true));
    let (l_jol, m_jol) = (path_table(jol_root, false), path_table(jol_root, true));
    put(&mut img, 19 * SECTOR, &l_iso);
    put(&mut img, 20 * SECTOR, &m_iso);
    put(&mut img, 21 * SECTOR, &l_jol);
    put(&mut img, 22 * SECTOR, &m_jol);

    let descriptor = |joliet: bool| -> Vec<u8> {
        let mut d = vec![0u8; SECTOR];
        d[0] = if joliet { 2 } else { 1 };
        d[1..6].copy_from_slice(b"CD001");
        d[6] = 1;
        let (root, l, m) = if joliet {
            (jol_root, 21u32, 22u32)
        } else {
            (iso_root, 19, 20)
        };
        let text = |dst: &mut [u8], s: &str| {
            if joliet {
                dst.fill(0x20);
                for pair in dst.chunks_exact_mut(2) {
                    pair.copy_from_slice(&[0, 0x20]);
                }
                let u = ucs2(s);
                let n = u.len().min(dst.len());
                dst[..n].copy_from_slice(&u[..n]);
            } else {
                dst.fill(b' ');
                let n = s.len().min(dst.len());
                dst[..n].copy_from_slice(&s.as_bytes()[..n]);
            }
        };
        text(&mut d[8..40], "");
        text(&mut d[40..72], volume);
        both32(&mut d[80..88], total);
        if joliet {
            d[88..91].copy_from_slice(&[0x25, 0x2F, 0x45]);
        }
        both16(&mut d[120..124], 1);
        both16(&mut d[124..128], 1);
        both16(&mut d[128..132], SECTOR as u16);
        both32(&mut d[132..140], 10);
        d[140..144].copy_from_slice(&l.to_le_bytes());
        d[148..152].copy_from_slice(&m.to_be_bytes());
        d[156..190].copy_from_slice(&record(&[0], root, SECTOR as u32, true));
        for range in [
            190..318,
            318..446,
            446..574,
            574..702,
            702..739,
            739..776,
            776..813,
        ] {
            text(&mut d[range], "");
        }
        for off in [813, 830, 847, 864] {
            d[off..off + 16].copy_from_slice(b"0000000000000000");
            d[off + 16] = 0;
        }
        d[881] = 1;
        d
    };
    put(&mut img, 16 * SECTOR, &descriptor(false));
    put(&mut img, 17 * SECTOR, &descriptor(true));
    let mut end = vec![0u8; 7];
    end[0] = 255;
    end[1..6].copy_from_slice(b"CD001");
    end[6] = 1;
    put(&mut img, 18 * SECTOR, &end);
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the files of the root directory at descriptor `sector`
    /// (16: ISO 9660 names, 17: Joliet names).
    fn read(img: &[u8], sector: usize) -> Vec<(String, Vec<u8>)> {
        let d = &img[sector * SECTOR..(sector + 1) * SECTOR];
        assert_eq!(&d[1..6], b"CD001");
        let joliet = d[0] == 2;
        let root = u32::from_le_bytes(d[158..162].try_into().unwrap()) as usize;
        let dir = &img[root * SECTOR..(root + 1) * SECTOR];
        let mut out = Vec::new();
        let mut at = 0;
        while at < SECTOR && dir[at] != 0 {
            let len = dir[at] as usize;
            let rec = &dir[at..at + len];
            at += len;
            let id = &rec[33..33 + rec[32] as usize];
            if id == [0] || id == [1] {
                continue;
            }
            let lba = u32::from_le_bytes(rec[2..6].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(rec[10..14].try_into().unwrap()) as usize;
            let name = if joliet {
                let u: Vec<u16> = id
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16(&u).unwrap()
            } else {
                String::from_utf8(id.to_vec()).unwrap()
            };
            out.push((name, img[lba * SECTOR..lba * SECTOR + size].to_vec()));
        }
        out
    }

    #[test]
    fn long_names_and_contents_survive_in_joliet() {
        let big = vec![7u8; 5000];
        let disc = write_disc(
            "OEMDRV",
            &[
                DiscFile {
                    name: "setup-spacesd.ps1",
                    data: b"Write-Host hi",
                },
                DiscFile {
                    name: "Autounattend.xml",
                    data: &big,
                },
            ],
        );
        assert_eq!(disc.len() % SECTOR, 0);
        let j = read(&disc, 17);
        let names: Vec<_> = j.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Autounattend.xml", "setup-spacesd.ps1"]);
        assert_eq!(j[0].1, big);
        assert_eq!(j[1].1, b"Write-Host hi");
    }

    #[test]
    fn the_plain_names_are_unique_8_3() {
        let disc = write_disc(
            "X",
            &[
                DiscFile {
                    name: "setup-spacesd.ps1",
                    data: b"a",
                },
                DiscFile {
                    name: "setup-spacesd.psd",
                    data: b"b",
                },
                DiscFile {
                    name: "startup.nsh",
                    data: b"c",
                },
            ],
        );
        let names: Vec<_> = read(&disc, 16).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names.len(), 3);
        for n in &names {
            let (stem, rest) = n.split_once('.').unwrap();
            assert!(stem.len() <= 8 && rest.ends_with(";1"), "{n}");
        }
        let set: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn the_joliet_descriptor_is_announced_and_the_disc_is_deterministic() {
        let a = write_disc(
            "OEMDRV",
            &[DiscFile {
                name: "a.txt",
                data: b"x",
            }],
        );
        let b = write_disc(
            "OEMDRV",
            &[DiscFile {
                name: "a.txt",
                data: b"x",
            }],
        );
        assert_eq!(a, b);
        let svd = &a[17 * SECTOR..];
        assert_eq!(svd[0], 2);
        assert_eq!(&svd[88..91], &[0x25, 0x2F, 0x45], "UCS-2 level 3 escape");
        assert_eq!(a[18 * SECTOR], 255, "terminator follows");
    }
}
