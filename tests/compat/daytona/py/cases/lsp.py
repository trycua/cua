"""LSP servers in the sandbox (pylsp for Python). Need a real guest with
network for the first start (the server is installed on demand).

Derived from daytona 0.225.0 `_sync/lsp_server.py` and the toolbox client's
`lsp_api.py`."""

from harness import case, err_obs

APP = """import os


def greet(name):
    return "hi " + name


class Greeter:
    def hello(self):
        return greet("x")


os.pa
"""


@case(
    "lsp.python_session",
    needs="space",
    ref="_sync/lsp_server.py start/did_open/completions/document_symbols/sandbox_symbols/did_close/stop",
    timeout=420.0,
)
async def python_session(c):
    sb = await c.main()
    sdk = c.sdk
    await c.w(sb.fs.upload_file(APP.encode(), "/tmp/lspp/app.py"))
    lsp = sb.create_lsp_server("python", "/tmp/lspp")
    await c.w(lsp.start())
    try:
        await c.w(lsp.did_open("/tmp/lspp/app.py"))
        comp = await c.w(
            lsp.completions("/tmp/lspp/app.py", sdk.LspCompletionPosition(line=12, character=5))
        )
        labels = [i.label for i in comp.items]
        assert any(lnk.startswith("path") for lnk in labels), labels[:20]
        syms = await c.w(lsp.document_symbols("/tmp/lspp/app.py"))
        names = sorted(s.name for s in syms)
        assert "greet" in names and "Greeter" in names, names
        greet = next(s for s in syms if s.name == "greet")
        assert greet.location.range.start.line == 3 and greet.location.uri.endswith(
            "/tmp/lspp/app.py"
        ), greet
        wsyms = await c.w(lsp.sandbox_symbols("Greeter"))
        assert any(s.name == "Greeter" for s in wsyms), [s.name for s in wsyms][:10]
        await c.w(lsp.did_close("/tmp/lspp/app.py"))
    finally:
        await c.w(lsp.stop())
    return {
        "completionHasPath": True,
        "symbols": ["Greeter", "greet"],
        "greetLine": 3,
        "workspaceFindsClass": True,
    }


@case("lsp.errors", needs="space", ref="common/errors.py LSP_SERVER_NOT_INITIALIZED")
async def errors(c):
    sb = await c.main()
    sdk = c.sdk
    lsp = sb.create_lsp_server("python", "/tmp/never-started")
    e = await c.raises(
        lambda: lsp.completions(
            "/tmp/never-started/a.py", sdk.LspCompletionPosition(line=0, character=0)
        )
    )
    assert e and e["type"] == "DaytonaLspServerNotInitializedError", e
    assert e["status_code"] == 400 and e["code"] == "LSP_SERVER_NOT_INITIALIZED", e
    bad = await c.raises(lambda: sb.create_lsp_server("cobol", "/tmp/x").start())
    assert bad and bad["status_code"] == 400, bad
    return {"notStarted": err_obs(e), "unsupportedLanguage": 400}
