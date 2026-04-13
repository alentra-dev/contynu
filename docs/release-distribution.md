# Release Distribution

Contynu ships with a user-facing install path based on GitHub Releases, not a Rust-source-first workflow.

## Primary Install Paths

### Linux / macOS

```bash
curl -fsSL https://github.com/alentra-dev/contynu/releases/latest/download/install.sh | sh
```

The Unix installer downloads the matching Linux or macOS release archive and places the binary into a user-local install directory.

### Windows

```powershell
irm https://github.com/alentra-dev/contynu/releases/latest/download/install.ps1 | iex
```

The Windows installer downloads the matching release archive and places `contynu.exe` into a user-local install directory.

## Release Artifacts

The release workflow publishes:

- `contynu-linux-x86_64.tar.gz`
- `contynu-linux-aarch64.tar.gz`
- `contynu-macos-x86_64.tar.gz`
- `contynu-macos-aarch64.tar.gz`
- `contynu-windows-x86_64.zip`
- `contynu-windows-aarch64.zip`
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
- Release binaries are produced for Linux, macOS, and Windows across x86_64 and aarch64 where supported by the workflow.
- Source installation remains available for developers but is not the primary UX.
- Additional package-manager distribution such as Homebrew can be added later without changing the canonical release artifact layout.

## Startup Update Flow

The `contynu` binary now performs a startup release check before normal interactive CLI dispatch.

- It queries the latest GitHub Release metadata.
- It verifies that a newer asset exists for the runtime OS and architecture of the binary currently being executed.
- It offers either an exact manual update command or an auto-update flow that runs the release installer.
- Both paths target the current install directory via `CONTYNU_INSTALL_DIR`, so updates land where the user is already running Contynu from.

`contynu mcp-server` intentionally skips this prompt because MCP stdio transport cannot tolerate extra interactive output.
