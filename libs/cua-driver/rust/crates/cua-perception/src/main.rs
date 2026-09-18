use std::io::{self, Write};

use cua_perception::{handle_payload, read_frame, write_frame, Response};

fn main() {
    if let Err(error) = run() {
        eprintln!("cua-perception: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    loop {
        let response = match read_frame(&mut reader) {
            Ok(Some(payload)) => handle_payload(&payload),
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
