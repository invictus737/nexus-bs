# Nexus-BS Static APT Repository

`generate-apt-repo.sh` builds a small static APT repository from one or more
Nexus-BS `.deb` artifacts. The output tree can be published directly by GitHub
Pages, nginx, Apache, or any other plain file server.

## Usage

```sh
packaging/apt/generate-apt-repo.sh \
  --output /tmp/nexus-bs-apt \
  --suite stable \
  --component main \
  --repo-url https://example.github.io/nexus-bs \
  path/to/nexus-bs_*.deb
```

You can also pass a directory containing packages:

```sh
packaging/apt/generate-apt-repo.sh \
  --output /tmp/nexus-bs-apt \
  --input-dir /tmp/nexus-bs-debs
```

The generated tree uses this layout:

```text
dists/<suite>/Release
dists/<suite>/InRelease
dists/<suite>/Release.gpg
dists/<suite>/<component>/binary-<arch>/Packages
dists/<suite>/<component>/binary-<arch>/Packages.gz
pool/<component>/*.deb
```

`InRelease` and `Release.gpg` are created only when `GPG_KEY` is set.

## Signing

Set `GPG_KEY` to the key ID, fingerprint, or signing identity accepted by
`gpg --local-user`:

```sh
GPG_KEY=0123456789ABCDEF packaging/apt/generate-apt-repo.sh \
  --output /tmp/nexus-bs-apt \
  --suite stable \
  path/to/nexus-bs_*.deb
```

If `GPG_KEY` is not set, the script leaves the repository unsigned and prints a
`deb [trusted=yes] ...` source line when `--repo-url` is provided.

## Tools

Required tools are `dpkg-deb`, `dpkg-scanpackages`, and `gzip`.
`apt-ftparchive` is used for `Release` metadata when available; otherwise the
script writes a minimal `Release` file with local checksum tools. `gpg` is
required only when signing.
