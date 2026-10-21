{
  description = "SwarmPanel — native Linux client for the Aria music bot fleet command centre";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      overlay = final: prev: {
        swarmpanel = final.callPackage ./nix/package.nix { };
      };
    in
    {
      overlays.default = overlay;
      nixosModules.default = import ./nix/module.nix;
      nixosModules.swarmpanel = import ./nix/module.nix;
    }
    // flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ overlay ];
          config.allowUnfree = true;
        };
      in
      {
        packages.default = pkgs.swarmpanel;
        packages.swarmpanel = pkgs.swarmpanel;

        apps.default = {
          type = "app";
          program = "${pkgs.swarmpanel}/bin/swarmpanel";
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ pkgs.swarmpanel ];
          packages = with pkgs; [
            # Enough to run the test suite headlessly, exactly as CI does.
            xvfb-run
            adwaita-icon-theme
            python3Packages.pygobject-stubs
          ];
          shellHook = ''
            echo "SwarmPanel dev shell — run: xvfb-run -a python3 tests/test_ui_smoke.py"
          '';
        };
      });
}
