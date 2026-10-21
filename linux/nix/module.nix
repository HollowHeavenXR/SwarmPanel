# NixOS module: programs.swarmpanel
#
# SwarmPanel's Linux client is a desktop application that talks to a remote
# backend over HTTPS, so this module installs and configures it rather than
# running a service. It deliberately does NOT open firewall ports or start
# daemons -- there is nothing to listen on.

{ config, lib, pkgs, ... }:

let
  cfg = config.programs.swarmpanel;
  inherit (lib) mkEnableOption mkOption mkIf types literalExpression;
in
{
  options.programs.swarmpanel = {
    enable = mkEnableOption "the SwarmPanel desktop client";

    package = mkOption {
      type = types.package;
      default = pkgs.swarmpanel;
      defaultText = literalExpression "pkgs.swarmpanel";
      description = "The SwarmPanel package to install.";
    };

    baseUrl = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "https://swarmpanel.xenusanimations.studio";
      description = ''
        Default backend address for users who have not set one themselves.

        Written to {file}`/etc/swarmpanel/settings.json`, which the client
        reads only as a fallback: a per-user
        {file}`$XDG_CONFIG_HOME/swarmpanel/settings.json` always wins, so
        setting this never overrides a choice a user made in the app.
      '';
    };

    enableKeyring = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Enable gnome-keyring so the session token is stored in the Secret
        Service rather than a 0600 file under the user's config directory.
        The app works either way but reports the weaker storage in its
        Appearance screen.
      '';
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    environment.etc."swarmpanel/settings.json" = mkIf (cfg.baseUrl != null) {
      text = builtins.toJSON { baseURL = cfg.baseUrl; };
      mode = "0444";
    };

    services.gnome.gnome-keyring.enable = mkIf cfg.enableKeyring true;

    # The app stores its token via the Secret Service; without a PAM-unlocked
    # login keyring the user is prompted for the keyring password on every
    # launch, which in practice is what makes people turn the feature off.
    security.pam.services.login.enableGnomeKeyring = mkIf cfg.enableKeyring true;
  };
}
