# Cua protobuf contract

The `.proto` files of Cua, for calling a Space from any language:

- `cua/env/v1`: **cua-spacesd**, the daemon inside every Cua Space. It serves
  native gRPC (HTTP/2) and gRPC-Web on port **3211**. Start with
  `cua.env.v1.SystemService` (`GetCapabilities`, `Health`),
  `ProcessService` (run commands), `FilesystemService`, and
  `ComputerService` (screenshots, pointer, keyboard).
- `cua/daemon/v1`: the local `cua daemon`.

Every call, reflection included, needs the Space's token as
`authorization: Bearer <token>`. The cua CLI prints a Space's spacesd URL
and token:

```sh
cua sb info <space> --connection --json   # .spacesd.url, .spacesd.token
```

## Look around with grpcurl

spacesd has gRPC server reflection on, so grpcurl needs no files:

```sh
TOKEN=...            # .spacesd.token
ADDR=127.0.0.1:3211  # host:port of .spacesd.url

grpcurl -plaintext -H "authorization: Bearer $TOKEN" "$ADDR" list
grpcurl -plaintext -H "authorization: Bearer $TOKEN" "$ADDR" describe cua.env.v1.ProcessService
grpcurl -plaintext -H "authorization: Bearer $TOKEN" "$ADDR" cua.env.v1.SystemService/Health
```

Or point grpcurl at these files instead of reflection:
`grpcurl -import-path . -proto cua/env/v1/system.proto ...`.
Use TLS (drop `-plaintext`) when the endpoint is `https://`.

## Generate a client with buf

`buf.gen.yaml` generates Go messages and gRPC clients into `gen/go`, and has
commented-out plugins for Java, C# and Python. No Buf account is needed.

```sh
# Set go_package_prefix in buf.gen.yaml to <your module>/gen/go first.
buf generate
```

Without the tarball, generate straight from a git ref:

```sh
buf generate --template buf.gen.yaml \
  "https://github.com/trycua/cua.git#ref=cua-sdk-v<version>,subdir=libs/cua/proto"
```

Plain `protoc` works too: the include root is this directory.

## Rust

Rust gets the same contract from crates.io: `cargo add cua-spacesd-client`
(typed client) or `cargo add cua-proto` (generated types; carries these files).

The contract is versioned with the cua SDK. MIT licensed.
