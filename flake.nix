{
  description = "Clean caches and temp files that are provably safe to delete";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
      manifest = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package;
    in {
      packages = forAll (pkgs: rec {
        safesweep = pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          version = manifest.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          # The tests need /proc of the host, unshare and a writable /tmp, which the build sandbox lacks.
          doCheck = false;
          meta = {
            description = manifest.description;
            homepage = manifest.repository;
            license = with pkgs.lib.licenses; [ mit asl20 ];
            mainProgram = "safesweep";
            platforms = pkgs.lib.platforms.linux;
          };
        };
        default = safesweep;
      });
    };
}
