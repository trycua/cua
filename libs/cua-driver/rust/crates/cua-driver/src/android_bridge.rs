//! Experimental, explicitly selected Android transport. Desktop dispatch is unchanged.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const CONTRACT: &str = "cua.android.v0";
const MAX_OUTPUT: u64 = 32 * 1024 * 1024;

struct Invocation {
    device: String,
    timeout: Duration,
    image: Option<String>,
    request: Value,
}

pub(crate) fn run_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !is_requested(&args) {
        return None;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let result = match parse(&args, &id) {
        Ok(invocation) => execute(invocation),
        Err(message) => failure(&id, "refused", 2, &message),
    };
    let code = result["exit_code"].as_i64().unwrap_or(5) as i32;
    println!("{result}");
    Some(code)
}

fn is_requested(args: &[String]) -> bool {
    // Select the backend only in the global prefix. Desktop subcommands may
    // legitimately pass --local or literal selector text to another tool.
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let key = arg.split('=').next().unwrap();
        match key {
            "--device" | "--local" | "--connection" => return true,
            "--json" => i += 1,
            "--session" | "--timeout-ms" => i += if arg.contains('=') { 1 } else { 2 },
            _ => return false,
        }
    }
    false
}

fn failure(id: &str, status: &str, code: i32, message: &str) -> Value {
    json!({"contract_version":CONTRACT,"request_id":id,"status":status,"exit_code":code,"data":{},"error":{"message":message}})
}

fn parse(args: &[String], id: &str) -> Result<Invocation, String> {
    let mut flags: HashMap<String, Vec<String>> = HashMap::new();
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with('-') {
            if arg == "--local" || arg.starts_with("--local=") {
                return Err("--local Android execution is unavailable in this desktop binary; specify --device SERIAL".into());
            }
            if arg == "--connection" || arg.starts_with("--connection=") {
                return Err(
                    "--connection is unsupported for Android; specify --device SERIAL".into(),
                );
            }
            let (key, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
            let value = if key == "--json" {
                if inline.is_some() {
                    return Err("--json does not take a value".into());
                }
                "true".to_string()
            } else if let Some(v) = inline {
                v.to_string()
            } else {
                i += 1;
                args.get(i)
                    .filter(|v| !v.starts_with("--"))
                    .ok_or_else(|| format!("missing value for {key}"))?
                    .clone()
            };
            if value.is_empty() {
                return Err(format!("empty value for {key}"));
            }
            if flags.contains_key(key) && key != "--allow-app" {
                return Err(format!("duplicate {key}"));
            }
            flags.entry(key.into()).or_default().push(value);
        } else {
            words.push(arg.clone());
        }
        i += 1;
    }
    let mut take = |key: &str| flags.remove(key).and_then(|v| v.into_iter().next());
    let device = take("--device").ok_or("an explicit --device SERIAL is required")?;
    if device.starts_with('-') || device.chars().any(char::is_whitespace) {
        return Err("invalid device serial".into());
    }
    let session = take("--session");
    take("--json");
    let timeout_ms = take("--timeout-ms")
        .unwrap_or_else(|| "10000".into())
        .parse::<u64>()
        .map_err(|_| "invalid --timeout-ms")?;
    if !(1..=300000).contains(&timeout_ms) {
        return Err("--timeout-ms must be between 1 and 300000".into());
    }
    let operation = match words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["doctor"] => "doctor",
        ["capabilities"] => "capabilities",
        ["session", "create"] => "session.create",
        ["session", "inspect"] => "session.inspect",
        ["session", "renew"] => "session.renew",
        ["session", "stop"] => "session.stop",
        ["app", "launch"] => "app.launch",
        ["app", "launch", package] => {
            if flags
                .insert("--package".into(), vec![package.to_string()])
                .is_some()
            {
                return Err("package specified twice".into());
            }
            "app.launch"
        }
        ["snapshot"] => "snapshot",
        ["tap"] => "tap",
        ["gesture", "swipe"] => "gesture.swipe",
        _ => return Err("unsupported Android command".into()),
    };
    if !matches!(operation, "doctor" | "capabilities" | "session.create") && session.is_none() {
        return Err("--session ID is required".into());
    }
    if operation == "session.create" && session.is_some() {
        return Err("session create cannot reuse --session".into());
    }
    let mut params = Map::new();
    let mut image = None;
    let mut take = |key: &str| flags.remove(key).and_then(|v| v.into_iter().next());
    match operation {
        "session.create" => {
            if let Some(size) = take("--size") {
                let (w, h) = size.split_once('x').ok_or("--size must be WIDTHxHEIGHT")?;
                params.insert("width".into(), positive(w, "width")?);
                params.insert("height".into(), positive(h, "height")?);
            }
            if let Some(density) = take("--density") {
                params.insert("density".into(), positive(&density, "density")?);
            }
            if let Some(label) = take("--label") {
                params.insert("label".into(), label.into());
            }
            if let Some(apps) = flags.remove("--allow-app") {
                params.insert("allowed_apps".into(), json!(apps));
            }
        }
        "app.launch" => {
            params.insert(
                "package".into(),
                take("--package")
                    .ok_or("app launch requires a package")?
                    .into(),
            );
        }
        "snapshot" => {
            params.insert(
                "target_id".into(),
                take("--target")
                    .ok_or("snapshot requires --target ID")?
                    .into(),
            );
            image = take("--image");
        }
        "tap" | "gesture.swipe" => {
            params.insert(
                "snapshot_id".into(),
                take("--snapshot")
                    .ok_or("input requires --snapshot ID")?
                    .into(),
            );
            let coordinates: &[(&str, &str)] = if operation == "tap" {
                &[("--x", "x"), ("--y", "y")]
            } else {
                &[
                    ("--from-x", "from_x"),
                    ("--from-y", "from_y"),
                    ("--to-x", "to_x"),
                    ("--to-y", "to_y"),
                ]
            };
            for (flag, field) in coordinates {
                let value = take(flag)
                    .ok_or_else(|| format!("missing {flag}"))?
                    .parse::<u32>()
                    .map_err(|_| format!("invalid {flag}"))?;
                params.insert((*field).into(), value.into());
            }
            if operation == "gesture.swipe" {
                params.insert(
                    "duration_ms".into(),
                    positive(
                        &take("--duration-ms").ok_or("missing --duration-ms")?,
                        "duration_ms",
                    )?,
                );
            }
        }
        _ => {}
    }
    if !flags.is_empty() {
        let mut unknown: Vec<_> = flags.keys().cloned().collect();
        unknown.sort();
        return Err(format!(
            "unsupported flags for {operation}: {}",
            unknown.join(", ")
        ));
    }
    let mut request =
        json!({"contract_version":CONTRACT,"request_id":id,"operation":operation,"params":params});
    if let Some(session) = session {
        request["session_id"] = session.into();
    }
    cua_driver_contract::android::validate_request(&request)?;
    Ok(Invocation {
        device,
        timeout: Duration::from_millis(timeout_ms),
        image,
        request,
    })
}

fn positive(value: &str, name: &str) -> Result<Value, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| format!("invalid {name}"))?;
    if value == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(value.into())
}

fn execute(invocation: Invocation) -> Value {
    let id = invocation.request["request_id"].as_str().unwrap();
    let payload = STANDARD.encode(invocation.request.to_string());
    let mut command = Command::new("adb");
    command.args([
        "-s",
        &invocation.device,
        "exec-out",
        "env",
        "CLASSPATH=/data/local/tmp/cua-driver/runtime.apk",
        "app_process",
        "/",
        "ai.cua.driver.ClientMain",
        &payload,
    ]);
    let output = match run_process(&mut command, invocation.timeout) {
        Ok(output) => output,
        Err((code, message)) => {
            return failure(
                id,
                if code == 4 { "uncertain" } else { "error" },
                code,
                &message,
            )
        }
    };
    let mut response = match validate_response(&output, id) {
        Ok(response) => response,
        Err(message) => return failure(id, "uncertain", 4, &message),
    };
    if response["status"] == "ok" {
        if let Some(path) = invocation.image {
            if let Err(message) = save_image(&mut response, &path) {
                return failure(id, "error", 5, &message);
            }
        }
    }
    response
}

fn run_process(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, (i32, String)> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| (5, format!("could not spawn adb: {e}")))?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err((4, format!("adb completion is uncertain: {e}")));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err((
                4,
                "Android request timed out after dispatch; do not blindly retry input".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let bytes = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| (4, "Android response timed out after dispatch".into()))?
        .map_err(|e| (4, format!("could not read Android response: {e}")))?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err((4, "Android response exceeds output limit".into()));
    }
    Ok(bytes)
}

fn validate_response(bytes: &[u8], id: &str) -> Result<Value, String> {
    let response: Value =
        serde_json::from_slice(bytes).map_err(|_| "invalid Android response JSON")?;
    if response["contract_version"] != CONTRACT
        || response["request_id"] != id
        || !response["data"].is_object()
    {
        return Err("Android response contract, request ID, or data mismatch".into());
    }
    let valid = matches!(
        (response["status"].as_str(), response["exit_code"].as_i64()),
        (Some("ok"), Some(0))
            | (Some("refused"), Some(2 | 3))
            | (Some("uncertain"), Some(4))
            | (Some("error"), Some(5))
    );
    if !valid {
        return Err("invalid Android response status or exit code".into());
    }
    Ok(response)
}

fn save_image(response: &mut Value, path: &str) -> Result<(), String> {
    let encoded = response["data"]["image_base64"]
        .as_str()
        .ok_or("snapshot response has no image_base64")?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "snapshot contains invalid base64")?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("snapshot image is not PNG".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("cannot create snapshot image: {e}"))?;
    file.write_all(&bytes)
        .map_err(|e| format!("cannot write snapshot image: {e}"))?;
    response["data"]
        .as_object_mut()
        .unwrap()
        .remove("image_base64");
    response["data"]["image_path"] = path.into();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_args(args: &str) -> Result<Invocation, String> {
        parse(
            &args
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>(),
            "test-id",
        )
    }
    #[test]
    fn creates_scoped_request() {
        let i = parse_args("--device emulator-5554 --json session create --size 800x600 --density 240 --allow-app ai.example --allow-app ai.other --label test").unwrap();
        assert_eq!(
            i.request["params"],
            json!({"width":800,"height":600,"density":240,"allowed_apps":["ai.example","ai.other"],"label":"test"})
        );
        assert_eq!(i.request["operation"], "session.create");
    }
    #[test]
    fn desktop_dispatch_remains_unselected() {
        for args in [
            "doctor",
            "--json snapshot",
            "session stop --session existing",
            "skills add --local",
            "type --text --device",
            "--session --device doctor",
        ] {
            assert!(!is_requested(
                &args
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            ));
        }
        for args in [
            "--device=x doctor",
            "--local doctor",
            "--connection x doctor",
            "--session existing --device serial doctor",
        ] {
            assert!(is_requested(
                &args
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            ));
        }
    }
    #[test]
    fn session_operations_serialize_exactly() {
        for operation in ["inspect", "renew", "stop"] {
            let i = parse_args(&format!(
                "--device serial --session session-id session {operation}"
            ))
            .unwrap();
            assert_eq!(
                i.request,
                json!({"contract_version":CONTRACT,"request_id":"test-id","operation":format!("session.{operation}"),"session_id":"session-id","params":{}})
            );
        }
        assert!(parse_args("--device serial doctor extra").is_err());
        assert!(
            parse_args("--device serial --session s snapshot --target t --unknown value").is_err()
        );
    }
    #[test]
    fn malformed_image_never_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.png");
        for encoded in ["invalid!".to_string(), STANDARD.encode(b"not PNG")] {
            let mut response = json!({"data":{"image_base64":encoded}});
            assert!(save_image(&mut response, path.to_str().unwrap()).is_err());
            assert!(!path.exists());
        }
    }
    #[test]
    fn rejects_ambiguous_or_unsupported_requests() {
        for args in [
            "--local doctor",
            "--device a --connection x doctor",
            "--device a --device b doctor",
            "--device a snapshot",
            "--device a --session s snapshot",
            "--device a doctor --wat x",
            "--device a --timeout-ms 0 doctor",
            "--device a --session s app launch p --package q",
            "--device a --session s tap --snapshot snap --x NaN --y 3",
        ] {
            assert!(parse_args(args).is_err(), "{args}");
        }
    }
    #[test]
    fn input_is_bound_to_session_and_snapshot() {
        let i = parse_args("--device=a --session=s gesture swipe --snapshot snap --from-x 0 --from-y 2 --to-x 5 --to-y 8 --duration-ms 100").unwrap();
        assert_eq!(i.request["session_id"], "s");
        assert_eq!(i.request["params"]["snapshot_id"], "snap");
        assert_eq!(i.request["params"]["from_x"], 0);
    }
    #[test]
    fn validates_envelope_and_preserves_runtime_codes() {
        for (status, code) in [
            ("ok", 0),
            ("refused", 2),
            ("refused", 3),
            ("uncertain", 4),
            ("error", 5),
        ] {
            let response = failure("id", status, code, "test");
            assert_eq!(
                validate_response(response.to_string().as_bytes(), "id").unwrap()["exit_code"],
                code
            );
            assert!(validate_response(response.to_string().as_bytes(), "wrong").is_err());
        }
        assert!(validate_response(b"garbage", "id").is_err());
        assert!(
            validate_response(failure("id", "ok", 5, "bad").to_string().as_bytes(), "id").is_err()
        );
    }
    #[test]
    fn image_export_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.png");
        let mut response =
            json!({"data":{"image_base64":STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture")}});
        save_image(&mut response, path.to_str().unwrap()).unwrap();
        assert!(response["data"].get("image_base64").is_none());
        let mut second =
            json!({"data":{"image_base64":STANDARD.encode(b"\x89PNG\r\n\x1a\nother")}});
        assert!(save_image(&mut second, path.to_str().unwrap()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"\x89PNG\r\n\x1a\nfixture");
    }
    #[cfg(unix)]
    #[test]
    fn subprocess_timeout_is_uncertain_and_spawn_failure_is_error() {
        assert_eq!(
            run_process(Command::new("sleep").arg("1"), Duration::from_millis(20))
                .unwrap_err()
                .0,
            4
        );
        assert_eq!(
            run_process(
                &mut Command::new("/nonexistent/android-adb"),
                Duration::from_millis(20)
            )
            .unwrap_err()
            .0,
            5
        );
    }
}
