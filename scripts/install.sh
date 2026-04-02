#!/usr/bin/env sh
set -eu

OWNER_REPO="${CONTYNU_REPO:-alentra-dev/contynu}"
INSTALL_DIR="${CONTYNU_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${CONTYNU_VERSION:-latest}"

detect_os() {
  case "$(uname -s)" in
    Linux) echo "linux" ;;
    Darwin) echo "macos" ;;
    *)
      echo "unsupported operating system: $(uname -s)" >&2
      exit 1
      ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      echo "unsupported architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required tool: $1" >&2
    exit 1
  fi
}

resolve_release_url() {
  asset="$1"
  if [ "$VERSION" = "latest" ]; then
    echo "https://github.com/${OWNER_REPO}/releases/latest/download/${asset}"
  else
    echo "https://github.com/${OWNER_REPO}/releases/download/${VERSION}/${asset}"
  fi
}

write_binary() {
  src="$1"
  dst="$2"
  if command -v install >/dev/null 2>&1; then
    install -m 0755 "$src" "$dst"
  else
    cp "$src" "$dst"
    chmod 0755 "$dst"
  fi
}

warn_if_not_on_path() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      echo "warning: $INSTALL_DIR is not currently on PATH" >&2
      ;;
  esac
}

main() {
  require_tool curl
  require_tool tar

  os="$(detect_os)"
  arch="$(detect_arch)"
  asset="contynu-${os}-${arch}.tar.gz"
  url="$(resolve_release_url "$asset")"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM

  echo "Downloading ${url}"
  curl -fsSL "$url" -o "$tmpdir/$asset"

  mkdir -p "$INSTALL_DIR"
  tar -xzf "$tmpdir/$asset" -C "$tmpdir"

  if [ ! -f "$tmpdir/contynu" ]; then
    echo "archive did not contain a contynu binary" >&2
    exit 1
  fi

  write_binary "$tmpdir/contynu" "$INSTALL_DIR/contynu"
  warn_if_not_on_path
  echo "Installed contynu to $INSTALL_DIR/contynu"
  echo "Run: contynu --help"
}

main "$@"
