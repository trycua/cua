//! `cargo run -p cua-bindgen -- generate --library <libcua_sdk> --language <python|swift|kotlin> --out-dir <dir>`
//!
//! Driven by `libs/cua/scripts/generate-uniffi-bindings.mjs`, which owns
//! the output locations and the `--check` drift gate.
//!
//! `cua-bindgen docs --library <lib> [--namespace <ns>]` prints the exported
//! API of one UniFFI namespace (default `cua_sdk`) as JSON for the docs
//! reference (`scripts/docs-generators/cua-sdk.ts`). The Cua Spaces app
//! export is read with `--library <libcua_spaces_ffi> --namespace cua_spaces_ffi`.

mod docs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("docs") {
        if let Err(error) = run_docs(&args[2..]) {
            eprintln!("cua-bindgen docs: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    uniffi::uniffi_bindgen_main();
}

fn run_docs(args: &[String]) -> anyhow::Result<()> {
    let mut library = None;
    let mut namespace = docs::NAMESPACE.to_string();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--library" => library = iter.next().cloned(),
            other if other.starts_with("--library=") => {
                library = Some(other["--library=".len()..].to_string())
            }
            "--namespace" => {
                namespace = iter
                    .next()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("--namespace needs a value"))?
            }
            other if other.starts_with("--namespace=") => {
                namespace = other["--namespace=".len()..].to_string()
            }
            "-h" | "--help" => {
                println!(
                    "usage: cua-bindgen docs --library <path to the cdylib> [--namespace <ns>] (default namespace: {})",
                    docs::NAMESPACE
                );
                return Ok(());
            }
            other => anyhow::bail!("unexpected argument {other:?}"),
        }
    }
    let library = library.ok_or_else(|| anyhow::anyhow!("--library is required"))?;
    let doc = docs::dump(camino::Utf8Path::new(&library), &namespace)?;
    print!("{}", docs::render(&doc));
    Ok(())
}
