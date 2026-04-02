# Release Distribution

Contynu ships with a user-facing install path based on GitHub Releases, not a Rust-source-first workflow.

## Primary Install Paths

### Linux

```bash
curl -fsSL https://github.com/alentra-dev/contynu/releases/latest/download/install.sh | sh
```

The installer downloads the Linux release archive and places the binary into a user-local install directory.

## Release Artifacts

The release workflow publishes:

- `contynu-linux-x86_64.tar.gz`
- `install.sh`
- `checksums.txt`

## Installer Environment Variables

The installers support:

- `CONTYNU_REPO`
  Defaults to `alentra-dev/contynu`
- `CONTYNU_VERSION`
  Defaults to `latest`
- `CONTYNU_INSTALL_DIR`
  Overrides the destination directory

## Distribution Notes

- GitHub Releases is the canonical binary distribution channel for the first product release.
- The first public release currently targets Linux x86_64 only.
- Source installation remains available for developers but is not the primary UX.
- Additional package-manager distribution such as Homebrew can be added later without changing the canonical release artifact layout.
