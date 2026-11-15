# cua-proto

The protobuf contract of Cua, with generated [prost](https://crates.io/crates/prost)
messages and [tonic](https://crates.io/crates/tonic) clients and servers:

- `cua.env.v1`: **cua-spacesd**, the daemon inside every Cua Space
  (gRPC and gRPC-Web on port 3211, server reflection on).
- `cua.daemon.v1`: the local `cua daemon`.

Most programs want the typed client on top of these:
[`cua-spacesd-client`](https://crates.io/crates/cua-spacesd-client).
Use this crate for the raw generated clients, the message types, or the
file descriptor set (`cua_proto::FILE_DESCRIPTOR_SET`).

```sh
cargo add cua-proto
```

The `.proto` files ship inside this crate under `proto/`. The build uses
`protoc` (from `PROTOC` or `PATH`) when there is one, and otherwise compiles
the files in-process, so no system protoc is needed.

Features: `client` and `server` (default) gate the generated tonic modules;
`serde` adds canonical proto3-JSON impls for every message.

Other languages: the same files are attached to every `cua-sdk-v*` GitHub
release of [trycua/cua](https://github.com/trycua/cua/releases) as
`cua-proto-<version>.tar.gz`, with a `buf.gen.yaml` for `buf generate`.

MIT licensed.
