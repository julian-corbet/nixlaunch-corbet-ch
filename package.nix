# package.nix — the build, kept separate from flake.nix so a consumer can `callPackage` it with
# their own pkgs rather than being forced through this flake's nixpkgs pin.
{ lib
, rustPlatform
, pkg-config
, gtk4
, gtk4-layer-shell
, wrapGAppsHook4
}:

rustPlatform.buildRustPackage {
  pname = "nixlaunch";
  version = "0.1.0";

  src = builtins.path {
    path = ./.;
    name = "nixlaunch-src";
    # Keep the store copy to the things that actually affect the build. Without this every
    # screenshot or scratch file in the working tree becomes part of the derivation's input hash,
    # and the package rebuilds because a note changed.
    filter = path: type:
      let base = baseNameOf path; in
      !(lib.hasPrefix "." base || base == "target" || base == "result");
  };

  cargoLock.lockFile = ./Cargo.lock;

  # pkg-config finds gtk4/gtk4-layer-shell at build time; wrapGAppsHook4 is what makes the RUNTIME
  # work -- a GTK4 program launched without GSETTINGS_SCHEMA_DIR and the icon/theme paths set
  # starts and then fails at the first icon lookup, which looks like a broken icon theme rather
  # than a packaging mistake.
  nativeBuildInputs = [ pkg-config wrapGAppsHook4 ];
  buildInputs = [ gtk4 gtk4-layer-shell ];

  # THE CAIRO RENDERER, BY DEFAULT.
  #
  # GTK4 picks its GSK renderer by probing the GPU, and on a machine with a working Vulkan driver it
  # picks Vulkan. For this program that is the wrong trade in both directions, measured on a
  # three-machine, 191-application inventory:
  #
  #   cairo    settles in 0.52-0.55s, ~560ms CPU, 10 Wayland frames -- every run
  #   vulkan   one run was still burning CPU at 4.6s and had emitted 80 frames when it was killed
  #
  # Vulkan never reaching idle is its own bug and is being chased separately, but the renderer
  # choice is not a workaround for it: a launcher draws boxes, labels and icons, and there is
  # nothing here for a GPU pipeline to do. Bringing one up costs device init, shader compilation and
  # a driver thread pool -- entirely on the path between the keystroke and the window, which is the
  # only latency this program is judged on.
  #
  # `--set-default`, so this is a default and not a decree: GSK_RENDERER in the environment still
  # wins, which is what makes it possible to reproduce the comparison above without rebuilding.
  preFixup = ''
    gappsWrapperArgs+=(--set-default GSK_RENDERER cairo)
  '';

  meta = {
    description = "A Wayland launcher whose layout is a matrix: machines across, folders down, appsets within";
    longDescription = ''
      Every other Wayland launcher is a search box over one list, so the only way it can express
      "which machine" or "which kind of thing" is by making you narrow a single flat set. A screen
      is two-dimensional. nixlaunch uses both axes: columns are machines, rows are folders, and a
      cell is that machine's applications in that folder, arranged as lines. A line is an appset --
      a group you start in one keystroke.

      It does not discover applications itself. Each machine carries a command that prints its
      inventory as JSON, so the launcher works against a local desktop, a remote host over SSH, or
      a script echoing fixed data in a test, without knowing the difference.
    '';
    homepage = "https://github.com/julian-corbet/nixlaunch-corbet-ch";
    license = lib.licenses.mit;
    mainProgram = "nixlaunch";
    platforms = lib.platforms.linux;
  };
}
