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

      aliases = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "short" ];
        description = ''
          Shorter names this machine also answers to in the search box, so a query can say WHICH
          machine as well as which application -- `thing@short` as well as `thing@long-hostname`.

          A local convention rather than anything derivable: what an estate shortens its hostnames
          to is not a fact this program could work out, which is why it is a value and not a
          default.

          Declared aliases beat prefix matching. Without a declaration, `@ar` already reaches the
          only machine starting with those letters -- convenient until a second one is added, at
          which point a shortcut somebody had relied on for months silently stops working. Naming
          it here is what makes it survive.
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

    subrows = lib.mkOption {
      type = lib.types.attrsOf (lib.types.listOf (lib.types.submodule {
        options = {
          name = lib.mkOption {
            type = lib.types.str;
            description = "The row's label. Short: it sits in a narrow column beside every row.";
          };
          apps = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            description = ''
              Which applications belong on this row, matched case-insensitively as a SUBSTRING of
              an application's id or display name -- `signal` catches `signal-desktop.desktop`
              without anyone needing to know which spelling the package used.

              Declared here rather than dragged in one at a time, because two hundred applications
              is not a drag-and-drop job, and because an arrangement that exists only in a state
              file cannot be reviewed, copied to another machine, or explained. Dragging still
              works and still wins: it writes to placement, which is applied after this.
            '';
          };
        };
      }));
      default = { };
      example = {
        Chat = [
          { name = "biz"; apps = [ "teams" "zoom" ]; }
          { name = "priv"; apps = [ "signal" "telegram" ]; }
        ];
      };
      description = ''
        Named rows INSIDE a box, keyed by folder label.

        A box holding two dozen applications is a list wearing a grid's clothes: the layout stops
        paying for itself the moment a cell is taller than a glance. Sub-rows put the second axis
        back inside the cell, so a folder reads as a few labelled groups rather than one long run.

        Declared rather than derived, because a taxonomy is a judgement -- no rule extracts
        "business" from a set of chat clients. And declared rows are drawn even when empty, which
        is what makes them usable: an invisible row is one nothing can be dragged into.

        Applications nobody has filed keep appearing in unnamed lines, so adding a sub-row never
        hides anything -- it only gives you somewhere to put things.
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

    keyboard = lib.mkOption {
      type = lib.types.enum [ "exclusive" "ondemand" "none" ];
      default = "exclusive";
      description = ''
        How the launcher takes the keyboard.

        `exclusive` is the default because on-demand DOES NOT WORK, and that is a compositor bug
        rather than a matter of taste: on every released sway (1.10–1.12) and its forks, a mapping
        layer surface is granted focus in `handle_map` and then has it revoked again by the
        `arrange_layers` call at the end of that same handler unless its keyboard_interactive is
        EXCLUSIVE. The surface maps and never receives a key. Every shipping launcher defaults to
        exclusive for this reason.

        Set `ondemand` only on a compositor known to have fixed it — it is the nicer behaviour,
        since exclusive holds the keyboard away from the rest of the session while the launcher is
        open.
      '';
    };

    terminal = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "foot" "-e" ];
      description = ''
        argv that wraps a program whose desktop entry declares `Terminal=true` — it draws no window
        of its own and must be given one.

        Not defaulted to any particular emulator on purpose. The right answer is whatever terminal
        this desktop already uses, and a launcher that opened a DIFFERENT one from the rest of the
        session would be wrong in a way nobody would think to look for. Left empty, such programs
        are launched bare, start without a controlling terminal, and exit immediately — which looks
        exactly like a keypress that did nothing.
      '';
    };

    daemon = {
      enable = lib.mkEnableOption ''
        a session service that starts the launcher hidden and leaves it running.

        Residency only pays from the SECOND open onwards, and the first is the one a person
        notices -- the press after login, when nothing is warm. Started at session start, even
        that one is a window being shown rather than a program being launched
      '';

      command = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ "nixlaunch" "--daemon" ];
        example = [ "/usr/bin/nixlaunch" "--daemon" ];
        description = ''
          argv for the resident process.

          A bare name by default, resolved against the unit's own PATH. Hosts that take the
          binary from their distro rather than from this module (see `package`) should give the
          absolute path, for the same reason every other command on this plane does: a unit does
          not inherit a login shell's PATH, and the failure looks like the service silently not
          existing rather than like a missing binary.
        '';
      };
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

    # A UNIT, not a compositor spawn line. A spawn line fires once at session start, so switching
    # a generation into a session that is already running leaves the old process in place -- or no
    # process at all if this was just enabled. As a unit, `home-manager switch` starts and
    # restarts it during activation, which converges the RUNNING session rather than waiting for
    # the next login.
    systemd.user.services = lib.mkIf cfg.daemon.enable {
      nixlaunch = {
        Unit = {
          Description = "nixlaunch, resident and hidden";
          # The launcher needs a compositor to build its surface against, so it cannot usefully
          # start before one exists.
          After = [ "graphical-session.target" ];
          PartOf = [ "graphical-session.target" ];
        };
        Service = {
          ExecStart = lib.escapeShellArgs cfg.daemon.command;
          # NOT Restart=always. A launcher that cannot build its window will fail the same way on
          # every retry, and a restart loop against a broken compositor connection is noise that
          # buries the first, real error.
          Restart = "on-failure";
          RestartSec = 3;
        };
        Install.WantedBy = [ "graphical-session.target" ];
      };
    };

    # `toJSON` on the option values directly, not a hand-assembled string: the Nix types ARE the
    # schema, so anything that type-checks here serialises correctly by construction and there is
    # no second place for the two to disagree.
    xdg.configFile."nixlaunch/config.json".text = builtins.toJSON ({
      inherit (cfg) folders subrows terminal keyboard;
      machines = map (m: { inherit (m) name aliases accent inventory launch; }) cfg.machines;
    } // lib.optionalAttrs (cfg.theme != { }) { inherit (cfg) theme; });
  };
}
