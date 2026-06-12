# Nexus-BS Compiled Distribution

This folder contains a minimal compiled Nexus-BS deployment bundle for a
Linux/aarch64 target.

Build included: `v0.1.62-69-gfb39e15`

## Contents

```text
bin/
  nexus-bs
  nexus-bs-control-service
  nexus-bs-dashboard

config/
  config.toml
  config.toml.fallback

dashboard/
  index.html
  assets/

systemd/
  nexus-bs@.service
  nexus-bs-control@.service
  nexus-bs-dashboard@.service
  journald-nexus-bs-volatile.conf
```

The main project license is PolyForm Noncommercial 1.0.0. Commercial use is by
written agreement only. Upstream notices are kept in the project root, not
duplicated in this binary bundle.

## SHA-256 Checksums

Run checksum verification from inside `compiled_distribution/`. The README file
itself is not listed because embedding its checksum would change the checksum.

```text
ee49139735f8652834b72667fdce02499a74c165ad002a392033477a7f82b263  bin/nexus-bs
72d01f6fbe919854f776b5204daf85e4e109015f84596f4456bf8ece9d69457d  bin/nexus-bs-control-service
eee3eece1a9e131e3c5ac2e636850ee939740961102156968f594496f83614b5  bin/nexus-bs-dashboard
3903a96a4ac0f3e770849876520a712a023998d9853eb70690f011da88d01c4d  config/config.toml
3903a96a4ac0f3e770849876520a712a023998d9853eb70690f011da88d01c4d  config/config.toml.fallback
68d3602033de6174247d9bbfd84e98e26f5c993637478f97a66039b34f033902  dashboard/index.html
59bd4f552b714ff3ec4d3226ce793994db33753dfb18098942e2468afceb0519  dashboard/assets/app.js
c333c9921916e0d8ee28cbe00b2931b00c3ccdf0c220934e130a56121b9b1930  dashboard/assets/styles.css
92d6127820d318a5df91bc60c60297e3b74068ff322e13c5b651fdee4075e9a5  dashboard/assets/nexus-bs-logo.svg
4a8db4661e48b6555912e1e6114ed2205ef5c9ffbeed997464a3d3dd7a5e06d0  dashboard/assets/nexus-bs-logo.png
21773b5621de3f15eff10559d5dfa95dff0d54a51ffbd693f66cfcabdcd5e478  systemd/nexus-bs@.service
fb14a7b7b7ec02037ef8a59c6620264fc5cb76ef9af65e44a0b20b9fb3af9a1a  systemd/nexus-bs-control@.service
43978753233599c2235cc90c1af7f66e09b7039a0c0ae04d7341e97f7cfae219  systemd/nexus-bs-dashboard@.service
5416b165bd62b12fb1b655dcc05c0f6aaa54c0f92c39d0bb4fcf937439077c16  systemd/journald-nexus-bs-volatile.conf
```

## Target Requirements

- 64-bit ARM Linux system with systemd.
- A working SoapySDR runtime and the SoapySDR hardware module for your SDR.
- A configured SDR device supported by the `config.toml` RF section.
- A user account that will run Nexus-BS, for example `chris`.
- Root privileges for installing systemd units.

Do not build Nexus-BS on the target radio computer. This bundle already contains
the compiled aarch64 binaries.

## Install From Zero

Set the target user name first:

```sh
export NEXUS_USER=chris
```

Create the install directory:

```sh
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 /home/"$NEXUS_USER"/nexus-bs
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 /home/"$NEXUS_USER"/nexus-bs/dashboard
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 /home/"$NEXUS_USER"/nexus-bs/dashboard/assets
```

Copy binaries:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 bin/nexus-bs /home/"$NEXUS_USER"/nexus-bs/nexus-bs
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 bin/nexus-bs-control-service /home/"$NEXUS_USER"/nexus-bs/nexus-bs-control-service
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 bin/nexus-bs-dashboard /home/"$NEXUS_USER"/nexus-bs/nexus-bs-dashboard
```

Copy config:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0600 config/config.toml /home/"$NEXUS_USER"/nexus-bs/config.toml
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0600 config/config.toml.fallback /home/"$NEXUS_USER"/nexus-bs/config.toml.fallback
```

Copy dashboard assets:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/index.html /home/"$NEXUS_USER"/nexus-bs/dashboard/index.html
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/app.js /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/app.js
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/styles.css /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/styles.css
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/nexus-bs-logo.svg /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/nexus-bs-logo.svg
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/nexus-bs-logo.png /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/nexus-bs-logo.png
```

Install systemd units:

```sh
sudo install -m 0644 systemd/nexus-bs@.service /etc/systemd/system/nexus-bs@.service
sudo install -m 0644 systemd/nexus-bs-control@.service /etc/systemd/system/nexus-bs-control@.service
sudo install -m 0644 systemd/nexus-bs-dashboard@.service /etc/systemd/system/nexus-bs-dashboard@.service
```

Optional volatile journald config:

```sh
sudo install -m 0644 systemd/journald-nexus-bs-volatile.conf /etc/systemd/journald.conf.d/nexus-bs-volatile.conf
sudo systemctl restart systemd-journald
```

Reload systemd and enable services:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now nexus-bs-control@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs-dashboard@"$NEXUS_USER".service
```

## Configure Before Transmitting

Edit:

```sh
sudoedit /home/"$NEXUS_USER"/nexus-bs/config.toml
```

At minimum, verify:

- `[phy_io.soapysdr]` frequencies, device selection, antennas and gains.
- `[net_info]` MCC/MNC.
- `[cell_info]` carrier plan, location area, colour code and local SSI/GSSI policy.
- `[brew]` credentials if Brew is used. Do not leave placeholder credentials active.
- `[dashboard]` username/password if the dashboard is reachable by other users.

The shipped example keeps the BlueStation reference RF/frequency profile. Change
it only to match your licensed RF plan and hardware.

## Verify

Check services:

```sh
systemctl status nexus-bs-control@"$NEXUS_USER".service
systemctl status nexus-bs@"$NEXUS_USER".service
systemctl status nexus-bs-dashboard@"$NEXUS_USER".service
```

Check recent logs:

```sh
journalctl -u nexus-bs@"$NEXUS_USER".service -n 120 --no-pager
journalctl -u nexus-bs-dashboard@"$NEXUS_USER".service -n 80 --no-pager
```

Open the dashboard:

```text
http://<target-ip>:8080
```

The core dashboard API listens on `127.0.0.1:18080`; the public browser service
is `nexus-bs-dashboard@USER.service` on port `8080`.

## Update An Existing Install

Stop services:

```sh
sudo systemctl stop nexus-bs-dashboard@"$NEXUS_USER".service
sudo systemctl stop nexus-bs@"$NEXUS_USER".service
sudo systemctl stop nexus-bs-control@"$NEXUS_USER".service
```

Repeat the binary, dashboard asset and systemd copy steps above. Do not overwrite
`/home/$NEXUS_USER/nexus-bs/config.toml` unless you intentionally want to replace
the live configuration.

Restart:

```sh
sudo systemctl daemon-reload
sudo systemctl start nexus-bs-control@"$NEXUS_USER".service
sudo systemctl start nexus-bs@"$NEXUS_USER".service
sudo systemctl start nexus-bs-dashboard@"$NEXUS_USER".service
```

## Troubleshooting

If `nexus-bs` fails at startup, check that SoapySDR and the hardware module are
installed on the target and that `config.toml` selects the right SDR device.

If the dashboard opens but API calls fail, check that `nexus-bs@USER.service` is
running and listening on `127.0.0.1:18080`.

If the config is broken, Nexus-BS tries `config.toml.fallback`. Keep the fallback
as a known-good copy.
