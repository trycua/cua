//! Where regions are and how far the requester is from them: the solver
//! prices round-trip time, because every exec of an agent or a rollout
//! pays it (a Space in Germany costs a US caller ~130 ms per call; one in
//! Oregon ~20 ms). Estimates come from coordinates and the speed of light
//! in fiber; measured RTTs to running hosts replace them.

/// Approximate coordinates (lat, lon) of a cloud region or location.
pub fn region_coords(provider: &str, region: &str) -> Option<(f64, f64)> {
    let r = region.to_ascii_lowercase();
    // GCP zones (`us-central1-a`) and AWS AZs (`us-west-2a`) name their region.
    let base = r
        .strip_suffix(|c: char| c.is_ascii_lowercase())
        .filter(|b| b.ends_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(&r)
        .trim_end_matches('-');
    let gcp_zone = r
        .rsplit_once('-')
        .map(|(b, z)| if z.len() == 1 { b } else { r.as_str() });
    let key = match provider {
        "gcp" => gcp_zone.unwrap_or(&r),
        _ => base,
    };
    Some(match (provider, key) {
        ("hetzner", "fsn1") => (50.47, 12.37),
        ("hetzner", "nbg1") => (49.45, 11.08),
        ("hetzner", "hel1") => (60.17, 24.94),
        ("hetzner", "ash") => (39.04, -77.49),
        ("hetzner", "hil") => (45.52, -122.99),
        ("hetzner", "sin") => (1.35, 103.82),
        (_, "us-east-1") | (_, "us-east4") => (38.9, -77.4),
        (_, "us-east-2") => (40.0, -83.0),
        (_, "us-west-1") => (37.4, -121.9),
        (_, "us-west-2") | (_, "us-west1") => (45.8, -119.7),
        (_, "us-central1") => (41.3, -95.9),
        (_, "us-east1") => (33.2, -80.0),
        (_, "eu-west-1") => (53.3, -6.3),
        (_, "eu-central-1") => (50.1, 8.7),
        (_, "europe-west1") => (50.4, 3.8),
        (_, "europe-west4") => (53.4, 6.8),
        (_, "ap-southeast-1") | (_, "asia-southeast1") => (1.35, 103.8),
        (_, "ap-northeast-1") | (_, "asia-northeast1") => (35.7, 139.7),
        ("modal", _) => (39.0, -77.5),
        _ => return None,
    })
}

/// Where this machine is, roughly: `CUA_LOCATION` (`lat,lon`), else its
/// time zone (`TZ`, else `/etc/localtime`).
pub fn here() -> Option<(f64, f64)> {
    if let Ok(v) = std::env::var("CUA_LOCATION")
        && let Some((a, b)) = v.split_once(',')
        && let (Ok(lat), Ok(lon)) = (a.trim().parse(), b.trim().parse())
    {
        return Some((lat, lon));
    }
    let tz = std::env::var("TZ")
        .ok()
        .map(|t| t.trim_start_matches(':').to_string())
        .or_else(|| {
            std::fs::read_link("/etc/localtime").ok().and_then(|p| {
                let s = p.to_string_lossy().to_string();
                s.split("zoneinfo/").nth(1).map(str::to_string)
            })
        })?;
    zone_coords(&tz)
}

/// Approximate coordinates of an IANA time zone.
pub fn zone_coords(tz: &str) -> Option<(f64, f64)> {
    Some(match tz {
        "America/Los_Angeles" | "US/Pacific" => (37.0, -121.0),
        "America/Vancouver" => (49.3, -123.1),
        "America/Denver" | "America/Phoenix" | "US/Mountain" => (39.7, -105.0),
        "America/Chicago" | "US/Central" => (41.9, -87.6),
        "America/New_York" | "America/Toronto" | "US/Eastern" => (40.7, -74.0),
        "America/Sao_Paulo" => (-23.5, -46.6),
        "Europe/London" | "Europe/Dublin" => (51.5, -0.1),
        "Europe/Berlin" | "Europe/Paris" | "Europe/Amsterdam" | "Europe/Zurich"
        | "Europe/Vienna" => (50.5, 7.0),
        "Europe/Helsinki" | "Europe/Stockholm" => (60.0, 24.0),
        "Asia/Singapore" => (1.35, 103.8),
        "Asia/Tokyo" => (35.7, 139.7),
        "Asia/Kolkata" => (19.0, 72.8),
        "Australia/Sydney" => (-33.9, 151.2),
        "UTC" | "Etc/UTC" => return None,
        t if t.starts_with("America/") => (39.0, -98.0),
        t if t.starts_with("Europe/") => (50.0, 10.0),
        t if t.starts_with("Asia/") => (20.0, 100.0),
        t if t.starts_with("Australia/") => (-30.0, 145.0),
        _ => return None,
    })
}

/// Estimated round trip between two points, ms: fiber is ~2/3 c and routes
/// are not straight, so ~70 km per ms of round trip, plus 5 ms of hops.
pub fn rtt_ms(a: (f64, f64), b: (f64, f64)) -> u32 {
    let (la1, lo1, la2, lo2) = (
        a.0.to_radians(),
        a.1.to_radians(),
        b.0.to_radians(),
        b.1.to_radians(),
    );
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    let km = 2.0 * 6371.0 * h.sqrt().asin();
    (km / 70.0 + 5.0).round() as u32
}

/// Estimated RTT from here to a region (`None`: unknown).
pub fn rtt_to(provider: &str, region: &str) -> Option<u32> {
    Some(rtt_ms(here()?, region_coords(provider, region)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_us_west_caller_is_near_oregon_and_far_from_germany() {
        let sf = zone_coords("America/Los_Angeles").unwrap();
        let hil = rtt_ms(sf, region_coords("hetzner", "hil").unwrap());
        let fsn = rtt_ms(sf, region_coords("hetzner", "fsn1").unwrap());
        let ash = rtt_ms(sf, region_coords("hetzner", "ash").unwrap());
        assert!(hil < 20, "{hil}");
        assert!((120..170).contains(&fsn), "{fsn}");
        assert!(hil < ash && ash < fsn);
        // Zones and AZs name their region.
        assert_eq!(
            region_coords("gcp", "us-west1-a"),
            region_coords("gcp", "us-west1")
        );
        assert_eq!(
            region_coords("aws", "us-west-2a"),
            region_coords("aws", "us-west-2")
        );
        assert!(region_coords("hetzner", "nowhere").is_none());
    }
}
