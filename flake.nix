{
  description = "A Wayland launcher whose layout is a matrix: machines across, folders down, appsets within";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: {
        nixlaunch = pkgs.callPackage ./package.nix { };
        default = self.packages.${pkgs.system}.nixlaunch;
      });

      # The module takes `pkgs` from the consumer's own home-manager tree, not from this flake's
      # nixpkgs pin -- it installs at most the package a host explicitly hands it, so pinning a
      # second nixpkgs through here would only make a host's closure larger without changing what
      # it gets.
      homeManagerModules = rec {
        nixlaunch = ./home/nixlaunch.nix;
        default = nixlaunch;
      };

      checks = forAll (pkgs: {
        # `cargo test` runs as the package's own checkPhase, so building this IS running the suite.
        build = self.packages.${pkgs.system}.nixlaunch;

        # The module's own behaviour, evaluated -- see checks/module.nix for what and why.
        module = import ./checks/module.nix { inherit pkgs; lib = pkgs.lib; };

        format = pkgs.runCommand "nixlaunch-nix-format"
          {
            nativeBuildInputs = [ pkgs.nixpkgs-fmt ];
            src = ./.;
          } ''
          cp -r "$src" source
          chmod -R u+w source
          cd source
          nixpkgs-fmt --check flake.nix package.nix home/nixlaunch.nix checks/module.nix
          touch "$out"
        '';
      });

      formatter = forAll (pkgs: pkgs.nixpkgs-fmt);
    };
}
