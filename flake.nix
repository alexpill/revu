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
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # gh and git are needed at runtime; appended so the user's own gh still wins.
          postInstall = ''
            wrapProgram $out/bin/revu --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.gh pkgs.git ]}
          '';
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
