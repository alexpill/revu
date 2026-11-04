{
  description = "revu: review GitHub pull requests from the terminal";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "revu";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          # Only what the build needs, so docs edits don't trigger rebuilds.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ];
          };
          # gh and git are expected on PATH: bundling them added ~400 MiB for tools
          # every user already has (and gh needs a login anyway).
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            description = "Terminal UI to triage and review GitHub pull requests";
            homepage = "https://github.com/alexpill/revu";
            license = with pkgs.lib.licenses; [ mit asl20 ];
            mainProgram = "revu";
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = [ pkgs.clippy pkgs.rustfmt pkgs.rust-analyzer ];
        };
      });
    };
}
