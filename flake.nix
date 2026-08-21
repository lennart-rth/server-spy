{
  description = "server-spy — system congestion tracker for shared servers";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "server-spy";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            description = "System congestion tracker for shared servers";
            homepage = "https://github.com/lennart-rth/server-spy";
            license = pkgs.lib.licenses.mit;
            mainProgram = "server-spy";
            platforms = [ "x86_64-linux" "aarch64-linux" ];
          };
        };

        checks.test = pkgs.rustPlatform.buildRustPackage {
          pname = "server-spy-tests";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          installPhase = "mkdir -p $out";
        };

        devShells.default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.python3 ];
        };
      });
}
