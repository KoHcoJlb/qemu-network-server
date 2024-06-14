{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, crane, ... }:

  with nixpkgs.lib;
  rec {
    mkPackages = pkgs: let
      craneLib = crane.mkLib pkgs;
    in {
      default = pkgs.callPackage (
        { openssl, pkg-config, rust, stdenv }:

        craneLib.buildPackage ({
          src = craneLib.cleanCargoSource (craneLib.path ./.);

          buildInputs = [ openssl ];
          nativeBuildInputs = [ pkg-config ];
        } // (with rust.envVars; with stdenv; {
          CARGO_BUILD_TARGET = rustHostPlatform;
          "CARGO_TARGET_${hostPlatform.rust.cargoEnvVarTarget}_LINKER" = linkerForHost;
          "CARGO_TARGET_${buildPlatform.rust.cargoEnvVarTarget}_LINKER" = linkerForBuild;
        }))
      ) {};
    };

    pkgsAarch64 = mkPackages (import nixpkgs {
      localSystem = "x86_64-linux";
      crossSystem = "aarch64-linux";
    });

    nixosModules.default = { config, pkgs, ... }:
    let
      svcConfig = config.services.qemu-network-server;
      packages = mkPackages pkgs;
    in {
      options = {
        services.qemu-network-server = {
          enable = mkOption {
            type = types.bool;
            default = false;
          };
          environment = mkOption {
            type = types.attrsOf types.anything;
            default = {};
          };
          environmentFile = mkOption {
            type = types.nullOr types.path;
            default = null;
          };
        };
      };

      config = mkIf svcConfig.enable {
        systemd.services.qemu-network-server = {
          wants = ["network-online.target"];
          after = ["network-online.target"];
          wantedBy = ["multi-user.target"];

          environment = svcConfig.environment;

          serviceConfig = mkMerge [
            {
              ExecStart = "${packages.default}/bin/qemu-network-server";
              Restart = "on-failure";

              AmbientCapabilities = "CAP_NET_ADMIN CAP_NET_RAW";
              CapabilityBoundingSet = "CAP_NET_ADMIN CAP_NET_RAW";
              DeviceAllow = "/dev/net/tun rw";
              ProtectSystem = true;
              ProtectHome = true;
            }
            (mkIf (svcConfig.environmentFile != null) {
              EnvironmentFile = svcConfig.environmentFile;
            })
          ];
        };
      };
    };
  };
}
