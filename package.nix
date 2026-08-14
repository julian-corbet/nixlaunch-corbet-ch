# package.nix — the build, kept separate from flake.nix so a consumer can `callPackage` it with
# their own pkgs rather than being forced through this flake's nixpkgs pin.
{ lib
, stdenv
, rustPlatform
, pkg-config
, gtk4
, gtk4-layer-shell
, wrapGAppsHook4
, clippy
, rustfmt
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
    #
    # `packaging`, `experiments` and `studies` are excluded by name for exactly that reason: the
    # PKGBUILD, the side-shell prototypes and the write-ups are read by people, never by this
    # build, so a commit that only touches one of them costs every consumer a rebuild otherwise.
    filter = path: type:
      let base = baseNameOf path; in
        !(lib.hasPrefix "." base
          || lib.elem base [ "target" "result" "packaging" "experiments" "studies" ]);
  };

  cargoLock.lockFile = ./Cargo.lock;

  # pkg-config finds gtk4/gtk4-layer-shell at build time; wrapGAppsHook4 is what makes the RUNTIME
  # work -- a GTK4 program launched without GSETTINGS_SCHEMA_DIR and the icon/theme paths set
  # starts and then fails at the first icon lookup, which looks like a broken icon theme rather
  # than a packaging mistake.
  #
  # The wrapper sets no renderer. Defaulting GSK_RENDERER to cairo is the PROGRAM's policy, made
  # where every build of it -- Nix, distro, plain cargo -- gets the same answer, and it defers to
  # an inherited GSK_RENDERER. A wrapper argument here would sit above the environment instead of
  # below it, so `GSK_RENDERER=vulkan nixlaunch` would silently keep rendering with cairo and the
  # escape hatch the program documents would exist everywhere except on Nix.
  nativeBuildInputs = [ pkg-config wrapGAppsHook4 ];
  nativeCheckInputs = [ clippy rustfmt ];
  buildInputs = [ gtk4 gtk4-layer-shell ];

  # Keep style and linting in the SAME gate as the build. CI runs `nix flake check`; putting these
  # here means a green package has compiled, tested, formatted, and passed clippy rather than
  # relying on a separate workflow step that can drift away from the derivation developers use.
  postCheck = ''
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features --profile release \
      --target ${stdenv.targetPlatform.rust.rustcTargetSpec} --offline -- -D warnings
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
