# SwarmPanel for Linux

Native Linux client for [SwarmPanel](https://swarmpanel.xenusanimations.studio),
the command centre for Aria and the 12-node music bot fleet. Same backend as
the web panel, the iPhone app and the Apple TV app — this is the GTK4 /
libadwaita port, built for NixOS.

It speaks exactly the same contract as `ios/`:

- bearer-token auth against `/api/*`, with snake_case bodies and
  `{"detail": ...}` error payloads
- one multiplexed `/ws` connection, watch-keyed per screen
  (`dashboard`, `control_state`, `friends`, `threads`, `thread_messages`,
  `notifications`, `account`, `community_counts`, `stability`, `events`,
  `lumisound_admin`, …)
- the information architecture from `lua/src/nav.lua`, including its
  permission gates, so the sidebar matches the web panel screen for screen

## Install (NixOS, flake)

```nix
{
  inputs.swarmpanel.url = "path:/home/desmondc/Documents/Discord Bots/SwarmPanel/linux";

  outputs = { nixpkgs, swarmpanel, ... }: {
    nixosConfigurations.nixos = nixpkgs.lib.nixosSystem {
      modules = [
        swarmpanel.nixosModules.default
        { nixpkgs.overlays = [ swarmpanel.overlays.default ];
          programs.swarmpanel.enable = true; }
      ];
    };
  };
}
```

Then `nixos-rebuild switch`. SwarmPanel appears in your app launcher.

Module options:

| Option | Default | Meaning |
| --- | --- | --- |
| `programs.swarmpanel.enable` | `false` | Install the client. |
| `programs.swarmpanel.package` | `pkgs.swarmpanel` | Package to install. |
| `programs.swarmpanel.baseUrl` | `null` | System-wide default backend. A per-user setting always wins. |
| `programs.swarmpanel.enableKeyring` | `true` | Store the session token in the Secret Service, PAM-unlocked at login. |

There is no service and no open port: this is a desktop client that talks
*out* to the backend over HTTPS.

## Run without installing

```sh
nix run .            # from this directory
```

## After editing the port

The system flake reaches this directory through a `swarmpanel-src` input, and
`flake.lock` pins it by content hash. Refresh it before rebuilding, or the
rebuild reuses the previously locked source and appears to do nothing:

```sh
sudo nix flake update swarmpanel-src --flake /etc/nixos
sudo nixos-rebuild switch --flake /etc/nixos#nixos
```

Check it took effect — the store path should change:

```sh
readlink -f "$(command -v swarmpanel)"
```

## Settings precedence

The backend address resolves in this order, most specific first:

1. `$XDG_CONFIG_HOME/swarmpanel/settings.json` — what the app writes when you
   change the server address in the UI
2. `/etc/swarmpanel/settings.json` — `programs.swarmpanel.baseUrl`
3. the built-in default, `https://swarmpanel.xenusanimations.studio`

The session token is stored in the Secret Service when one is available, and
otherwise in a `0600` file next to the settings. The Appearance screen always
reports which of the two is in use, so the weaker storage is never silent.

## Development

```sh
nix develop
python3 -m swarmpanel                        # run against the real backend
xvfb-run -a python3 tests/test_ui_smoke.py   # build the window, visit all 19 screens
python3 tests/test_api.py                    # REST contract
python3 tests/test_live.py                   # /ws protocol
```

`tests/test_live.py` and `tests/test_api.py` run against stand-in servers that
implement the real protocol, so they need no backend and no credentials.
`tests/test_ui_smoke.py` additionally fails on any GTK or GLib warning.

## Layout

```
swarmpanel/core/     api.py  live.py  nav.py  casing.py  config.py  secrets.py  work.py
swarmpanel/ui/       app.py  window.py  auth.py  widgets.py  style.css
swarmpanel/ui/screens/   fleet  insights  community  account  admin   (+ base.py)
nix/                 package.nix  module.nix
data/                desktop entry, metainfo, icon
```

`core/` is UI-free and independently testable; `ui/screens/base.py` owns the
loading/error/content lifecycle, the live watch, and the REST fallback poll
that runs only while the socket is down.

## Copyright

(c) HeavenlyXenusVR. Discord: <https://discord.com/users/1304564041863266347>
