# home/nixlaunch.nix — homeManagerModules.nixlaunch: the launcher's config, declared.
#
# This module renders ONE file, `~/.config/nixlaunch/config.json`, and installs nothing else. That
# is the whole of its job, and the narrowness is deliberate: the binary reads config, discovers its
# own inventory and writes its own placement state, so a module that tried to own any of the other
# two would be racing the program it configures.
#
# ── WHY THIS SCHEMA LOOKS FAMILIAR ────────────────────────────────────────────────────────────
#
# It mirrors `nixremote.launcher`'s options on purpose rather than inventing a second vocabulary
# for the same facts. Those decisions were argued once and are still right:
#
#   * `machines` is a LIST, not an attrset. The order is the column order and it is meaningful; an
#     attrset would silently alphabetise the machines and quietly move the one you open on.
#   * `folders` is a LIST in PRIORITY ORDER for a sharper reason: grouping upstream is
#     first-match-wins, so a specific tag has to be able to precede a broad one. Alphabetising that
#     does not just reorder rows, it changes which row an application lands in.
#   * "Other" needs no entry and cannot be positioned. It is the inbox, not a category — the
#     program appends it and forces it last, so a config physically cannot bury it mid-list.
#
# ── WHAT THIS MODULE DELIBERATELY DOES NOT DO ────────────────────────────────────────────────
#
# It does not know how to find applications. `inventory` is a command, and any command that prints
# the documented JSON will do: `rlaunch --json <host>` today, something else tomorrow, a script
# echoing a fixed string in a test. "What programs exist on a machine" is a question that already
# has owners elsewhere, and a launcher growing a second answer to it would be the wrong place for
# that argument to live.
{ config, lib, pkgs, ... }:
let
  cfg = config.nixlaunch;

  machineModule = { ... }: {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = ''
          The column heading, and the key this machine's placements are stored under.

          Changing it is not cosmetic: the user's saved arrangement is keyed on this string, so a
          rename reads as "a new machine with no arrangement" and the old one's filings are
          orphaned rather than migrated.
        '';
      };

      accent = lib.mkOption {
        type = lib.types.str;
        default = "#22C55E";
        example = "#166534";
        description = ''
          The column's identity colour. Worth spending a real value on: this is what makes a column
          recognisable before its label is read, so it should be the SAME colour that machine's
          window frames and forwarded-window badges already use rather than a new palette.
        '';
      };

      inventory = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        example = [ "rlaunch" "--json" "somehost" ];
        description = ''
          argv that prints this machine's applications as JSON on stdout. See the repository README
          for the contract; `rlaunch --json <host>` already emits it unmodified.

          A LIST, never a shell string. A string would be re-split on spaces by whatever ran it, so
          a path or a machine name containing one would silently become two arguments — and the
          failure would look like an unreachable machine rather than a quoting bug.
        '';
      };

      launch = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "waypipe@somehost" ];
        description = ''
          argv prefix for starting one of this machine's applications. Empty means this machine is
          a READ-ONLY column: it can be browsed and searched but nothing can be started on it,
          which is a legitimate state for a box you want to see the contents of and not drive.
        '';
      };
    };
  };
in
{
  options.nixlaunch = {
    enable = lib.mkEnableOption ''
      nixlaunch, a launcher whose layout is a matrix: machines across, folders down, appsets
      within. Renders the config file only; the binary discovers its own inventory and owns its own
      placement state
    '';

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      defaultText = lib.literalExpression "null";
      description = ''
        The nixlaunch package to install, or null to install nothing and use whatever the host
        already provides.

        Null by default for the same reason `nixremote.forward.package` is: on a foreign distro a
        nixpkgs build of a GPU/display-touching program links against Nix's own graphics libraries
        and can lose sight of the system's real drivers. Choosing one here would be a decision this
        module has no way to make correctly for every host.
      '';
    };

    folders = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "Terminals" "Editors" "Browsers" "Chat" ];
      description = ''
        The rows, IN PRIORITY ORDER. These are the group labels the `inventory` command emits; a
        label it returns that is not listed here falls into the inbox rather than being dropped, so
        an uncategorised application is always still reachable.

        Empty puts everything in the inbox — usable, and exactly what a machine with no category
        table should look like, rather than an error.
      '';
    };

    theme = lib.mkOption {
      type = lib.types.attrsOf (lib.types.either lib.types.str lib.types.int);
      default = { };
      example = {
        ground = "#0A0A0A";
        accent = "#22C55E";
        icon_size = 20;
      };
      description = ''
        Appearance overrides, written verbatim into the config file. Anything omitted keeps the
        program's own default.

        Colours: `ground`, `surface`, `fg`, `muted`, `dim`, `accent`, `error`, `border`.

        Numbers: `icon_size` (default 20, keep it in proportion to your UI font), `line_width`
        (default 4 — apps per line, which is how many left/right steps a row costs before up/down
        is the faster move; more machine columns or a narrower display want fewer),
        `max_height_fraction` (default 0.66 — how much of the display the grid may take before it
        scrolls, so how much of the session stays visible behind it), and `width` (default 560,
        the minimum window width).

        The built-in defaults are a working dark set so the launcher is usable the moment it is
        installed — they are NOT a house palette. Override them with whatever the rest of your
        desktop already uses, so this looks like part of the same product rather than a second one
        that happens to be running.
      '';
    };

    machines = lib.mkOption {
      type = lib.types.listOf (lib.types.submodule machineModule);
      default = [ ];
      description = ''
        The columns, IN ORDER. The first is the one the launcher opens on.

        An empty list is a no-op rather than a failure: the module renders nothing and the binary
        falls back to its own built-in demo data, which is what makes a fresh checkout runnable
        before any of this is filled in.
      '';
    };
  };

  config = lib.mkIf (cfg.enable && cfg.machines != [ ]) {
    assertions = [
      {
        assertion = lib.all (m: m.inventory != [ ]) cfg.machines;
        message =
          "nixlaunch.machines: every machine needs an `inventory` command -- a column with no way "
          + "to be asked what it has renders as permanently unreachable, which is indistinguishable "
          + "from a real outage.";
      }
      {
        assertion =
          let names = map (m: m.name) cfg.machines;
          in lib.length (lib.unique names) == lib.length names;
        message =
          "nixlaunch.machines: names must be unique -- placements are keyed on the name, so two "
          + "columns sharing one would silently share the user's arrangement.";
      }
    ];

    home.packages = lib.optional (cfg.package != null) cfg.package;

    # `toJSON` on the option values directly, not a hand-assembled string: the Nix types ARE the
    # schema, so anything that type-checks here serialises correctly by construction and there is
    # no second place for the two to disagree.
    xdg.configFile."nixlaunch/config.json".text = builtins.toJSON ({
      inherit (cfg) folders;
      machines = map (m: { inherit (m) name accent inventory launch; }) cfg.machines;
    } // lib.optionalAttrs (cfg.theme != { }) { inherit (cfg) theme; });
  };
}
