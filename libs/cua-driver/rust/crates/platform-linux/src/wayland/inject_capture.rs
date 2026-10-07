//! Foreground-only capture attested by the nested compositor. No focus changes.
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Attestation {
    requested_pid: u32,
    owner_pid: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    epoch: u64,
    title: String,
}

impl Attestation {
    fn parse(line: &str) -> Result<Self> {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() != 9 || f[0] != "capture" {
            bail!("nested compositor refused capture: {}", line.trim());
        }
        let hex = f[8];
        if hex.is_empty() || hex.len() > 1024 || hex.len() % 2 != 0 || !hex.is_ascii() {
            bail!("invalid capture title encoding");
        }
        let title = String::from_utf8(
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<std::result::Result<Vec<_>, _>>()?,
        )?;
        let result = Self {
            requested_pid: f[1].parse()?,
            owner_pid: f[2].parse()?,
            x: f[3].parse()?,
            y: f[4].parse()?,
            width: f[5].parse()?,
            height: f[6].parse()?,
            epoch: f[7].parse()?,
            title,
        };
        if result.requested_pid == 0
            || result.owner_pid == 0
            || result.epoch == 0
            || result.x < 0
            || result.y < 0
            || result.width == 0
            || result.height == 0
            || result.width > 16_384
            || result.height > 16_384
        {
            bail!("invalid compositor capture geometry or identity");
        }
        Ok(result)
    }

    fn binds(&self, pid: u32, title: &str) -> Result<()> {
        if self.requested_pid != pid || title.is_empty() || self.title != title {
            bail!("compositor capture does not match the freshly listed target");
        }
        Ok(())
    }
}

fn verify_stable(before: &Attestation, after: &Attestation) -> Result<()> {
    if before != after {
        bail!("capture identity, geometry or compositor epoch changed during capture");
    }
    Ok(())
}

fn verify_bounds(target: &Attestation, width: u32, height: u32) -> Result<()> {
    if target.x as u64 + target.width as u64 > width as u64
        || target.y as u64 + target.height as u64 > height as u64
    {
        bail!("attested target is not wholly inside the captured output");
    }
    Ok(())
}

fn query(pid: u32) -> Result<Attestation> {
    let replies = super::inject_exchange(&[format!("c {pid}")])?;
    Attestation::parse(replies.first().context("missing capture attestation")?)
}

fn fresh_title(pid: u32, window_id: u64) -> Result<String> {
    if !super::window_was_listed_for_pid(pid, window_id) {
        bail!("capture target was not listed for the current process instance");
    }
    super::list_windows_dispatch(Some(pid))
        .into_iter()
        .find(|w| w.xid == window_id && w.pid == Some(pid))
        .map(|w| super::undecorated_native_title(&w).to_owned())
        .context("capture target disappeared from fresh window enumeration")
}

fn unique_pid(window_id: u64, windows: &[super::WindowInfo]) -> Result<u32> {
    let mut matches = windows.iter().filter(|w| w.xid == window_id);
    let pid = matches
        .next()
        .and_then(|w| w.pid)
        .filter(|pid| *pid > 0)
        .context("fresh window enumeration did not bind a process")?;
    if matches.next().is_some() {
        bail!("fresh window enumeration returned an ambiguous window identity");
    }
    Ok(pid)
}

pub(super) fn screenshot(window_id: u64, requested_pid: Option<u32>) -> Result<Vec<u8>> {
    // ID-only callers (including zoom) still need a fresh, unique process binding.
    let pid = match requested_pid {
        Some(pid) => pid,
        None => unique_pid(window_id, &super::list_windows_dispatch(None))?,
    };
    let title = fresh_title(pid, window_id)?;
    let before = query(pid)?;
    before.binds(pid, &title)?;
    // Stay on the nested Wayland connection: portal/X11 fallbacks may capture
    // another desktop and cannot be bound by this compositor attestation.
    let pixels = super::screenshot_bytes()?;
    let after = query(pid)?;
    after.binds(pid, &fresh_title(pid, window_id)?)?;
    verify_stable(&before, &after)?;
    let (width, height) = crate::capture::png_dimensions_pub(&pixels)?;
    verify_bounds(&before, width, height)?;
    super::crop_png_to_rect(
        &pixels,
        before.x,
        before.y,
        before.width,
        before.height,
        "nested compositor target",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const VALID: &str = "capture 12 14 0 0 640 480 9 546172676574";

    #[test]
    fn id_only_capture_requires_a_unique_fresh_process_binding() {
        let window = |id, pid| super::super::WindowInfo {
            xid: id,
            pid,
            app_name: String::new(),
            title: "Target".into(),
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        };
        assert_eq!(unique_pid(9, &[window(9, Some(12))]).unwrap(), 12);
        assert!(unique_pid(9, &[window(8, Some(12))]).is_err());
        assert!(unique_pid(9, &[window(9, None)]).is_err());
        assert!(unique_pid(9, &[window(9, Some(0))]).is_err());
        assert!(unique_pid(9, &[window(9, Some(12)), window(9, Some(13))]).is_err());
    }

    #[test]
    fn exact_target_requires_pid_and_full_title() {
        let a = Attestation::parse(VALID).unwrap();
        assert!(a.binds(12, "Target").is_ok());
        assert!(a.binds(13, "Target").is_err());
        assert!(a.binds(12, "Other record").is_err());
        assert!(a.binds(12, "").is_err());
    }

    #[test]
    fn refuses_occlusion_ambiguity_invalid_geometry_and_malformed_titles() {
        for line in [
            "err target-occluded",
            "err ambiguous-pid",
            "capture 12 14 -1 0 640 480 9 546172676574",
            "capture 12 14 0 0 0 480 9 546172676574",
            "capture 12 14 0 0 640 480 0 546172676574",
            "capture 12 14 0 0 640 480 9 ff",
            "capture 12 14 0 0 640 480 9 0",
            "capture 12 14 0 0 640 480 9 gg",
        ] {
            assert!(Attestation::parse(line).is_err(), "accepted {line}");
        }
    }

    #[test]
    fn refuses_partial_output_crop() {
        let target = Attestation::parse(VALID).unwrap();
        assert!(verify_bounds(&target, 640, 480).is_ok());
        assert!(verify_bounds(&target, 639, 480).is_err());
        assert!(verify_bounds(&target, 640, 479).is_err());
    }

    #[test]
    fn epoch_detects_focus_or_lifecycle_aba_even_when_geometry_matches() {
        let before = Attestation::parse(VALID).unwrap();
        assert!(verify_stable(&before, &before).is_ok());
        let mut after = before.clone();
        after.epoch += 1;
        assert!(verify_stable(&before, &after).is_err());
        after = before.clone();
        after.owner_pid += 1;
        assert!(verify_stable(&before, &after).is_err());
    }
}
