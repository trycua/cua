// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Result schema, markdown summary and the budget gate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Results {
    pub schema: u32,
    pub created_unix: u64,
    pub git_sha: String,
    pub host: BTreeMap<String, String>,
    pub image: String,
    pub seconds_per_run: u64,
    pub runs: Vec<Run>,
    /// Lanes not run, with the reason (missing toolchain, deferred hardware).
    pub skipped: Vec<Skip>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Run {
    /// `runtime/transport/decoder/client/scenario`.
    pub id: String,
    pub runtime: String,
    pub transport: String,
    pub decoder: String,
    pub client: String,
    pub scenario: String,
    pub target: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Server encoder as reported by the driver's stats.
    #[serde(default)]
    pub encoder: String,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skip {
    pub lane: String,
    pub reason: String,
}

/// Percentile summary helpers.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = (p / 100.0 * (v.len() - 1) as f64).round() as usize;
    Some(v[rank.min(v.len() - 1)])
}

pub fn put_dist(metrics: &mut BTreeMap<String, f64>, name: &str, values: &[f64]) {
    if values.is_empty() {
        return;
    }
    metrics.insert(format!("{name}_n"), values.len() as f64);
    for (label, p) in [("p50", 50.0), ("p95", 95.0)] {
        if let Some(v) = percentile(values, p) {
            metrics.insert(format!("{name}_{label}"), round3(v));
        }
    }
    let max = values.iter().cloned().fold(f64::MIN, f64::max);
    let min = values.iter().cloned().fold(f64::MAX, f64::min);
    metrics.insert(format!("{name}_max"), round3(max));
    metrics.insert(format!("{name}_min"), round3(min));
}

pub fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn fmt(metrics: &BTreeMap<String, f64>, key: &str, digits: usize) -> String {
    metrics
        .get(key)
        .map(|v| format!("{v:.digits$}"))
        .unwrap_or_else(|| "–".into())
}

pub fn markdown(results: &Results) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Streaming benchmark\n\n- git: `{}`\n- host: {}\n- image: `{}`\n- {} s per run\n\n",
        results.git_sha,
        results
            .host
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", "),
        results.image,
        results.seconds_per_run
    ));
    out.push_str("| run | enc | TTFF ms | fps | int p50/p95 ms | g2g p50/p95 ms | in→photon p50 ms | video KB/s | cli CPU % | srv CPU % | notes |\n");
    out.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for run in &results.runs {
        let m = &run.metrics;
        let mut notes = Vec::new();
        if let Some(v) = m.get("recovery_ms_p50") {
            notes.push(format!(
                "recovery p50 {v:.0} ms (max {})",
                fmt(m, "recovery_ms_max", 0)
            ));
        }
        if let Some(v) = m.get("datagrams_dropped") {
            notes.push(format!("dropped {v:.0} dgrams"));
        }
        if let Some(v) = m.get("av_skew_ms_p50") {
            notes.push(format!(
                "A/V skew p50 {v:.1} ms (max {})",
                fmt(m, "av_skew_abs_ms_max", 1)
            ));
        }
        if let Some(v) = m.get("audio_latency_ms_p50") {
            notes.push(format!("audio lat p50 {v:.0} ms"));
        }
        if let Some(v) = m.get("decode_us_p50") {
            notes.push(format!("decode p50 {:.1} ms", v / 1000.0));
        }
        if let Some(v) = m.get("container_cpu_pct") {
            notes.push(format!("container CPU {v:.0}%"));
        }
        if let Some(e) = &run.error {
            notes.push(format!("error: {e}"));
        }
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {}/{} | {}/{} | {} | {} | {} | {} | {} |\n",
            run.id,
            if run.encoder.is_empty() {
                "–"
            } else {
                &run.encoder
            },
            fmt(m, "ttff_ms", 0),
            fmt(m, "fps", 1),
            fmt(m, "interval_ms_p50", 1),
            fmt(m, "interval_ms_p95", 1),
            fmt(m, "g2g_ms_p50", 1),
            fmt(m, "g2g_ms_p95", 1),
            fmt(m, "input_photon_ms_p50", 1),
            m.get("video_bytes_per_s")
                .map(|v| format!("{:.1}", v / 1024.0))
                .unwrap_or_else(|| "–".into()),
            fmt(m, "client_cpu_pct", 1),
            fmt(m, "server_cpu_pct", 1),
            notes.join("; ")
        ));
    }
    if !results.skipped.is_empty() {
        out.push_str("\n## Not run\n\n");
        for skip in &results.skipped {
            out.push_str(&format!("- `{}`: {}\n", skip.lane, skip.reason));
        }
    }
    out
}

// ------------------------------------------------------------------ budgets

#[derive(Debug, Clone, Deserialize)]
pub struct Budgets {
    pub budgets: Vec<Budget>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Budget {
    /// Glob over run ids (`*` matches within and across segments).
    #[serde(rename = "match")]
    pub pattern: String,
    pub metric: String,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub min: Option<f64>,
    /// Missing metric (or no matching run) is not a failure.
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub note: String,
}

pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == t[ti] || p[pi] == '?') {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Returns (checked, failures).
pub fn check(results: &Results, budgets: &Budgets) -> (usize, Vec<String>) {
    let mut failures = Vec::new();
    let mut checked = 0;
    for budget in &budgets.budgets {
        let runs: Vec<&Run> = results
            .runs
            .iter()
            .filter(|r| glob(&budget.pattern, &r.id))
            .collect();
        if runs.is_empty() {
            if !budget.optional {
                failures.push(format!(
                    "{} {}: no matching run",
                    budget.pattern, budget.metric
                ));
            }
            continue;
        }
        for run in runs {
            if !run.ok {
                if !budget.optional {
                    failures.push(format!(
                        "{}: run failed ({})",
                        run.id,
                        run.error.clone().unwrap_or_default()
                    ));
                }
                continue;
            }
            let Some(value) = run.metrics.get(&budget.metric) else {
                if !budget.optional {
                    failures.push(format!("{} {}: metric missing", run.id, budget.metric));
                }
                continue;
            };
            checked += 1;
            if let Some(max) = budget.max {
                if *value > max {
                    failures.push(format!(
                        "{} {} = {value} > max {max} {}",
                        run.id, budget.metric, budget.note
                    ));
                }
            }
            if let Some(min) = budget.min {
                if *value < min {
                    failures.push(format!(
                        "{} {} = {value} < min {min} {}",
                        run.id, budget.metric, budget.note
                    ));
                }
            }
        }
    }
    (checked, failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_ids() {
        assert!(glob(
            "runc/*/rust/timecode",
            "runc/ws/videotoolbox/rust/timecode"
        ));
        assert!(glob("*/quic/*", "runsc/quic/openh264/rust/loss"));
        assert!(!glob("runc/ws/*", "runsc/ws/openh264/rust/static"));
        assert!(glob("*", ""));
    }

    #[test]
    fn percentiles_and_budget_gate() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), Some(51.0));
        assert_eq!(percentile(&v, 95.0), Some(95.0));
        let mut run = Run {
            id: "runc/ws/x/rust/timecode".into(),
            ok: true,
            ..Default::default()
        };
        run.metrics.insert("g2g_ms_p50".into(), 80.0);
        let results = Results {
            runs: vec![run],
            ..Default::default()
        };
        let budgets: Budgets = serde_json::from_str(
            r#"{"budgets":[{"match":"*/timecode","metric":"g2g_ms_p50","max":100},
                           {"match":"*/timecode","metric":"fps","min":10},
                           {"match":"*/nothing","metric":"fps","min":10,"optional":true}]}"#,
        )
        .unwrap();
        let (checked, failures) = check(&results, &budgets);
        assert_eq!(checked, 1);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("fps: metric missing"));
    }
}
