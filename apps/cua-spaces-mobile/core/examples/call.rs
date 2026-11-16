// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
//! `cargo run --example call -- <method> '<args json>'`: one core call.
fn main() {
    let mut a = std::env::args().skip(1);
    let (m, args) = (
        a.next().unwrap_or_default(),
        a.next().unwrap_or_else(|| "{}".into()),
    );
    match cua_spaces_mobile_core::call(&m, &args) {
        Ok(j) => println!("{j}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1)
        }
    }
}
