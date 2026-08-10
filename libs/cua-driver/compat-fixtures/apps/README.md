# Previous-release application fixtures

These small applications were frozen against `cua-driver-rs-v0.12.6`
(`9eb1f481b8a12cd6ffda2ad5af21653a9e5aa9e5`). They intentionally use only
the released connection constructor, endpoint accessor, and language-specific
cleanup operation.

CI compiles or runs the unchanged sources against the candidate Rust, Python,
and TypeScript packages. The apps do not require a running daemon: constructing
a compatibility client and reading its selected endpoint is local and
side-effect free.

Do not update these sources for additive SDK features. Change them only after
an explicit compatibility decision establishes a new frozen baseline.
