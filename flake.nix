{
  description = "memori: explains where your RAM is going";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      forAllSystems =
        f: lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (system: f nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: _prev: {
        memori = final.callPackage ./nix/package.nix { };
      };

      packages = forAllSystems (pkgs: rec {
        memori = pkgs.callPackage ./nix/package.nix { };
        default = memori;
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.memori;
          meta.description = "Print a report of where RAM is going";
        };
      });

      checks = forAllSystems (
        pkgs:
        let
          memori = self.packages.${pkgs.stdenv.hostPlatform.system}.memori;
        in
        {
          # Builds and runs the unit tests.
          inherit memori;

          clippy = memori.overrideAttrs (old: {
            pname = "${old.pname}-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = ''
              runHook preBuild
              cargo clippy --offline --all-targets -- --deny warnings
              runHook postBuild
            '';
            doCheck = false;
            installPhase = "touch $out";
          });

          rustfmt =
            pkgs.runCommand "memori-rustfmt"
              {
                nativeBuildInputs = [
                  pkgs.cargo
                  pkgs.rustfmt
                ];
              }
              ''
                cd ${memori.src}
                cargo fmt --check
                touch $out
              '';
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.memori ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
