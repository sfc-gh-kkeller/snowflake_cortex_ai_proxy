#!/usr/bin/env sh
set -eu

REPO="sfc-gh-kkeller/snowflake_cortex_ai_proxy"
APP="cortex-proxy"
INSTALL_DIR="$HOME/.local/bin"
CONFIG_DIR="$HOME/.config/cortex-proxy"
CONFIG_FILE="$CONFIG_DIR/config.toml"
EXAMPLE_CONFIG="cortex-proxy.example.toml"

require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1"
    exit 1
  fi
}

require curl

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin) OS_ID="macos" ;;
  Linux) OS_ID="linux" ;;
  *) echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
  x86_64|amd64) ARCH_ID="x64" ;;
  arm64|aarch64) ARCH_ID="arm64" ;;
  *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

ASSET="${APP}-v__VERSION__-${OS_ID}-${ARCH_ID}"
EXT="tar.gz"

TAG="$(curl -sS "https://api.github.com/repos/${REPO}/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
if [ -z "$TAG" ]; then
  echo "Failed to determine latest release tag."
  exit 1
fi

ASSET="${APP}-v${TAG#v}-${OS_ID}-${ARCH_ID}.${EXT}"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"

TMP_DIR="$(mktemp -d)"
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

echo "Downloading ${URL}"
curl -fL "$URL" -o "$TMP_DIR/$ASSET"

echo "Installing to ${INSTALL_DIR}"
mkdir -p "$INSTALL_DIR"
tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
mv "$TMP_DIR/$APP" "$INSTALL_DIR/$APP"
chmod +x "$INSTALL_DIR/$APP"

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
  for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile"; do
    if [ -f "$rc" ]; then
      if ! grep -q "PATH=.*$INSTALL_DIR" "$rc"; then
        printf '\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$rc"
      fi
    fi
  done
  echo "Added ${INSTALL_DIR} to PATH in your shell rc files. Restart your terminal."
fi

mkdir -p "$CONFIG_DIR"
if [ ! -f "$CONFIG_FILE" ]; then
  cp "$EXAMPLE_CONFIG" "$CONFIG_FILE"
  echo "Wrote sample config to ${CONFIG_FILE}"
fi

cat <<EOF

Next steps:
1) Edit ${CONFIG_FILE} and set:
   - snowflake.base_url (your account URL)
   - snowflake.pat (Programmatic Access Token)
   - snowflake.default_model (e.g. claude-opus-4-5)
2) Run:
   ${APP} --config ${CONFIG_FILE}
3) Test:
   curl http://localhost:8766/

EOF
