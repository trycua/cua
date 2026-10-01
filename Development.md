# Development

This file is the contributor map for the Cua monorepo. Each component owns its
detailed setup, build, and test instructions. Start here, then follow the guide
next to the code you plan to change.

## Choose a Component

| Area                         | Main paths                                                                           | Toolchain                           | Start here                                                                                                                   |
| ---------------------------- | ------------------------------------------------------------------------------------ | ----------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| cua SDK and `cua` CLI        | `libs/cua` (Rust crates, `proto/`, `python/`, `typescript/`, `swift/`, `kotlin/`)    | Rust, protoc, buf; Node, uv, Swift  | [`libs/cua/README.md`](libs/cua/README.md)                                                                                   |
| cua-spacesd               | `libs/cua-spacesd`                                                                | Rust, protoc; Docker for Linux lanes | [`libs/cua-spacesd/README.md`](libs/cua-spacesd/README.md)                                                            |
| Cua Spaces app               | `apps/cua-spaces`                                                                    | Node.js, pnpm, Rust, Tauri 2        | [`apps/cua-spaces/README.md`](apps/cua-spaces/README.md)                                                                     |
| cua-driver                   | `libs/cua-driver/rust`, `libs/cua-driver/python`                                     | Rust, platform SDKs, Nix on Linux   | [`libs/cua-driver/README.md`](libs/cua-driver/README.md), [`libs/cua-driver/rust/README.md`](libs/cua-driver/rust/README.md) |
| Python SDKs and services     | `libs/python` (incl. `cua-sandbox`), `libs/cua-bench`                                | Python 3.12, uv                     | Package `pyproject.toml` and README                                                                                          |
| TypeScript SDKs              | `libs/typescript`                                                                    | Node.js, pnpm                       | [`libs/typescript/README.md`](libs/typescript/README.md) and its `package.json` scripts                                      |
| CuaBot                       | `libs/cuabot`                                                                        | Node.js, pnpm                       | [`libs/cuabot/README.md`](libs/cuabot/README.md)                                                                             |
| Lume                         | `libs/lume`                                                                          | Swift, Xcode command-line tools     | [`libs/lume/Development.md`](libs/lume/Development.md)                                                                       |
| Sandbox and container images | `libs/images`, `libs/kasm`, `libs/lumier`, `libs/qemu-docker`, `libs/xfce`, `libs/xfce-cua` | Docker plus the component toolchain | [`libs/images/README.md`](libs/images/README.md) or the README in the component                                        |
| Cross-language E2E           | `tests/e2e/cua-sdk`, `tests/e2e/local-runtimes`                                      | Rust, uv, Node.js, Go, Docker       | [`TESTING.md`](TESTING.md#cua-sdk-and-cli)                                                                                   |
| Public documentation         | `docs/content/docs`                                                                  | Node.js, pnpm, Fumadocs             | [`docs/README.md`](docs/README.md)                                                                                           |
| Samples and examples         | `samples`, `examples`                                                                | Depends on the sample               | README next to the sample                                                                                                    |

The repository changes quickly. Directory listings in this root guide are an
orientation aid, not a package registry. Release targets come from
[`.github/releases/components.json`](.github/releases/components.json) and
[`.github/workflows/release-bump-version.yml`](.github/workflows/release-bump-version.yml),
and test ownership comes from CI plus the component guides.

Two boundaries to keep in mind:

- `libs/cua-spacesd` is server-only (it runs inside the sandbox). Client
  code, including wire formats and media clients, lives in `libs/cua`, and the
  driver depends on it, never the reverse. CI enforces this.
- `libs/fleet` is a read-only mirror of the Fleet SDK. `libs/cua` depends on it
  by path; never edit it here.

## Common Setup

Clone the repository and enter it:

```bash
git clone https://github.com/trycua/cua.git
cd cua
```

Install only the toolchains required by your component:

| Work                        | Required tools                                                              |
| --------------------------- | --------------------------------------------------------------------------- |
| Python packages             | Python 3.12 and [uv](https://docs.astral.sh/uv/)                            |
| TypeScript, CuaBot, or docs | The Node.js version required by the component and its declared pnpm version |
| cua SDK or cua-spacesd   | Rust (pinned in `rust-toolchain.toml`), `protoc`, and `buf` for contract checks |
| cua-driver                  | Rust plus the target OS SDK; Nix for the reproducible Linux lanes           |
| Lume                        | macOS, Swift, and Xcode command-line tools                                  |
| Images                      | Docker or the image-specific builder documented by the component            |

The root uv workspace contains only the members declared in
[`pyproject.toml`](pyproject.toml). Other directories under `libs/python` are
independent packages; use their own `pyproject.toml` and CI workflow.

The root Node package installs repository-wide Prettier only. Run `pnpm
install` inside `libs/typescript`, `libs/cuabot`, `apps/cua-spaces`, or `docs`
for those components' dependencies. `libs/cua/typescript` uses npm.

API keys are not required for ordinary builds and deterministic tests. Add
credentials only for a test or example that explicitly calls an external
provider, and never commit them.

## Root Formatting Hooks

Install the root Python development tools and optional Git hooks with:

```bash
uv sync --group dev
uv run pre-commit install
```

Run all configured hooks against the repository with:

```bash
uv run pre-commit run --all-files
```

The hooks currently run Prettier, the TypeScript workspace typecheck, Black,
isort, and Ruff. Install the TypeScript workspace dependencies before running
the typecheck hook. Mypy is configured in `pyproject.toml` but is not a
pre-commit gate.

For a read-only repository-wide formatting check:

```bash
pnpm install --frozen-lockfile
pnpm prettier:check
```

Component-specific Rust, TypeScript, Swift, and documentation checks remain in
their component guides. See [`TESTING.md`](TESTING.md) for the test map.

## cua-driver Development

cua-driver has three distinct validation layers:

1. Rust unit and protocol tests that do not require a target GUI application.
2. Source-built harness E2E tests that drive Electron, Tauri, and native toolkit
   fixtures in a real user desktop session.
3. Supporting installed-application checks whose runner declares whether they
   are canonical (the macOS Calculator/TextEdit rows) or optional (LibreOffice).

The Rust harnesses are the source of truth for desktop behavior. Python tests
do not duplicate that matrix. Start with:

- [`libs/cua-driver/rust/README.md`](libs/cua-driver/rust/README.md) for the Cargo workspace.
- [`libs/cua-driver/rust/crates/cua-driver-e2e/tests/README.md`](libs/cua-driver/rust/crates/cua-driver-e2e/tests/README.md) for desktop E2E test ownership.
- [`scripts/ci/README.md`](scripts/ci/README.md) for canonical OS runners.
- [Platform support](https://cua.ai/docs/cua-driver/concepts/platform-support) for current capability boundaries.
- [How Cua Driver is validated](https://cua.ai/docs/cua-driver/concepts/how-cua-driver-is-validated) for the public evidence model.
- [Platform roadmap](https://cua.ai/docs/cua-driver/concepts/platform-roadmap) for remaining work and platform boundaries.

Windows and macOS desktop tests need a real user session. Windows requires an
active console or RDP session. macOS requires a logged-in session with
Accessibility and Screen Recording permissions. The hosted Linux Sway and
nested-compositor runners create controlled sessions; GNOME, KDE, and real
Xorg validation use an existing graphical login.

## Documentation

Public docs use Fumadocs and follow Diataxis:

- tutorials teach a first success;
- how-to guides solve a specific task;
- concepts explain constraints and design;
- reference pages state commands, contracts, support, and limits.

Run documentation commands from `docs`; see [`docs/README.md`](docs/README.md).
Contributor-only implementation notes should remain next to their component
instead of entering the public docs navigation.

## Releases

Release Please manages the components listed in
[`.github/releases/components.json`](.github/releases/components.json) (Cua
Driver, Cua Perception, Lume, and the cua SDK) plus cua-sandbox
(`sandbox-v*` tags). Their pull request titles decide the release; see
[`CONTRIBUTING.md`](CONTRIBUTING.md#agent-assisted-contributions).

Other packages are released through the
[Legacy packages: Bump Version](https://github.com/trycua/cua/actions/workflows/release-bump-version.yml)
workflow. Its `service` input is the current target list. Each package's
`.bumpversion.cfg`, Cargo manifest, or package manifest owns its version and
tag format, and tag-triggered CD workflows publish.

The root `Makefile` provides local version inspection and dry-run helpers. It
does not publish production releases.

## Generated and Local Files

Do not commit build products, staged harness binaries, local VM artifacts,
credentials, permission databases, or editor-specific state. Promote an
artifact into source control only when it becomes a stable fixture, sample, or
maintained document.
