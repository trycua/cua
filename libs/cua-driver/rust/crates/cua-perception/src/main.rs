use std::{
    env,
    io::{self, Write},
    path::PathBuf,
};

use cua_perception::{read_frame, write_frame, Response, Worker};

fn main() {
    if let Err(error) = start() {
        eprintln!("cua-perception: {error}");
        std::process::exit(1);
    }
}

fn start() -> Result<(), String> {
    let worker = parse_startup()?;
    run(&worker).map_err(|error| error.to_string())
}

fn parse_startup() -> Result<Worker, String> {
    let mut args = env::args_os().skip(1);
    let first = args.next().ok_or_else(usage)?;
    if first == "--fixture" {
        if args.next().is_some() {
            return Err(usage());
        }
        return Ok(Worker::fixture());
    }
    if first != "--manifest" {
        return Err(usage());
    }
    let manifest = PathBuf::from(args.next().ok_or_else(usage)?);
    if args.next().as_deref() != Some("--onnx-runtime-library".as_ref()) {
        return Err(usage());
    }
    let runtime = PathBuf::from(args.next().ok_or_else(usage)?);
    if args.next().is_some() {
        return Err(usage());
    }
    Worker::from_manifest(&manifest, &runtime).map_err(|error| error.to_string())
}

fn usage() -> String {
    "usage: cua-perception --fixture | --manifest <manifest.json> --onnx-runtime-library <library>"
        .to_owned()
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
