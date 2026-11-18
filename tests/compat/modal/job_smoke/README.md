# Modal job smoke

`cua run --compat modal -- python smoke.py` from this directory: the job is a Space on the pool; its environment holds `MODAL_SERVER_URL`, `MODAL_TOKEN_ID`, `MODAL_TOKEN_SECRET` and `SSL_CERT_FILE`; the unmodified `modal` SDK creates a Sandbox (another Space) through the job's gateway, runs a command, reads and writes a file, reaches a TLS tunnel and uses a Dict. Run on a Mac with a Cua daemon (a real-Space test host); not part of the PR lane.
