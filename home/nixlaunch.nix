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

      inventory_timeout_ms = lib.mkOption {
        type = lib.types.ints.positive;
        default = 5000;
        description = ''
          Maximum wall-clock time for this inventory command. An unreachable machine is a normal
          column state; one command that never returns must not hold the launcher indefinitely.
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

          Use [ "{}" ] for the local machine: the placeholder is replaced by the application's
          argv without adding a forwarding prefix. Empty never falls back to local execution,
          because doing so on a remote column would start the same-named program on the wrong host.
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
        an uncategorised application is always still reachable. Usage may reorder a row's contents,
        but never the configured rows themselves.

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
      type = lib.types.submodule {
        options = {
          ground = lib.mkOption { type = lib.types.str; default = "#0A0A0A"; };
          surface = lib.mkOption { type = lib.types.str; default = "#0E0E0E"; };
          fg = lib.mkOption { type = lib.types.str; default = "#F0F0F0"; };
          muted = lib.mkOption { type = lib.types.str; default = "#999999"; };
          dim = lib.mkOption { type = lib.types.str; default = "#444444"; };
          accent = lib.mkOption { type = lib.types.str; default = "#22C55E"; };
          error = lib.mkOption { type = lib.types.str; default = "#B91322"; };
          border = lib.mkOption { type = lib.types.str; default = "#1C1C1C"; };
          logo = lib.mkOption { type = lib.types.str; default = ""; };
          icon_size = lib.mkOption { type = lib.types.ints.positive; default = 20; };
          logo_size = lib.mkOption { type = lib.types.ints.positive; default = 28; };
          line_width = lib.mkOption { type = lib.types.ints.positive; default = 4; };
          width = lib.mkOption { type = lib.types.ints.positive; default = 560; };
          max_height_fraction = lib.mkOption {
            type = lib.types.addCheck (lib.types.either lib.types.int lib.types.float)
              (value: value > 0 && value <= 1);
            default = 0.66;
          };
          max_width_fraction = lib.mkOption {
            type = lib.types.addCheck (lib.types.either lib.types.int lib.types.float)
              (value: value > 0 && value <= 1);
            default = 0.9;
          };
        };
      };
      default = { };
      example = {
        ground = "#0A0A0A";
        accent = "#22C55E";
        icon_size = 20;
      };
      description = ''
        Appearance values, type-checked here and rendered into the config file.

        Colours: `ground`, `surface`, `fg`, `muted`, `dim`, `accent`, `error`, `border`.

        `logo` puts an image in the corner the label columns leave empty -- an absolute path, or an
        icon name resolved against your theme. Empty by default: a launcher that shipped somebody
        else's mark would be wearing it. `logo_size` (default 28) sizes it, separately from
        `icon_size` because the corner has a header row to fill while an application icon has to
        sit inside a line of text.

        Numbers: `icon_size` (default 20, keep it in proportion to your UI font), `line_width`
        (default 4 — apps per line, which is how many left/right steps a row costs before up/down
        is the faster move; more machine columns or a narrower display want fewer),
        `max_height_fraction` (default 0.66 — how much of the display height the grid may take),
        `max_width_fraction` (default 0.9 — the corresponding width cap), and `width` (default 560,
        the minimum window width). Both fractions must be greater than zero and at most one.

        The built-in defaults are a working dark set so the launcher is usable the moment it is
        installed — they are NOT a house palette. Override them with whatever the rest of your
        desktop already uses, so this looks like part of the same product rather than a second one
        that happens to be running.
      '';
    };

    surface = lib.mkOption {
      type = lib.types.enum [ "layer" "window" ];
      default = "layer";
      description = "Layer-shell surface for normal use, or an ordinary window for debugging.";
    };

    exit_on_focus_loss = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Hide the launcher when keyboard focus goes elsewhere. A short grace period is restarted on
        every reveal so focus bounce from a bar or dock does not immediately dismiss it.
      '';
    };

    keys = lib.mkOption {
      type = lib.types.attrsOf (lib.types.nullOr (lib.types.enum [
        "move-left"
        "move-right"
        "move-up"
        "move-down"
        "enter"
        "launch-line"
        "launch-cell"
        "launch-selection"
        "toggle-inside"
        "go-outside"
        "cancel"
        "backspace"
      ]));
      default = { };
      example = {
        "ctrl+j" = "move-down";
        "shift+return" = "launch-selection";
        "ctrl+return" = "launch-cell";
      };
      description = ''
        Chord-to-action overrides. They extend the defaults; null unbinds a default chord. Explicit
        launch-line and launch-cell actions mean the same thing in either focus mode, while
        launch-selection preserves Shift+Return's contextual default (cell outside, line inside).
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
        are refused with an error rather than launched without a visible window.
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
        default =
          if cfg.package != null
          then [ "${cfg.package}/bin/nixlaunch" "--daemon" ]
          else [ "/usr/bin/nixlaunch" "--daemon" ];
        defaultText = lib.literalExpression ''
          if config.nixlaunch.package != null
          then [ "''${config.nixlaunch.package}/bin/nixlaunch" "--daemon" ]
          else [ "/usr/bin/nixlaunch" "--daemon" ]
        '';
        example = [ "/opt/nixlaunch/bin/nixlaunch" "--daemon" ];
        description = ''
          argv for the resident process.

          ALWAYS ABSOLUTE, because a systemd unit does not inherit a login shell's PATH, and the
          failure mode of getting that wrong is not a missing binary — it is `status=203/EXEC`
          every three seconds, forever, in a unit nobody is looking at.

          The default derives from `package`, which is the only thing that actually knows where
          the binary is: set it, and this is that store path, with the dependency the store path
          implies; leave it null and the host is providing its own, which every Linux distribution
          puts in `/usr/bin`. Both halves are right without anyone having to say so, which matters
          because the same values file usually serves hosts of both kinds — one absolute path
          written by hand is correct on some of them and silently wrong on the rest.
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
          let
            tokens = lib.concatMap
              (m: map lib.toLower ([ m.name ] ++ m.aliases))
              cfg.machines;
          in
          lib.length (lib.unique tokens) == lib.length tokens;
        message =
          "nixlaunch.machines: names and aliases must be unique case-insensitively -- the search "
          + "box resolves them case-insensitively and takes the first match, so a collision makes "
          + "one column unreachable by name.";
      }
      {
        assertion = lib.all
          (folder: folder != "Other" && lib.elem folder cfg.folders)
          (lib.attrNames cfg.subrows);
        message =
          "nixlaunch.subrows: every key must name a configured folder other than Other -- an "
          + "unknown key is otherwise silently discarded.";
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

    # A UNIT, not a compositor spawn line. That gives the session one supervised resident process
    # and starts it when the feature is first enabled. Ordinary config changes do not need a
    # service restart: each reveal re-reads the model inputs from config.json. Settings bound to
    # GTK or the layer surface take effect at the next ordinary process start.
    systemd.user.services = lib.mkIf cfg.daemon.enable {
      nixlaunch = {
        Unit = {
          Description = "nixlaunch, resident and hidden";
          # The launcher needs a compositor to build its surface against, so it cannot usefully
          # start before one exists.
          After = [ "graphical-session.target" ];
          PartOf = [ "graphical-session.target" ];

          # AND IT GIVES UP. `Restart=on-failure` with no limit is an infinite loop, which is not
          # what "on failure" sounds like it means: a permanently broken ExecStart -- a wrong path,
          # a binary that is not installed -- retries every RestartSec until the session ends.
          # Observed, not theorised: a daemon command that was absolute but pointed at the wrong
          # distribution's location restarted 14,723 times over about twelve hours, on a host
          # nobody happened to be looking at, and the only evidence was a line in the journal that
          # said the same thing 14,723 times.
          #
          # Five attempts is generous for the transient case this restart exists for -- a
          # compositor socket that is not up yet -- and instantly diagnostic for the permanent one,
          # because a unit in `failed` is visible and a unit in `activating (auto-restart)` looks
          # for all the world like it is starting.
          StartLimitIntervalSec = 60;
          StartLimitBurst = 5;
        };
        Service = {
          ExecStart = lib.escapeShellArgs cfg.daemon.command;
          # NOT Restart=always. A launcher that cannot build its window will fail the same way on
          # every retry, and a restart loop against a broken compositor connection is noise that
          # buries the first, real error.
          Restart = "on-failure";
          RestartSec = 3;

          # KILL THE LAUNCHER, NOT WHAT IT LAUNCHED.
          #
          # systemd's default is `control-group`: stopping a unit kills every process in its
          # cgroup. Applications started from a resident launcher are in that cgroup, so every
          # restart of this unit also killed everything the user had opened through it -- including
          # forwarded sessions to other machines, which take a visible moment to rebuild and are
          # the last thing anyone expects a launcher restart to touch.
          #
          # The normal launch path asks systemd for a transient user scope, which moves the
          # application into a different cgroup. Its fallback only creates a new process group;
          # systemd does not kill by process group, and the two mechanisms are not interchangeable.
          # Keeping KillMode=process protects that fallback on an older or degraded user session.
          KillMode = "process";
        };
        Install.WantedBy = [ "graphical-session.target" ];
      };
    };

    # `toJSON` on the option values directly, not a hand-assembled string: the Nix types ARE the
    # schema, so anything that type-checks here serialises correctly by construction and there is
    # no second place for the two to disagree.
    xdg.configFile."nixlaunch/config.json".text = builtins.toJSON ({
      inherit (cfg) folders subrows terminal keyboard surface exit_on_focus_loss keys theme;
      machines = map
        (m: { inherit (m) name aliases accent inventory inventory_timeout_ms launch; })
        cfg.machines;
    });
  };
}
