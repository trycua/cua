{
  description = "CUA - Computer Use Agent";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
      ]
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };

          rustSrc = ./libs/cua-driver/rust;

          cuaDriverPackage = import ./nix/cua-driver/package.nix {
            inherit pkgs;
            src = rustSrc;
          };
        in
        {
          packages = {
            cua-driver = cuaDriverPackage;
            default = cuaDriverPackage;
          };

          checks =
            {
              cua-driver-build = cuaDriverPackage;
            }
            // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
              # NixOS VM integration test (x86_64-linux only)
              cua-driver-integration = import ./nix/cua-driver/tests/integration.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };

              # Same test but captures a VM screenshot at the end
              cua-driver-screenshot = import ./nix/cua-driver/tests/screenshot.nix {
                inherit pkgs;
                inherit (pkgs) lib;
                cuaDriverModule = {
                  imports = [ ./nix/cua-driver/module.nix ];
                  services.cua-driver.package = cuaDriverPackage;
                };
              };
            };
        }
      )
    // {
      # NixOS module — consumers must set services.cua-driver.package
      # (or use the per-system package from self.packages)
      nixosModules.cua-driver = ./nix/cua-driver/module.nix;
    };
}
