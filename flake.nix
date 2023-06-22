{
  inputs = rec {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-23.05";
    utils = { url = "github:numtide/flake-utils"; };
  };

  outputs = { self, nixpkgs, utils, ... }:
    utils.lib.eachDefaultSystem (system:
      let
        overlays = [ ];
        pkgs = import nixpkgs { inherit system overlays; };

        fuseEnabled = pkgs.stdenv.isLinux;
        linuxDeps = with pkgs; [ fuse3 ];
        macDeps = with pkgs; [
          macfuse-stubs
          darwin.apple_sdk.frameworks.Security
        ];

        buildFlags = "-p zerostash -p zerostash-files"
          + pkgs.lib.optionalString fuseEnabled "-p zerostash-fuse";

        features = pkgs.lib.optionals fuseEnabled [ "fuse" ];

        ifTestable = block:
          if (pkgs.stdenv.isLinux && pkgs.stdenv.isx86_64) then
            block
          else
            rec { };

        zstashpkg = pkgs:
          pkgs.rustPlatform.buildRustPackage ({
            meta = with pkgs.lib; {
              description = "Secure, speedy, distributed backups";
              homepage = "https://symmetree.dev";
              license = licenses.mit;
              platforms = platforms.all;
            };

            name = "zerostash";
            pname = "0s";
            src = pkgs.lib.sources.cleanSource ./.;

            cargoLock = { lockFile = ./Cargo.lock; };

            buildFeatures = features;
            cargoCheckFeatures = features;

            cargoBuildFlags = buildFlags;
            cargoTestFlags = buildFlags;

            nativeBuildInputs = with pkgs;
              [ pkg-config ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin macDeps;
            buildInputs = with pkgs;
              [ libusb ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux linuxDeps;
          } // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
            SODIUM_LIB_DIR = "${pkgs.pkgsStatic.libsodium}/lib";
          });
      in rec {
        packages = rec {
          zerostash = zstashpkg pkgs;
          zerostash-static = zstashpkg pkgs.pkgsStatic;

          vm = self.nixosConfigurations.test.config.system.build.vm;

          default = zerostash;
        } // (ifTestable rec {
          nixosTest = import ./nix/nixos-test.nix {
            inherit (self) nixosModule;
            inherit pkgs;
          };
        });

        apps = rec {
          zerostash = utils.lib.mkApp { drv = packages.zerostash; };

          vm = utils.lib.mkApp {
            drv = packages.vm;
            exePath = "/bin/run-nixos-vm";
          };

          default = zerostash;
        } // (ifTestable rec {
          nixosTest = utils.lib.mkApp {
            drv = packages.nixosTest.driver;
            exePath = "/bin/nixos-test-driver";
          };
        });

        devShells.default =
          pkgs.mkShell { inputsFrom = [ self.packages.${system}.default ]; };

        formatter = pkgs.nixfmt;

      }) // {
        nixosModule = { pkgs, ... }: {
          imports = [
            ./nix/zerostash-nixos-module.nix
            {
              nixpkgs.overlays = [
                (_: _: { zerostash = self.packages.${pkgs.system}.zerostash; })
              ];
            }
          ];
        };

        nixosConfigurations.test = nixpkgs.lib.nixosSystem {
          system = "x86_64-linux";
          modules =
            [ self.nixosModule (import ./nix/test-nixos-configuration.nix) ];
        };
      };
}
