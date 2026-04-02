# Release Distribution

Contynu ships with a user-facing install path based on GitHub Releases, not a Rust-source-first workflow.

## Primary Install Paths

### macOS and Linux

```bash
curl -fsSL https://github.com/alentra-dev/contynu/releases/latest/download/install.sh | sh
```

### Windows PowerShell

```powershell
irm https://github.com/alentra-dev/contynu/releases/latest/download/install.ps1 | iex
```

The installers download a prebuilt release archive for the current platform and place the binary into a user-local install directory.

## Release Artifacts

The release workflow publishes:

- `contynu-linux-x86_64.tar.gz`
- `contynu-macos-x86_64.tar.gz`
- `contynu-macos-aarch64.tar.gz`
- `contynu-windows-x86_64.zip`
- `install.sh`
- `install.ps1`
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
- Source installation remains available for developers but is not the primary UX.
- Additional package-manager distribution such as Homebrew can be added later without changing the canonical release artifact layout.
