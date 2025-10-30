<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="../../img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="../../img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![Swift 6](https://img.shields.io/badge/Swift_6-F54A2A?logo=swift&logoColor=white&labelColor=F54A2A)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)

</h1>
</div>

**lume** is a lightweight Command Line Interface and local API server to create, run and manage macOS and Linux virtual machines (VMs) with near-native performance on Apple Silicon, using Apple's `Virtualization.Framework`.

### Run prebuilt macOS images in just 1 step

<div align="center">
<img src="../../img/cli.png" alt="lume cli">
</div>

```bash
lume run macos-sequoia-vanilla:latest
```

## Quickstart

Install and run a prebuilt macOS VM in two commands:

```bash
# Install Lume
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"
# Pull & start a macOS image
lume run macos-sequoia-vanilla:latest
```

<Callout title="Security Note">
All prebuilt images use the default password `lume`. Change this immediately after your first login using the `passwd` command.
</Callout>

**System Requirements**:

- Apple Silicon Mac (M1, M2, M3, etc.)
- macOS 13.0 or later
- At least 8GB of RAM (16GB recommended)
- At least 50GB of free disk space

## Development

To get set up with Lume for development, read [these instructions](Development.md).

## Docs

- [Installation](https://trycua.com/docs/libraries/lume/installation)
- [Prebuilt Images](https://trycua.com/docs/libraries/lume/prebuilt-images)
- [CLI Reference](https://trycua.com/docs/libraries/lume/cli-reference)
- [HTTP API](https://trycua.com/docs/libraries/lume/http-api)
- [FAQ](https://trycua.com/docs/libraries/lume/faq)

## Contributing

We welcome and greatly appreciate contributions to lume! Whether you're improving documentation, adding new features, fixing bugs, or adding new VM images, your efforts help make lume better for everyone. For detailed instructions on how to contribute, please refer to our [Contributing Guidelines](CONTRIBUTING.md).

Join our [Discord community](https://discord.com/invite/mVnXXpdE85) to discuss ideas or get assistance.

## License

lume is open-sourced under the MIT License - see the [LICENSE](LICENSE) file for details.

## Trademarks

Apple, macOS, and Apple Silicon are trademarks of Apple Inc. Ubuntu and Canonical are registered trademarks of Canonical Ltd. This project is not affiliated with, endorsed by, or sponsored by Apple Inc. or Canonical Ltd.
