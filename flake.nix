{
  description = "A Wayland launcher whose layout is a matrix: machines across, folders down, appsets within";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # THE COMPOSITOR THIS IS TESTED AGAINST, and it is an input rather than a dependency: nothing
    # here links against it and the package does not need it. `checks/headless-session.sh` runs the
    # launcher inside a real compositor on the headless backend, because every hard bug this program
    # has had was in how it talks to one -- which output it maps on, what size it asks for, how many
    # times it resizes getting there -- and none of that is reachable from a unit test.
    #
    # Scroll specifically, not a more common wlroots compositor, because the behaviour under test IS
    # the compositor's: the exclusive-keyboard workaround in main.rs exists because of sway-fork
    # `arrange_layers` semantics, and the resize storm the check guards against was driven by how
    # `enter-monitor` is delivered. Testing against a sibling implementation would test the wrong one.
    nixscroll.url = "github:julian-corbet/nixscroll-corbet-ch";
  };

  outputs = { self, nixpkgs, nixscroll }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: {
        nixlaunch = pkgs.callPackage ./package.nix { };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.nixlaunch;
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
        build = self.packages.${pkgs.stdenv.hostPlatform.system}.nixlaunch;

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

      # AN APP, DELIBERATELY NOT A CHECK. It starts a compositor and a session bus, which a build
      # sandbox is a poor and flaky home for -- and a flaky entry in `nix flake check` would punish
      # every contributor building this from a cold cache for a fault in the harness rather than in
      # the program. It is run by the builder that has the store warm, on a machine with a real
      # /tmp: `nix run .#headless-session`.
      apps = forAll (pkgs:
        let
          system = pkgs.stdenv.hostPlatform.system;
        in
        nixpkgs.lib.optionalAttrs (nixscroll.packages ? ${system}) {
          headless-session =
            let
              runner = pkgs.writeShellScriptBin "nixlaunch-headless-session" ''
                export PATH=${nixpkgs.lib.makeBinPath [
                  pkgs.bash pkgs.coreutils pkgs.dbus pkgs.python3 pkgs.wtype
                  nixscroll.packages.${system}.scroll
                ]}:$PATH
                export DBUS_SESSION_CONF=${pkgs.dbus}/share/dbus-1/session.conf
                exec ${pkgs.bash}/bin/bash ${./checks/headless-session.sh} \
                  ${self.packages.${system}.nixlaunch}/bin/nixlaunch \
                  ${nixscroll.packages.${system}.scroll}/bin/scroll "$@"
              '';
            in
            {
              type = "app";
              program = "${runner}/bin/nixlaunch-headless-session";
            };
        });

      formatter = forAll (pkgs: pkgs.nixpkgs-fmt);
    };
}
