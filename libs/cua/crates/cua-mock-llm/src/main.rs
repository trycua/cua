//! `cua-mock-llm [--listen ADDR] [--capture-dir DIR] [--rules FILE] [--scripts FILE]`:
//! the scripted mock provider. The key clients must present comes from
//! `CUA_MOCK_LLM_KEY` (default `mock-key`), never argv. `--rules` is a JSON
//! array of `{"match": "prompt text", "script": "say ..."}`: a prompt with no
//! inline `mock:` directive that contains `match` runs `script`. `--scripts`
//! is the same with `{"when": …, "do": …}`, matched regardless of case. Either
//! flag reads either spelling; both may be given.

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut listen = "127.0.0.1:8787".to_string();
    let mut capture_dir = None;
    let mut rules = vec![];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--listen" => listen = args.next().unwrap_or(listen),
            "--capture-dir" => capture_dir = args.next().map(Into::into),
            flag @ ("--rules" | "--scripts") => {
                let path = args.next().unwrap_or_default();
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| std::io::Error::other(format!("{flag} {path}: {e}")))?;
                rules.extend(
                    cua_mock_llm::scenario::parse_rules(&text)
                        .map_err(|e| std::io::Error::other(format!("{flag} {path}: {e}")))?,
                );
            }
            "-h" | "--help" => {
                println!(
                    "cua-mock-llm [--listen ADDR] [--capture-dir DIR] [--rules FILE] [--scripts FILE]  (key: $CUA_MOCK_LLM_KEY)"
                );
                return Ok(());
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let config = cua_mock_llm::Config {
        api_key: std::env::var("CUA_MOCK_LLM_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "mock-key".into()),
        capture_dir,
        rules,
    };
    eprintln!("cua-mock-llm (mock provider, scripted) on http://{listen}");
    cua_mock_llm::serve(&listen, config).await
}
