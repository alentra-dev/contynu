# Release Checklist

Use this checklist before cutting a serious Contynu release.

## Runtime

- PTY and pipe transports both pass the full test suite
- interactive launcher smoke coverage is green
- signal/interruption handling is validated for common cases
- workspace context files are restored correctly after runs

## Storage and Recovery

- `cargo test` is green
- journal replay and tail repair tests are green
- checkpoint generation tests are green
- repair and reconciliation commands are verified manually

## Launcher Layer

- `.contynu/config.json` is seeded correctly by `contynu init`
- `contynu config validate` is green
- known launchers (`codex`, `claude`, `gemini`) have documented defaults
- launcher override behavior is covered by smoke tests

## Product Surface

- `contynu status`
- `contynu projects`
- `contynu recent`
- `contynu doctor`
- `contynu repair`
- `contynu checkpoint`
- `contynu resume`
- `contynu handoff`

All should be exercised against a real local state directory before release.

## Distribution

- GitHub release workflow succeeds for all supported targets
- release artifacts include installers and checksums
- `scripts/install.sh` installs correctly on Linux and macOS
- `scripts/install.ps1` installs correctly on Windows
- README install instructions match the published release assets

## Documentation

- README reflects actual behavior
- CLI doc reflects actual commands
- crash recovery doc reflects actual repair semantics
- rehydration doc reflects actual packet construction rules
- adapter architecture doc reflects actual launcher config behavior

## Final Audit

- architecture docs and ADRs still match the implementation
- no dirty worktree remains
- latest commits are coherent and intentional
- known limitations are documented honestly
