//! The code-interpreter server that `code-interpreter-v1` sandboxes run
//! (`src/e2b/assets/code_interpreter.py`), driven over its HTTP API the way
//! the e2b-code-interpreter SDKs do. The script is the guest-side program;
//! here it runs on the test host with fixed, harmless snippets.

use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

struct Server {
    child: Child,
    _dir: tempfile::TempDir,
    base: String,
    http: reqwest::Client,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("ci.py");
    std::fs::write(
        &script,
        include_str!("../src/e2b/assets/code_interpreter.py"),
    )
    .unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new("python3")
        .arg(&script)
        .env("CUA_CODE_INTERPRETER_PORT", port.to_string())
        .env("HOME", dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("python3");
    let s = Server {
        child,
        _dir: dir,
        base: format!("http://127.0.0.1:{port}"),
        http: reqwest::Client::new(),
    };
    for _ in 0..100 {
        if s.http
            .get(format!("{}/health", s.base))
            .send()
            .await
            .is_ok()
        {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the server did not start");
}

impl Server {
    async fn exec(&self, body: Value) -> (u16, Vec<Value>) {
        let r = self
            .http
            .post(format!("{}/execute", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        let lines = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        (status, lines)
    }
}

fn kinds(evs: &[Value]) -> Vec<String> {
    evs.iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn state_persists_and_output_streams_in_order() {
    let s = start().await;
    let (st, evs) = s.exec(json!({"code": "x = 41\nprint('a')"})).await;
    assert_eq!(st, 200);
    assert_eq!(kinds(&evs), ["stdout", "number_of_executions"]);
    assert_eq!(evs[0]["text"], "a\n");
    // A string result is its text, not its repr, as the E2B server shows it.
    let (_, evs) = s.exec(json!({"code": "'v'"})).await;
    assert_eq!(evs[0]["text"], "v");
    let (_, evs) = s.exec(json!({"code": "x + 1"})).await;
    assert_eq!(evs[0]["type"], "result");
    assert_eq!(evs[0]["text"], "42");
    assert_eq!(evs[0]["is_main_result"], true);
    assert_eq!(evs[1]["execution_count"], 3);
    // Two prints in one burst are one message, as Jupyter batches them;
    // stdout and stderr are separate streams.
    let (_, evs) = s
        .exec(json!({"code": "import sys\nprint(1)\nprint(2, file=sys.stderr)\nprint(3)"}))
        .await;
    let text = |kind: &str| -> Vec<String> {
        evs.iter()
            .filter(|e| e["type"] == kind)
            .map(|e| e["text"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(text("stdout"), ["1\n3\n"]);
    assert_eq!(text("stderr"), ["2\n"]);
    assert_eq!(evs.last().unwrap()["type"], "number_of_executions");
    // Output is streamed while the cell still runs, not held to its end.
    let (_, evs) = s
        .exec(json!({"code": "import time\nprint('early', flush=True)\ntime.sleep(0.4)\nprint('late')"}))
        .await;
    assert_eq!(text_of(&evs), ["early\n", "late\n"]);
}

fn text_of(evs: &[Value]) -> Vec<String> {
    evs.iter()
        .filter(|e| e["type"] == "stdout")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn errors_and_env_vars_and_child_output() {
    let s = start().await;
    let (_, evs) = s.exec(json!({"code": "1/0"})).await;
    assert_eq!(evs[0]["type"], "error");
    assert_eq!(evs[0]["name"], "ZeroDivisionError");
    assert_eq!(evs[0]["value"], "division by zero");
    assert!(
        evs[0]["traceback"]
            .as_str()
            .unwrap()
            .contains("ZeroDivisionError")
    );
    let (_, evs) = s
        .exec(json!({"code": "import os\nos.system('echo child')\nprint(os.environ['Q'])", "env_vars": {"Q": "z"}}))
        .await;
    let out: String = evs
        .iter()
        .filter(|e| e["type"] == "stdout")
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(out, "child\nz\n");
    let (_, evs) = s
        .exec(json!({"code": "import os\nprint(os.environ.get('Q'))"}))
        .await;
    assert_eq!(evs[0]["text"], "None\n", "per-call env does not leak");
}

/// A child's output comes before the Python text printed after the child
/// wrote it, however the threads are scheduled. The cell holds the GIL (a
/// ctypes call that does not release it) while the child writes, so the
/// thread that reads the child's pipe cannot run until the cell is done.
#[tokio::test]
async fn child_output_is_ordered_before_later_prints_even_when_the_reader_is_starved() {
    let s = start().await;
    let code = "import subprocess, ctypes\n\
                subprocess.Popen(['sh', '-c', 'echo child'])\n\
                ctypes.PyDLL(None).usleep(400000)\n\
                print('z')";
    let (_, evs) = s.exec(json!({ "code": code })).await;
    assert_eq!(text_of(&evs).concat(), "child\nz\n");
}

#[tokio::test]
async fn contexts_are_isolated_and_manageable() {
    let s = start().await;
    let c: Value = s
        .http
        .post(format!("{}/contexts", s.base))
        .json(&json!({"cwd": "/"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = c["id"].as_str().unwrap();
    assert_eq!(c["language"], "python");
    assert_eq!(c["cwd"], "/");
    s.exec(json!({"code": "v = 1", "context_id": id})).await;
    let (_, evs) = s.exec(json!({"code": "v", "context_id": id})).await;
    assert_eq!(evs[0]["text"], "1");
    let (_, evs) = s.exec(json!({"code": "v"})).await;
    assert_eq!(
        evs[0]["name"], "NameError",
        "the default context is separate"
    );
    let (_, evs) = s
        .exec(json!({"code": "import os; os.getcwd()", "context_id": id}))
        .await;
    assert_eq!(evs[0]["text"], "/");
    let r = s
        .http
        .post(format!("{}/contexts/{id}/restart", s.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let (_, evs) = s.exec(json!({"code": "v", "context_id": id})).await;
    assert_eq!(evs[0]["name"], "NameError", "restart clears the state");
    let list: Value = s
        .http
        .get(format!("{}/contexts", s.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.as_array().unwrap().iter().any(|c| c["id"] == id));
    let r = s
        .http
        .delete(format!("{}/contexts/{id}", s.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let (st, _) = s.exec(json!({"code": "1", "context_id": id})).await;
    assert_eq!(st, 404);
}

#[tokio::test]
async fn bash_and_unavailable_languages() {
    let s = start().await;
    let (_, evs) = s
        .exec(json!({"code": "echo a; echo b >&2; exit 3", "language": "bash"}))
        .await;
    let k = kinds(&evs);
    assert!(k.contains(&"stdout".to_string()) && k.contains(&"stderr".to_string()));
    let err = evs.iter().find(|e| e["type"] == "error").unwrap();
    assert_eq!(err["value"], "exit status 3");
    let (st, _) = s.exec(json!({"code": "1", "language": "cobol"})).await;
    assert_eq!(st, 400);
    let (st, _) = s
        .exec(json!({"code": "1", "language": "python", "context_id": "x"}))
        .await;
    assert_eq!(st, 400);
}
