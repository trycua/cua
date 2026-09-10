use std::io::{self, Read};

fn main() {
    if std::env::args().any(|arg| arg == "-version") {
        return;
    }
    let mut byte = [0];
    while io::stdin().read_exact(&mut byte).is_ok() {
        if byte[0] == b'q' {
            std::process::exit(23);
        }
    }
    std::process::exit(24);
}
