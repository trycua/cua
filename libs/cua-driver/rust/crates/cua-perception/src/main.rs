use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cua_perception::{read_frame, write_frame, Response, Worker};
use serde_json::{json, Value};

fn main() {
    if let Err(error) = start() {
        eprintln!("cua-perception: {error}");
        std::process::exit(1);
    }
}

fn start() -> Result<(), String> {
    match parse_startup()? {
        Startup::Serve(worker) => run(&worker).map_err(|error| error.to_string()),
        Startup::Gate(gate, identity) => run_gate(gate, &identity),
    }
}

enum Startup {
    Serve(Worker),
    Gate(Gate, ExtensionIdentity),
}

struct ExtensionIdentity {
    id: String,
    version: String,
}

#[derive(Clone, Copy)]
enum Gate {
    Health,
    SelfTest,
    RealParse,
    MismatchRejection,
}

fn parse_startup() -> Result<Startup, String> {
    let mut args = env::args_os().skip(1);
    let first = args.next().ok_or_else(usage)?;
    if first == "--fixture" {
        if args.next().is_some() {
            return Err(usage());
        }
        return Ok(Startup::Serve(Worker::fixture()));
    }
    let gate = match first.to_str() {
        Some("--health") => Some(Gate::Health),
        Some("--self-test") => Some(Gate::SelfTest),
        Some("--real-parse-self-test") => Some(Gate::RealParse),
        Some("--mismatch-rejection-self-test") => Some(Gate::MismatchRejection),
        _ => None,
    };
    if let Some(gate) = gate {
        let identity = parse_extension_identity(&mut args)?;
        return Ok(Startup::Gate(gate, identity));
    }
    if first != "--manifest" {
        return Err(usage());
    }
    let manifest = PathBuf::from(args.next().ok_or_else(usage)?);
    if args.next().as_deref() != Some("--onnx-runtime-library".as_ref()) {
        return Err(usage());
    }
    let runtime = PathBuf::from(args.next().ok_or_else(usage)?);
    let identity = parse_extension_identity(&mut args)?;
    Worker::from_manifest_with_extension_identity(
        &manifest,
        &runtime,
        &identity.id,
        &identity.version,
    )
    .map(Startup::Serve)
    .map_err(|error| error.to_string())
}

fn parse_extension_identity(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<ExtensionIdentity, String> {
    let flag = args.next().ok_or_else(usage)?;
    if flag != "--extension-id" {
        return Err(usage());
    }
    let id = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    if args.next().as_deref() != Some("--extension-version".as_ref()) {
        return Err(usage());
    }
    let version = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    Ok(ExtensionIdentity { id, version })
}

fn usage() -> String {
    "usage: cua-perception --fixture | --manifest <manifest.json> --onnx-runtime-library <library> --extension-id <id> --extension-version <version> | (--health | --self-test | --real-parse-self-test | --mismatch-rejection-self-test) --extension-id <id> --extension-version <version>".to_owned()
}

fn run_gate(gate: Gate, identity: &ExtensionIdentity) -> Result<(), String> {
    let root = installed_root()?;
    let manifest = find_required_file(
        &root,
        &[
            Path::new("model-manifest.json"),
            Path::new("models/model-manifest.json"),
        ],
        "model manifest",
    )?;
    let runtime = resolve_runtime(&root)?;
    if matches!(gate, Gate::MismatchRejection) {
        return run_mismatch_rejection(&manifest, &runtime, identity);
    }

    let worker = Worker::from_manifest_with_extension_identity(
        &manifest,
        &runtime,
        &identity.id,
        &identity.version,
    )
    .map_err(|error| error.to_string())?;
    let (method, params) = match gate {
        Gate::Health => ("health", json!({})),
        Gate::SelfTest => ("self_test", json!({})),
        Gate::RealParse => {
            let fixture = find_required_file(
                &root,
                &[Path::new("verification/known-answer.png")],
                "real-parse fixture",
            )?;
            let bytes = fs::read(&fixture)
                .map_err(|error| format!("read {}: {error}", fixture.display()))?;
            let (width, height) = png_dimensions(&bytes)?;
            (
                "parse",
                json!({
                    "capture_id": "candidate-real-parse",
                    "image": {
                        "media_type": "image/png",
                        "width": width,
                        "height": height,
                        "byte_length": bytes.len(),
                        "data_base64": BASE64.encode(bytes),
                    }
                }),
            )
        }
        Gate::MismatchRejection => unreachable!(),
    };
    let payload = serde_json::to_vec(&json!({
        "protocol": "cua-perception/1",
        "request_id": format!("candidate-{method}"),
        "method": method,
        "params": params,
    }))
    .map_err(|error| error.to_string())?;
    let response =
        serde_json::to_value(worker.handle_payload(&payload)).map_err(|error| error.to_string())?;
    require_success(&response)?;
    println!(
        "{}",
        serde_json::to_string(&response).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn installed_root() -> Result<PathBuf, String> {
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let parent = executable
        .parent()
        .ok_or_else(|| "worker executable has no parent directory".to_owned())?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("bin") {
        parent
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "installed worker has no extension root".to_owned())
    } else {
        Ok(parent.to_path_buf())
    }
}

fn find_required_file(root: &Path, relatives: &[&Path], label: &str) -> Result<PathBuf, String> {
    relatives
        .iter()
        .map(|relative| root.join(relative))
        .find(|path| path.is_file())
        .ok_or_else(|| format!("{label} is unavailable under {}", root.display()))
}

fn resolve_runtime(root: &Path) -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("CUA_PERCEPTION_RUNTIME") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "CUA_PERCEPTION_RUNTIME is not a regular file: {}",
            path.display()
        ));
    }
    let directory = root.join("runtime");
    let mut matches = fs::read_dir(&directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("dll" | "dylib" | "so")
                )
        })
        .collect::<Vec<_>>();
    matches.sort();
    match matches.as_slice() {
        [runtime] => Ok(runtime.clone()),
        _ => Err(format!(
            "expected exactly one runtime library under {}, found {}",
            directory.display(),
            matches.len()
        )),
    }
}

fn run_mismatch_rejection(
    manifest: &Path,
    runtime: &Path,
    identity: &ExtensionIdentity,
) -> Result<(), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let tampered = env::temp_dir().join(format!(
        "cua-perception-runtime-mismatch-{}-{nonce}",
        std::process::id()
    ));
    fs::copy(runtime, &tampered)
        .map_err(|error| format!("copy runtime mismatch fixture: {error}"))?;
    let result = (|| {
        use std::io::Write as _;
        fs::OpenOptions::new()
            .append(true)
            .open(&tampered)
            .and_then(|mut file| file.write_all(b"tampered"))
            .map_err(|error| format!("tamper runtime mismatch fixture: {error}"))?;
        match Worker::from_manifest_with_extension_identity(
            manifest,
            &tampered,
            &identity.id,
            &identity.version,
        ) {
            Err(error)
                if error
                    .to_string()
                    .to_ascii_lowercase()
                    .contains("sha256 mismatch") =>
            {
                println!("{{\"mismatch_rejection\":true}}");
                Ok(())
            }
            Err(error) => Err(format!(
                "runtime mismatch failed for an unexpected reason: {error}"
            )),
            Ok(_) => Err("worker accepted a runtime whose SHA-256 did not match".to_owned()),
        }
    })();
    let cleanup = fs::remove_file(&tampered)
        .map_err(|error| format!("remove runtime mismatch fixture: {error}"));
    result.and(cleanup)
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err("real-parse fixture is not a PNG with an IHDR".to_owned());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("four bytes"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("four bytes"));
    if width == 0 || height == 0 {
        return Err("real-parse fixture has zero dimensions".to_owned());
    }
    Ok((width, height))
}

fn require_success(response: &Value) -> Result<(), String> {
    if response.get("status").and_then(Value::as_str) == Some("ok") {
        Ok(())
    } else {
        Err(format!("worker gate failed: {response}"))
    }
}

fn run(worker: &Worker) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    loop {
        let response = match read_frame(&mut reader) {
            Ok(Some(payload)) => worker.handle_payload(&payload),
            Ok(None) => return Ok(()),
            Err(error) => {
                let response = Response::from_frame_error(&error);
                write_response(&mut writer, &response)?;
                return Ok(());
            }
        };
        write_response(&mut writer, &response)?;
    }
}

fn write_response(writer: &mut impl Write, response: &Response) -> io::Result<()> {
    let payload = serde_json::to_vec(response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_frame(writer, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn real_startup_identity_is_explicit_and_complete() {
        let mut missing = Vec::<OsString>::new().into_iter();
        assert_eq!(parse_extension_identity(&mut missing).err(), Some(usage()));

        let mut partial = ["--extension-id", "cua-perception"]
            .map(OsString::from)
            .into_iter();
        assert_eq!(parse_extension_identity(&mut partial).err(), Some(usage()));

        let mut complete = [
            "--extension-id",
            "cua-perception",
            "--extension-version",
            "0.1.0",
        ]
        .map(OsString::from)
        .into_iter();
        let identity = parse_extension_identity(&mut complete).unwrap();
        assert_eq!(identity.id, "cua-perception");
        assert_eq!(identity.version, "0.1.0");
    }
}
