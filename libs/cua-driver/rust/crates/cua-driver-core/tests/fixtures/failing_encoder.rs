use std::io::{self, Read};

fn main() {
    if std::env::args().any(|arg| arg == "-version") {
        return;
    }
    let mut byte = [0];
    while io::stdin().read_exact(&mut byte).is_ok() {
        if byte[0] == b'q' {
            if std::env::var("CUA_RECORDING_ERROR_TEST_CHILD").as_deref() == Ok("timeout") {
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
            std::process::exit(23);
        }
    }
    std::process::exit(24);
}
