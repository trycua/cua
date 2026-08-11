{
  description = "CUA Docs MCP Server - Nix-built Docker image with CPU-only PyTorch";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };

        python = pkgs.python312;

        # Base image with Python, uv, and system dependencies
        # Python packages are installed in CI via the install script
        baseImage = pkgs.dockerTools.buildLayeredImage {
          name = "docs-mcp-server-base";
          tag = "latest";

          contents = [
            pkgs.bashInteractive
            pkgs.coreutils
            pkgs.cacert
            python
            pkgs.uv
            # Required for native extensions
            pkgs.stdenv.cc.cc.lib
            pkgs.zlib
          ];

          extraCommands = ''
            # Create FHS-like structure
            mkdir -p app data tmp usr/lib lib64

            # Copy application files
            cp ${./pyproject.toml} app/pyproject.toml
            cp ${./main.py} app/main.py

            # Symlink for glibc compatibility
            ln -s ${pkgs.stdenv.cc.cc.lib}/lib/libstdc++.so.6 usr/lib/libstdc++.so.6 || true
          '';

          config = {
            WorkingDir = "/app";
            Env = [
              "PATH=${python}/bin:${pkgs.uv}/bin:${pkgs.coreutils}/bin:${pkgs.bashInteractive}/bin"
              "PYTHONUNBUFFERED=1"
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "LD_LIBRARY_PATH=${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.zlib}/lib"
              "HOME=/tmp"
              "UV_EXTRA_INDEX_URL=https://download.pytorch.org/whl/cpu"
              "UV_INDEX_STRATEGY=unsafe-best-match"
            ];
          };
        };

        # Streamable base image for piping to skopeo
        baseImageStream = pkgs.dockerTools.streamLayeredImage {
          name = "docs-mcp-server-base";
          tag = "latest";

          contents = [
            pkgs.bashInteractive
            pkgs.coreutils
            pkgs.cacert
            python
            pkgs.uv
            pkgs.stdenv.cc.cc.lib
            pkgs.zlib
          ];

          extraCommands = ''
            mkdir -p app data tmp usr/lib lib64
            cp ${./pyproject.toml} app/pyproject.toml
            cp ${./main.py} app/main.py
            ln -s ${pkgs.stdenv.cc.cc.lib}/lib/libstdc++.so.6 usr/lib/libstdc++.so.6 || true
          '';

          config = {
            WorkingDir = "/app";
            Env = [
              "PATH=${python}/bin:${pkgs.uv}/bin:${pkgs.coreutils}/bin:${pkgs.bashInteractive}/bin"
              "PYTHONUNBUFFERED=1"
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "LD_LIBRARY_PATH=${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.zlib}/lib"
              "HOME=/tmp"
              "UV_EXTRA_INDEX_URL=https://download.pytorch.org/whl/cpu"
              "UV_INDEX_STRATEGY=unsafe-best-match"
            ];
          };
        };

      in {
        packages = {
          inherit baseImage baseImageStream;
          default = baseImage;
        };

        devShells.default = pkgs.mkShell {
          packages = [
            python
            pkgs.uv
            pkgs.skopeo
          ];

          shellHook = ''
            export UV_EXTRA_INDEX_URL=https://download.pytorch.org/whl/cpu
            export UV_INDEX_STRATEGY=unsafe-best-match
          '';
        };
      }
    );
}
