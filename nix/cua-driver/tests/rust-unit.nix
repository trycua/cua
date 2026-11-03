# Source-built Linux Rust unit/compile checks.
#
# This is intentionally separate from package.nix: the shipped daemon package
# must remain display-independent, while this check validates the Rust source
# and testkit in Nix's reproducible build environment.
#
# Built with crane in three derivations. `cargoArtifacts` compiles only the
# third-party dependencies from a dummy source tree that holds nothing but
# Cargo.toml and Cargo.lock files, so its store path changes only when a
# manifest or the lockfile changes. The archive derivation then reuses those
# artifacts, compiles just the workspace crates and packs every test binary
# into a cargo-nextest archive. Partition runs execute that archive without
# compiling, so CI fans the tests out over several jobs.
{
  pkgs,
  craneLib,
  src,
  sourceSubdir ? null,
  ...
}:

let
  # Fixed location for sources and build output of the archived tests.
  nextestRoot = "/tmp/cua-nextest";
  manifestDir = if sourceSubdir == null then "." else sourceSubdir;
  rustSrc = if sourceSubdir == null then src else "${src}/${sourceSubdir}";
  pythonClosure = pkgs.closureInfo {
    rootPaths = [ pkgs.python3 ];
  };

  testPackages = [
    "cua-driver"
    "cua-driver-e2e"
    "cua-driver-core"
    "cua-driver-testkit"
    "platform-linux"
  ];
  cargoArgs = "--manifest-path ${manifestDir}/Cargo.toml ${
    pkgs.lib.concatMapStringsSep " " (p: "-p ${p}") testPackages
  } --all-targets";

  commonArgs = {
    pname = "cua-driver-rust-unit-tests";
    version = (pkgs.lib.importTOML "${rustSrc}/Cargo.toml").workspace.package.version;
    inherit src;
    strictDeps = true;
    # Tests bake env!("CARGO_BIN_EXE_*") and env!("CARGO_MANIFEST_DIR") into
    # the binaries as absolute paths. Build and run in the same fixed
    # directory so the archived binaries find them wherever they run.
    CARGO_TARGET_DIR = "${nextestRoot}/target";
    # Test binaries here are never debugged; skipping debuginfo cuts compile
    # and link time. Dependencies and workspace share the setting so the
    # dependency artifacts stay reusable.
    CARGO_PROFILE_DEV_DEBUG = "0";
    cargoVendorDir = craneLib.vendorCargoDeps { cargoLock = "${rustSrc}/Cargo.lock"; };
    nativeBuildInputs = with pkgs; [
      pkg-config
      rustPlatform.bindgenHook
      clang
      python3
    ];
    buildInputs = with pkgs; [
      libx11
      libxi
      libxtst
      libxext
      pipewire
      libei
      # platform-linux resolves niri and libei text against the XKB keymap.
      libxkbcommon
    ];
  };

  # Compile the test targets of every dependency once. `cargo test --no-run`
  # uses the same dev profile and feature set as the test run below, so the
  # artifacts it saves are exactly the ones the next derivation reuses.
  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs
    // {
      buildPhaseCargoCommand = "cargo test --no-run ${cargoArgs}";
      # Compile only; crane's default check phase would rerun a release build.
      doCheck = false;
    }
  );
  testEnv = {
    CUA_TEST_PYTHON = "${pkgs.python3}/bin/python3";
    CUA_TEST_PYTHON_DYNAMIC_LINKER = pkgs.stdenv.cc.bintools.dynamicLinker;
    CUA_TEST_GLIBC_STATIC_LIB = "${pkgs.glibc.static}/lib";
  };

  # Build every test binary once and pack them into a cargo-nextest archive.
  # CI builds this in one job and fans the archive out to partition jobs.
  archive = craneLib.mkCargoDerivation (
    commonArgs
    // testEnv
    // {
      inherit cargoArtifacts;
      pnameSuffix = "-archive";
      doInstallCargoArtifacts = false;
      doCheck = false;
      nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.cargo-nextest ];
      preBuild = ''
        export CUA_TEST_PYTHON_READ_ROOTS="$(paste -sd: ${pythonClosure}/store-paths)"
      '';
      buildPhaseCargoCommand = ''
        mkdir -p $out ${nextestRoot}/src
        cp -r "$PWD/." ${nextestRoot}/src
        chmod -R u+w ${nextestRoot}/src
        cd ${nextestRoot}/src
        cargo nextest archive ${cargoArgs} --archive-file $out/rust-unit.tar.zst
      '';
      installPhaseCommand = "true";
    }
  );

  # Runs one partition of the archived tests. Usable inside the Nix sandbox
  # (the full check below) and on a bare runner (`nix run`) that holds only
  # the archive file, so partition jobs never compile anything.
  runner = pkgs.writeShellApplication {
    name = "cua-rust-unit-run";
    runtimeInputs =
      with pkgs;
      [
        cargo
        rustc
        cargo-nextest
        dbus
        xorg.xorgserver
        python3
        coreutils
      ]
      ++ commonArgs.buildInputs;
    text = ''
      # usage: cua-rust-unit-run ARCHIVE [PARTITION, e.g. count:1/3]
      archive=$1
      partition=''${2:-count:1/1}
      root=${nextestRoot}
      rm -rf "$root"
      mkdir -p "$root/src" "$root/home"
      trap 'rm -rf "$root"' EXIT
      cp -r ${src}/. "$root/src"
      chmod -R u+w "$root/src"
      export HOME=$root/home
      cd "$root/src/${manifestDir}"
      export CUA_TEST_PYTHON=${testEnv.CUA_TEST_PYTHON}
      export CUA_TEST_PYTHON_DYNAMIC_LINKER=${testEnv.CUA_TEST_PYTHON_DYNAMIC_LINKER}
      export CUA_TEST_GLIBC_STATIC_LIB=${testEnv.CUA_TEST_GLIBC_STATIC_LIB}
      CUA_TEST_PYTHON_READ_ROOTS="$(paste -sd: ${pythonClosure}/store-paths)"
      export CUA_TEST_PYTHON_READ_ROOTS
      # Focus unit tests launch private Xvfb servers. The ignored desktop
      # matrix is run by the manual Linux e2e workflow and is not hidden here.
      cargo nextest run \
        --archive-file "$archive" \
        --extract-to "$root" \
        --extract-overwrite \
        --partition "$partition" \
        --no-fail-fast
    '';
  };

  # The whole suite in the sandbox: `nix flake check` and local runs.
  check = pkgs.runCommand "cua-driver-rust-unit-tests" { } ''
    ${runner}/bin/cua-rust-unit-run ${archive}/rust-unit.tar.zst count:1/1
    touch $out
  '';
in
{
  inherit archive runner check;
}
