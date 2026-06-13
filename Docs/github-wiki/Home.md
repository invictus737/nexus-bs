# Nexus-BS Wiki

This wiki contains operator and packaging notes for Nexus-BS.

## Installation

- [Build from Source](Build-from-Source) - compile Nexus-BS, install the
  binaries manually, prepare global config, install systemd units, and verify
  services.
- [Install from APT](Install-from-APT) - build the Debian package, publish a
  static APT repository, install `nexus-bs`, create global config, enable
  services, and troubleshoot common install issues.

## Notes

- The packaged install path uses `/opt/nexus-bs` for binaries and dashboard
  assets, `/etc/nexus-bs` for examples and global live config, and systemd
  template services named `nexus-bs-control@USER.service`,
  `nexus-bs@USER.service`, and `nexus-bs-dashboard@USER.service`.
- Review all RF, identity, dashboard, and external-service settings before
  starting a live service. Do not place private credentials in package examples
  or published repository artifacts.
