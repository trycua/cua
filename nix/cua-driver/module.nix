# CUA Driver NixOS Module
#
# Installs the cua-driver binary and its runtime dependencies for
# Linux computer-use automation (X11, AT-SPI accessibility, ImageMagick).
#
# Usage in a NixOS configuration:
#   imports = [ ./nix/cua-driver/module.nix ];
#   services.cua-driver = {
#     enable = true;
#     package = cuaDriverPackage;
#   };
#
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.cua-driver;
in
{
  options.services.cua-driver = {
    enable = lib.mkEnableOption "CUA Driver MCP server for computer-use automation";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The cua-driver package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [
      cfg.package
      pkgs.imagemagick # `import` command used by capture.rs for window screenshots
      pkgs.at-spi2-core # AT-SPI bus daemon for accessibility tree queries
    ];

    # D-Bus is required for AT-SPI communication
    services.dbus.enable = true;

    environment.variables = {
      CUA_DRIVER_BIN = "${cfg.package}/bin/cua-driver";
    };
  };
}
