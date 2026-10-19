//! `cua skills`: recorded demonstrations that guide agents, stored under
//! `~/.cua/skills/<name>/` as `SKILL.md` (front matter `name`/`description`)
//! plus `trajectory/` (video, `events.json`, `trajectory.json`, frames).
//!
//! `record` serves a loopback WebSocket, opens the HTML5 viewer in recording
//! mode (`autorecord=true&record_format=mp4&record_url=ws://…`), receives
//! `[u32 BE json length][json {events, metadata}][mp4]`, extracts one frame
//! per event with `ffmpeg` and captions it with a vision model.

use crate::util::{self, internal, line};
use cua_sdk::CuaError;
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

/// Upper bound on a received recording.
const MAX_RECORDING_BYTES: usize = 1 << 30;

/// `~/.cua/skills`.
pub fn dir() -> PathBuf {
    util::cua_home().join("skills")
}

/// `name`, `description` and body of a SKILL.md, when well-formed.
pub fn parse_front_matter(content: &str) -> Option<(String, String, String)> {
    let rest = content.strip_prefix("---\n")?;
    let (fm, body) = rest.split_once("\n---\n")?;
    let get = |k: &str| {
        fm.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}:")))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    Some((get("name")?, get("description")?, body.trim().to_string()))
}

/// Summary of one skill.
pub fn info(skill_dir: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    let (name, description, _) = parse_front_matter(&content)?;
    let traj: serde_json::Value =
        std::fs::read_to_string(skill_dir.join("trajectory/trajectory.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    Some(serde_json::json!({
        "name": name,
        "description": description,
        "steps": traj["trajectory"].as_array().map(|a| a.len()).unwrap_or(0),
        "created": traj["metadata"]["created_at"],
        "path": skill_dir.display().to_string(),
    }))
}

/// All skills, sorted by directory name.
pub fn list() -> Vec<serde_json::Value> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.iter().filter_map(|d| info(d)).collect()
}

/// A skill directory by name (no path traversal).
pub fn skill_path(name: &str) -> Result<PathBuf, CuaError> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(CuaError::InvalidArgument(format!(
            "bad skill name {name:?}"
        )));
    }
    Ok(dir().join(name))
}

/// A skill as JSON (`read --format json` and the MCP `skills_read`).
pub fn read_json(name: &str) -> Result<serde_json::Value, CuaError> {
    let d = skill_path(name)?;
    let content = std::fs::read_to_string(d.join("SKILL.md"))
        .map_err(|_| CuaError::NotFound(format!("skill not found: {name}")))?;
    let (n, desc, body) = parse_front_matter(&content)
        .ok_or_else(|| CuaError::InvalidArgument(format!("invalid skill file format: {name}")))?;
    let traj: serde_json::Value = std::fs::read_to_string(d.join("trajectory/trajectory.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Ok(serde_json::json!({
        "name": n,
        "description": desc,
        "trajectory": traj.get("trajectory").cloned().unwrap_or(serde_json::json!([])),
        "skill_prompt": body,
        "trajectory_dir": d.join("trajectory").display().to_string(),
        "metadata": traj.get("metadata").cloned().unwrap_or(serde_json::json!({})),
        "content": content,
    }))
}

/// `cua skills list`.
pub fn cmd_list(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let skills = list();
    if json {
        util::json_line(out, &serde_json::Value::Array(skills));
        return Ok(0);
    }
    if skills.is_empty() {
        line(out, "No skills found.");
        line(
            out,
            "Record a skill with: cua skills record --sandbox <name>",
        );
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = skills
        .iter()
        .map(|s| {
            let d = s["description"].as_str().unwrap_or_default();
            let short: String = d.chars().take(40).collect();
            let created = s["created"]
                .as_str()
                .map(|c| c.chars().take(10).collect())
                .unwrap_or_else(|| "-".into());
            vec![
                s["name"].as_str().unwrap_or_default().to_string(),
                if d.chars().count() > 40 {
                    format!("{short}...")
                } else {
                    short
                },
                s["steps"].to_string(),
                created,
            ]
        })
        .collect();
    util::table(out, &["NAME", "DESCRIPTION", "STEPS", "CREATED"], &rows);
    Ok(0)
}

/// `cua skills read`.
pub fn cmd_read(name: &str, format: &str, out: &mut dyn Write) -> Result<i32, CuaError> {
    let v = read_json(name)?;
    if format == "md" {
        let _ = write!(out, "{}", v["content"].as_str().unwrap_or_default());
        line(out, "");
    } else {
        let mut v = v;
        if let Some(o) = v.as_object_mut() {
            o.remove("content");
        }
        util::json_line(out, &v);
    }
    Ok(0)
}

/// `cua skills replay`: opens the recording.
pub fn cmd_replay(name: &str, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = skill_path(name)?;
    if !d.is_dir() {
        return Err(CuaError::NotFound(format!("skill not found: {name}")));
    }
    let t = d.join("trajectory");
    let video = std::fs::read_dir(&t)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "mp4"))
        .ok_or_else(|| CuaError::NotFound(format!("no video found in {}", t.display())))?;
    line(out, format!("Opening: {}", video.display()));
    util::open_browser(&format!("file://{}", video.display()));
    Ok(0)
}

/// `cua skills delete`.
pub fn cmd_delete(name: &str, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = skill_path(name)?;
    if !d.is_dir() {
        return Err(CuaError::NotFound(format!("skill not found: {name}")));
    }
    std::fs::remove_dir_all(&d).map_err(internal)?;
    line(out, format!("Deleted skill: {name}"));
    Ok(0)
}

/// `cua skills clean`.
pub fn cmd_clean(yes: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let skills: Vec<PathBuf> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("SKILL.md").is_file())
        .collect();
    if skills.is_empty() {
        line(out, "No skills to clean.");
        return Ok(0);
    }
    eprintln!("Skills to delete:");
    for s in &skills {
        eprintln!(
            "  - {}",
            s.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    if !yes && !util::confirm(&format!("Delete {} skill(s)?", skills.len()), false) {
        line(out, "Cancelled.");
        return Ok(0);
    }
    for s in &skills {
        std::fs::remove_dir_all(s).map_err(internal)?;
    }
    line(out, format!("Deleted {} skill(s).", skills.len()));
    Ok(0)
}

// ----------------------------------------------------------------- record

/// Captioning settings.
#[derive(Clone, Debug)]
pub struct Captioner {
    /// `anthropic` or `openai`.
    pub provider: String,
    /// Model id.
    pub model: String,
    /// API key.
    pub api_key: String,
}

impl Captioner {
    /// From flags and the environment.
    pub fn resolve(
        provider: &str,
        model: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self, CuaError> {
        let env = if provider == "openai" {
            "OPENAI_API_KEY"
        } else {
            "ANTHROPIC_API_KEY"
        };
        let api_key = api_key
            .or_else(|| std::env::var(env).ok())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                CuaError::ProviderNotConfigured(format!(
                    "no {} API key; set {env} or pass --api-key",
                    provider.to_uppercase()
                ))
            })?;
        let model = model.unwrap_or_else(|| {
            if provider == "openai" {
                "gpt-4o-mini".into()
            } else {
                "claude-haiku-4-5".into()
            }
        });
        Ok(Self {
            provider: provider.into(),
            model,
            api_key,
        })
    }

    async fn caption(
        &self,
        jpeg: &[u8],
        event: &serde_json::Value,
        step: usize,
        task: &str,
    ) -> serde_json::Value {
        use base64::Engine;
        let fallback = serde_json::json!({
            "observation": "", "think": "", "action": event["type"].as_str().unwrap_or(""), "expectation": "",
        });
        let prompt = format!(
            "Describe this GUI action step. The overall task is: {task}\n\nStep {step}: {}\nEvent data: {}\n\nRespond with JSON only:\n{{\n  \"Observation\": \"Describe what you see in the screenshot\",\n  \"Think\": \"Explain the user's likely intention\",\n  \"Action\": \"Describe the action being taken\",\n  \"Expectation\": \"What should happen after this action\"\n}}",
            event["type"].as_str().unwrap_or("action"),
            event.get("data").cloned().unwrap_or(serde_json::json!({}))
        );
        let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let http = util::http();
        let base = |var: &str, d: &str| {
            std::env::var(var)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| d.into())
        };
        let text = if self.provider == "openai" {
            let r = http
                .post(format!("{}/v1/chat/completions", base("OPENAI_BASE_URL", "https://api.openai.com").trim_end_matches('/')))
                .bearer_auth(&self.api_key)
                .json(&serde_json::json!({
                    "model": self.model,
                    "messages": [{"role": "user", "content": [
                        {"type": "text", "text": prompt},
                        {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{b64}")}},
                    ]}],
                    "temperature": 0.2,
                }))
                .send()
                .await;
            match r {
                Ok(r) if r.status().is_success() => {
                    r.json::<serde_json::Value>().await.ok().and_then(|v| {
                        v["choices"][0]["message"]["content"]
                            .as_str()
                            .map(str::to_string)
                    })
                }
                _ => None,
            }
        } else {
            let r = http
                .post(format!("{}/v1/messages", base("ANTHROPIC_BASE_URL", "https://api.anthropic.com").trim_end_matches('/')))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&serde_json::json!({
                    "model": self.model,
                    "max_tokens": 1200,
                    "messages": [{"role": "user", "content": [
                        {"type": "text", "text": prompt},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": b64}},
                    ]}],
                }))
                .send()
                .await;
            match r {
                Ok(r) if r.status().is_success() => {
                    r.json::<serde_json::Value>().await.ok().and_then(|v| {
                        v["content"]
                            .as_array()?
                            .iter()
                            .find(|b| b["type"] == "text")?["text"]
                            .as_str()
                            .map(str::to_string)
                    })
                }
                _ => None,
            }
        };
        let Some(text) = text else { return fallback };
        let Some(json) = text
            .find('{')
            .and_then(|a| text.rfind('}').map(|b| &text[a..=b]))
            .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
        else {
            return fallback;
        };
        let pick = |a: &str, b: &str| {
            json.get(a)
                .or(json.get(b))
                .cloned()
                .unwrap_or(serde_json::json!(""))
        };
        serde_json::json!({
            "observation": pick("Observation", "observation"),
            "think": pick("Think", "think"),
            "action": pick("Action", "action"),
            "expectation": pick("Expectation", "expectation"),
        })
    }
}

/// Parses `[u32 BE json length][json][mp4]`.
pub fn split_recording(data: &[u8]) -> Result<(serde_json::Value, &[u8]), CuaError> {
    let bad = |m: &str| CuaError::InvalidArgument(format!("recording: {m}"));
    if data.len() < 4 {
        return Err(bad("data too short"));
    }
    let n = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let json = data.get(4..4 + n).ok_or_else(|| bad("invalid format"))?;
    let mp4 = &data[4 + n..];
    if mp4.is_empty() {
        return Err(bad("no video data"));
    }
    let v = serde_json::from_slice(json).map_err(|e| bad(&format!("bad JSON: {e}")))?;
    Ok((v, mp4))
}

fn frame_at(video: &Path, seconds: f64, dest: &Path) -> bool {
    std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-ss",
            &format!("{seconds:.3}"),
            "-i",
        ])
        .arg(video)
        .args(["-frames:v", "1", "-q:v", "2"])
        .arg(dest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && dest.is_file()
}

/// Turns a received recording into a skill. Returns the step count.
pub async fn process_recording(
    data: &[u8],
    name: &str,
    description: &str,
    cap: &Captioner,
) -> Result<usize, CuaError> {
    let (rec, mp4) = split_recording(data)?;
    let events = rec["events"].as_array().cloned().unwrap_or_default();
    let metadata = rec
        .get("metadata")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let skill = skill_path(name)?;
    let traj = skill.join("trajectory");
    std::fs::create_dir_all(&traj).map_err(internal)?;
    let video = traj.join(format!("{name}.mp4"));
    std::fs::write(&video, mp4).map_err(internal)?;
    std::fs::write(
        traj.join("events.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"events": events, "metadata": metadata}))?,
    )
    .map_err(internal)?;
    let mut steps = vec![];
    for (i, ev) in events.iter().enumerate() {
        let step = i + 1;
        let at = (ev["timestamp"].as_f64().unwrap_or(0.0) / 1000.0 - 0.1).max(0.0);
        let frame = traj.join(format!("step_{step}_full.jpg"));
        if !frame_at(&video, at, &frame) {
            steps.push(serde_json::json!({
                "step_idx": step,
                "caption": {"observation": "", "think": "", "action": ev["type"], "expectation": ""},
                "raw_event": ev,
            }));
            continue;
        }
        let jpeg = std::fs::read(&frame).map_err(internal)?;
        let caption = cap.caption(&jpeg, ev, step, description).await;
        steps.push(serde_json::json!({
            "step_idx": step,
            "caption": caption,
            "raw_event": ev,
            "screenshot_full": frame.display().to_string(),
        }));
    }
    let action_of = |s: &serde_json::Value| {
        s["caption"]["action"]
            .as_str()
            .filter(|a| !a.is_empty())
            .or(s["raw_event"]["type"].as_str())
            .unwrap_or("")
            .to_string()
    };
    std::fs::write(
        traj.join("trajectory.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "events": events,
            "trajectory": steps,
            "metadata": {
                "task_description": description,
                "total_steps": steps.len(),
                "width": metadata["width"],
                "height": metadata["height"],
                "duration": metadata["duration"],
                "created_at": chrono::Local::now().naive_local().format("%Y-%m-%dT%H:%M:%S%.6f").to_string(),
            },
        }))?,
    )
    .map_err(internal)?;
    let steps_text: Vec<String> = steps
        .iter()
        .map(|s| format!("Step {}: {}", s["step_idx"], action_of(s)))
        .collect();
    let steps_md: Vec<String> = steps
        .iter()
        .map(|s| {
            let c = &s["caption"];
            format!(
                "### Step {}: {}\n\n**Context:** {}\n\n**Intent:** {}\n\n**Expected Result:** {}\n",
                s["step_idx"],
                action_of(s),
                c["observation"].as_str().unwrap_or(""),
                c["think"].as_str().unwrap_or(""),
                c["expectation"].as_str().unwrap_or("")
            )
        })
        .collect();
    let md = format!(
        "---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\n{description}\n\n## Steps\n\n{}\n\n## Agent Prompt\n\nYou have been shown a demonstration of how to perform this task:\n{description}\n\nThe demonstration consisted of the following steps:\n{}\n\nFollow this workflow pattern, adapting as needed for the current screen state.\nTotal steps: {}\n",
        steps_md.join("\n"),
        steps_text.join("\n"),
        steps.len()
    );
    std::fs::write(skill.join("SKILL.md"), md).map_err(internal)?;
    Ok(steps.len())
}

/// Receives one recording on a loopback WebSocket.
async fn receive_recording(
    listener: tokio::net::TcpListener,
    timeout: Duration,
) -> Result<Vec<u8>, CuaError> {
    use futures_util::StreamExt;
    let fut = async {
        let (stream, _) = listener.accept().await.map_err(internal)?;
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .map_err(internal)?;
        let mut data = vec![];
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Binary(b)) => {
                    if data.len() + b.len() > MAX_RECORDING_BYTES {
                        return Err(CuaError::InvalidArgument("recording exceeds 1 GiB".into()));
                    }
                    data.extend_from_slice(&b);
                }
                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
        Ok(data)
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| CuaError::Timeout("recording timeout (30 minutes)".into()))?
}

/// `cua skills record`.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_record(
    viewer_url: String,
    provider: String,
    model: Option<String>,
    api_key: Option<String>,
    name: Option<String>,
    description: Option<String>,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        return Err(CuaError::ProviderNotConfigured(
            "ffmpeg is required for skill recording (brew install ffmpeg / apt install ffmpeg)"
                .into(),
        ));
    }
    let cap = Captioner::resolve(&provider, model, api_key)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(internal)?;
    let port = listener.local_addr().map_err(internal)?.port();
    let mut u = url::Url::parse(&viewer_url)
        .map_err(|e| CuaError::InvalidArgument(format!("viewer URL: {e}")))?;
    u.query_pairs_mut()
        .append_pair("autorecord", "true")
        .append_pair("record_format", "mp4")
        .append_pair("record_url", &format!("ws://localhost:{port}"));
    line(out, format!("Recording server started on port {port}"));
    line(
        out,
        "Recording starts when you connect; click 'Stop Recording' in the viewer when done.",
    );
    line(out, format!("Viewer: {u}"));
    let _ = out.flush();
    util::open_browser(u.as_str());
    let data = receive_recording(listener, Duration::from_secs(30 * 60)).await?;
    if data.is_empty() {
        return Err(CuaError::InvalidArgument(
            "no recording data received".into(),
        ));
    }
    line(
        out,
        format!("Received {} bytes of recording data", data.len()),
    );
    let base = match name {
        Some(n) => n,
        None => loop {
            let n = util::prompt_line("Enter skill name: ").unwrap_or_default();
            if !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                break n;
            }
            eprintln!("Use only letters, numbers, hyphens, and underscores.");
        },
    };
    let mut final_name = base.clone();
    let mut i = 1;
    while skill_path(&final_name)?.exists() {
        final_name = format!("{base}-{i}");
        i += 1;
    }
    if final_name != base {
        line(
            out,
            format!("Skill \"{base}\" exists, using \"{final_name}\""),
        );
    }
    let description = match description {
        Some(d) => d,
        None => loop {
            if let Some(d) = util::prompt_line("Describe what this skill demonstrates: ") {
                break d;
            }
            eprintln!("Description is required.");
        },
    };
    line(out, "Processing recording...");
    let steps = process_recording(&data, &final_name, &description, &cap).await?;
    line(
        out,
        format!(
            "Skill saved: {}",
            skill_path(&final_name)?.join("SKILL.md").display()
        ),
    );
    line(out, format!("Steps: {steps}"));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter() {
        let (n, d, b) =
            parse_front_matter("---\nname: x\ndescription: does y\n---\n\n# x\n").unwrap();
        assert_eq!((n.as_str(), d.as_str(), b.as_str()), ("x", "does y", "# x"));
        assert!(parse_front_matter("# no front matter").is_none());
    }

    #[test]
    fn recording_framing() {
        let json = br#"{"events":[]}"#;
        let mut data = (json.len() as u32).to_be_bytes().to_vec();
        data.extend(json);
        data.extend(b"MP4");
        let (v, mp4) = split_recording(&data).unwrap();
        assert!(v["events"].is_array());
        assert_eq!(mp4, b"MP4");
        assert!(split_recording(&data[..6]).is_err());
    }

    #[test]
    fn skill_names_cannot_escape() {
        assert!(skill_path("../x").is_err());
        assert!(skill_path("ok-name").is_ok());
    }
}
