# Source-built Linux Rust unit/compile checks.
#
# This is intentionally separate from package.nix: the shipped daemon package
# must remain display-independent, while this check validates the Rust source
# and testkit in Nix's reproducible build environment.
#
# Built with crane in two derivations. `cargoArtifacts` compiles only the
# third-party dependencies from a dummy source tree that holds nothing but
# Cargo.toml and Cargo.lock files, so its store path changes only when a
# manifest or the lockfile changes. The test derivation then reuses those
# artifacts and compiles just the workspace crates, so a source edit no longer
# rebuilds every dependency.
{
  pkgs,
  craneLib,
  src,
  sourceSubdir ? null,
  ...
}:

let
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
in
craneLib.mkCargoDerivation (
  commonArgs
  // {
    inherit cargoArtifacts;
    pnameSuffix = "";
    doInstallCargoArtifacts = false;
    doCheck = false;

    # Transport tests launch an isolated D-Bus session instead of using the host bus.
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
      pkgs.dbus
      pkgs.xorg.xorgserver
    ];
    CUA_TEST_PYTHON = "${pkgs.python3}/bin/python3";
    CUA_TEST_PYTHON_DYNAMIC_LINKER = pkgs.stdenv.cc.bintools.dynamicLinker;
    CUA_TEST_GLIBC_STATIC_LIB = "${pkgs.glibc.static}/lib";
    preBuild = ''
      export CUA_TEST_PYTHON_READ_ROOTS="$(paste -sd: ${pythonClosure}/store-paths)"
    '';

    # Focus unit tests launch private Xvfb servers. The ignored desktop
    # matrix is run by the manual Linux e2e workflow and is not hidden in Nix.
    buildPhaseCargoCommand = "cargo test ${cargoArgs}";
    installPhaseCommand = "touch $out";
  }
)
