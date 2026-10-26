`cua run --compat e2b -- python smoke.py` from this directory: `cua run`
ships it, `uv run` installs the pinned `e2b` from `pyproject.toml`, and the
unmodified SDK creates a Cua Space through the job's gateway, runs commands,
writes files and fetches `https://get_host(8080)`.
