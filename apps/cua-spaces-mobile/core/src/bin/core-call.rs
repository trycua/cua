// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `core-call <method> <argsJson>`: the same core the phone links, from a command line. The opt-in end to end test
//! (`__tests__/e2e/harness.e2e.test.ts`) answers the app's core calls with it, so a local-harness run uses the real
//! core rather than recorded answers. Prints the result JSON; an error goes to stderr with a non-zero exit.

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(method), Some(json)) = (args.next(), args.next()) else {
        eprintln!("usage: core-call <method> <argsJson>");
        std::process::exit(2);
    };
    match cua_spaces_mobile_core::call(&method, &json) {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
