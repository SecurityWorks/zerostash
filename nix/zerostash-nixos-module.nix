{ pkgs, config, lib, utils, ... }:
let
  cfg = config.services.zerostash;

  inherit (utils.systemdUtils.unitOptions) unitOption;
in with lib; {
  options.services.zerostash = {
    enable = mkEnableOption "Zerostash automated backups";
    configFile = mkOption {
      description = ''
        TOML configuration file to use for the scheduled backups.
        If <option>configFile</option> is null, the configuration for each backup listed in <option>services.zerostash.backups.<backupName>.repository</option> will be used.

        See <link xlink:href="https://github.com/symmetree-labs/zerostash/blob/master/config.toml.example ">the example file in the repository</link> for options.'';
      type = with types; nullOr path;
      default = null;
    };

    package = mkOption {
      type = types.package;
      default = pkgs.zerostash;
      description = "zerostash package to use.";
    };

    backups = mkOption {
      type = with types;
        attrsOf (submodule ({ ... }: {
          options = {
            paths = mkOption {
              type = with types; listOf path;
              default = [ ];
              description = ''
                The list of paths to be included in the backup.
              '';
              example = [ "/home" "/var" ];
            };

            options = mkOption {
              type = with types; listOf str;
              default = [ "-x" ];
              description = ''
                Options to pass to the <literal>0s</literal> command.
                By default <literal>-x</literal> will not traverse filesystem boundaries.
              '';
              example = [ "-x" "-I" ];
            };

            timerConfig = mkOption {
              type = types.attrsOf unitOption;
              default = { OnCalendar = "daily"; };
              description = ''
                Each attribute in this set specifies an option in the
                <literal>[Timer]</literal> section of the unit.  See
                <citerefentry><refentrytitle>systemd.timer</refentrytitle>
                <manvolnum>5</manvolnum></citerefentry> and
                <citerefentry><refentrytitle>systemd.time</refentrytitle>
                <manvolnum>7</manvolnum></citerefentry> for details.
              '';
              example = {
                OnCalendar = "00:05";
                RandomizedDelaySec = "5h";
              };
            };

            environmentFile = mkOption {
              type = with types; nullOr path;
              default = null;
              description = ''
                File containing the <literal>AWS_ACCESS_KEY_ID</literal> and
                <literal>AWS_SECRET_ACCESS_KEY</literal> for an S3-hosted
                repository, in the format of an <literal>EnvironmentFile</literal>
                as described by <citerefentry>
                <citerefentrytitle>systemd.exec</citerefentrytitle>
                <manvolnum>5</manvolnum></citerefentry>.
              '';
            };

            user = mkOption {
              type = types.str;
              default = "root";
              description = ''
                The username under which to run the backup process.
              '';
            };

            stashName = mkOption {
              type = with types; nullOr str;
              default = null;
              description = ''
                If a <option>configFile</option> is specified, use it
                as the configuration file for the backup operation, and
                <option>stashName</stash> as the target stash.

                This setting is mutually exclusive with <option>stash</option>.
              '';
            };

            stash = mkOption {
              type = with types; nullOr attrs;
              default = null;
              description = ''
                The configuration of the stash to use as backup.
                This setting is mutually exclusive with
                <option>stashName</option>.
              '';
            };
          };
        }));

      default = { };
      example = {
        daily_root_backup = {
          paths = [ "/" ];
          options = [ "-x" ];
          timerConfig = { OnCalendar = "daily"; };
          environmentFile = "/path/to/env/file";
          stash = {
            key = {
              source = "file";
              path = "/path/to/keyfile.toml";
            };
            backend = {
              type = "s3";
              bucket = "test_bucket";
              region = { name = "us-east-1"; };
            };
          };
        };
      };
      description = "Declarative backup configuration.";
    };
  };
  config = mkIf cfg.enable {
    systemd.services = mapAttrs' (name: backup:
      let
        json = cfg: pkgs.writeText "config.json" (builtins.toJSON cfg);
        toml = name: cfg:
          pkgs.runCommand name { }
          "${pkgs.remarshal}/bin/remarshal --of toml ${json cfg} > $out";

        configFile =
          if (backup.stashName != null && cfg.configFile != null) then
            cfg.configFile
          else
            toml "${name}.toml" { stash."${name}" = backup.stash; };
        options = concatStringsSep " " backup.options;
        paths = concatStringsSep " " backup.paths;
        command =
          "${cfg.package}/bin/0s --insecure-config -c ${configFile} commit ${options} ${name} ${paths}";
      in nameValuePair "zerostash-${name}" ({
        restartIfChanged = false;
        serviceConfig = {
          Type = "oneshot";
          ExecStart = command;
          User = backup.user;
          RuntimeDirectory = "zerostash-${name}";
          CacheDirectory = "zerostash-${name}";
          CacheDirectoryMode = "0700";
        } // optionalAttrs (backup.environmentFile != null) {
          EnvironmentFile = backup.environmentFile;
        };
      })) config.services.zerostash.backups;

    systemd.timers = mapAttrs' (name: backup:
      nameValuePair "zerostash-${name}" {
        wantedBy = [ "timers.target" ];
        timerConfig = backup.timerConfig;
      }) config.services.zerostash.backups;
  };
}
