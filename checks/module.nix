# checks/module.nix — what the home-manager module actually RENDERS, not merely what it accepts.
#
# Asserting on option values would prove nothing interesting: they are what you just set. The thing
# that can be wrong is the file, so every check below parses the rendered JSON back and looks at
# it, which is the same artefact the binary reads.
{ pkgs, lib }:

let
  # A minimal stand-in for the home-manager options this module writes into, so the module can be
  # evaluated on its own. Cheaper and far more precise than instantiating home-manager: a failure
  # here is about this module, never about somebody else's.
  stubs = {
    options = {
      home.packages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [ ];
      };
      xdg.configFile = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule {
          options.text = lib.mkOption { type = lib.types.str; };
        });
        default = { };
      };
      assertions = lib.mkOption {
        type = lib.types.listOf lib.types.unspecified;
        default = [ ];
      };
      systemd.user.services = lib.mkOption {
        type = lib.types.attrsOf lib.types.unspecified;
        default = { };
      };
    };
  };

  eval = module: (lib.evalModules { modules = [ ../home/nixlaunch.nix stubs module ]; }).config;

  rendered = module:
    let c = eval module; in
    builtins.fromJSON c.xdg.configFile."nixlaunch/config.json".text;

  # Assertions are collected, not thrown, so a check can ask whether one WOULD fire without the
  # evaluation aborting first.
  failures = module: map (a: a.message) (lib.filter (a: !a.assertion) (eval module).assertions);

  base = {
    nixlaunch.enable = true;
    nixlaunch.folders = [ "Terminals" "Editors" ];
    nixlaunch.machines = [
      { name = "alpha"; accent = "#166534"; inventory = [ "inv" "--json" "alpha" ]; }
      { name = "beta"; inventory = [ "inv" "--json" "beta" ]; launch = [ "run-on" "beta" ]; }
    ];
  };

  check = name: cond: detail:
    if cond then "ok ${name}"
    else throw "FAILED ${name}: ${detail}";

  results = [
    (check "column order survives rendering"
      (map (m: m.name) (rendered base).machines == [ "alpha" "beta" ])
      "the first machine is the one the launcher opens on, so a list that reorders is a different launcher")

    (check "row order survives rendering"
      ((rendered base).folders == [ "Terminals" "Editors" ])
      "grouping is first-match-wins upstream; reordering rows changes which row an app lands in")

    (check "accent defaults without being stated"
      ((builtins.elemAt (rendered base).machines 1).accent == "#22C55E")
      "a machine that names no colour must still get one")

    (check "a machine with no launch command renders a read-only column"
      ((builtins.elemAt (rendered base).machines 0).launch == [ ])
      "browsable-but-not-startable is a legitimate state, not an error")

    (check "inventory stays a list"
      ((builtins.elemAt (rendered base).machines 0).inventory == [ "inv" "--json" "alpha" ])
      "a shell string would be re-split on spaces and the failure would look like an unreachable machine")

    (check "inventory timeout reaches the rendered contract"
      ((builtins.elemAt (rendered base).machines 0).inventory_timeout_ms == 5000)
      "an unbounded external command can hang a cold launcher forever")

    (check "fractional theme values render"
      ((rendered (lib.recursiveUpdate base {
        nixlaunch.theme.max_height_fraction = 0.5;
      })).theme.max_height_fraction == 0.5)
      "the Rust schema uses f64; a module that only accepts strings and integers cannot configure it")

    (check "key overrides and focus policy render"
      (
        let
          r = rendered (lib.recursiveUpdate base {
            nixlaunch.keys."ctrl+j" = "move-down";
            nixlaunch.surface = "window";
            nixlaunch.exit_on_focus_loss = false;
          });
        in
        r.keys."ctrl+j" == "move-down" && r.surface == "window" && !r.exit_on_focus_loss
      )
      "public options must reach the exact JSON the binary reads")

    (check "library folder mode reaches the rendered contract"
      ((rendered (lib.recursiveUpdate base {
        nixlaunch.folderModes.Games = "library";
        nixlaunch.folders = [ "Terminals" "Editors" "Games" ];
      })).folder_modes.Games == "library")
      "a library row must survive module rendering so the binary can suppress wrapping and bulk launch")

    # The module writes nothing at all rather than an empty config, so a fresh checkout falls back
    # to the binary's own demo data instead of rendering an empty launcher.
    (check "no machines renders no file"
      (!((eval { nixlaunch.enable = true; }).xdg.configFile ? "nixlaunch/config.json"))
      "an empty machine list should be a no-op, not an empty launcher")

    (check "a machine with no inventory command is refused"
      (failures
        {
          nixlaunch.enable = true;
          nixlaunch.machines = [{ name = "mute"; inventory = [ ]; }];
        } != [ ])
      "a column that cannot be asked what it has is indistinguishable from a real outage")

    (check "duplicate machine names are refused"
      (failures
        {
          nixlaunch.enable = true;
          nixlaunch.machines = [
            { name = "same"; inventory = [ "a" ]; }
            { name = "same"; inventory = [ "b" ]; }
          ];
        } != [ ])
      "placements are keyed on the name, so duplicates would silently share the user's arrangement")

    (check "case-insensitive name and alias collisions are refused"
      (failures
        {
          nixlaunch.enable = true;
          nixlaunch.machines = [
            { name = "Server"; aliases = [ "nas" ]; inventory = [ "a" ]; }
            { name = "nas"; inventory = [ "b" ]; }
          ];
        } != [ ])
      "the search box resolves case-insensitively and takes the first match")

    (check "folder mode for an undeclared folder is refused"
      (failures
        {
          nixlaunch.enable = true;
          nixlaunch.folders = [ "Files" ];
          nixlaunch.folderModes.Games = "library";
          nixlaunch.machines = [{ name = "box"; inventory = [ "inv" ]; }];
        } != [ ])
      "a mode for a row that cannot render would otherwise be accepted and silently ignored")

    (check "daemon service renders the bounded lifecycle policy"
      (
        let
          service = (eval (lib.recursiveUpdate base {
            nixlaunch.daemon.enable = true;
          })).systemd.user.services.nixlaunch;
        in
        service.Service.KillMode == "process"
        && service.Service.ExecStart == "/usr/bin/nixlaunch --daemon"
        && service.Unit.StartLimitBurst == 5
      )
      "daemon support must be covered by the module harness that previously lacked its option stub")
  ];
in
pkgs.runCommand "nixlaunch-module-checks" { passthru.results = results; } ''
  ${lib.concatMapStringsSep "\n" (r: "echo ${lib.escapeShellArg r}") results}
  touch $out
''
