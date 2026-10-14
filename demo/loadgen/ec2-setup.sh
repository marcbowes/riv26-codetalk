#!/usr/bin/env bash
# Build the DAT401 load generator on Amazon Linux 2023, close to the API.
#
# Run as ec2-user on a fresh instance in us-east-1 (where Region A's API
# lives); a c7i.4xlarge / c7g.4xlarge (16 vCPU) is plenty.
#
#   curl -sSfO https://raw.githubusercontent.com/marcbowes/riv26-codetalk/main/demo/loadgen/ec2-setup.sh
#   bash ec2-setup.sh
#
# Or copy the repo up yourself (it may be private before the talk) and run
# this script from inside it: it builds whichever checkout it lives in.
#
# Then copy the deploy outputs up from your laptop:
#   scp demo/outputs.json ec2-user@<host>:riv26-codetalk/demo/outputs.json
set -euo pipefail

REPO_URL=${REPO_URL:-https://github.com/marcbowes/riv26-codetalk.git}
CHECKOUT=${CHECKOUT:-$HOME/riv26-codetalk}

echo "==> Packages (C toolchain + cmake for aws-lc-rs, which rustls uses)"
sudo dnf install -y gcc gcc-c++ make cmake clang git tmux

echo "==> Rust"
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
rustup update stable

echo "==> Network limits"
# One socket per in-flight request (function URLs are HTTP/1.1 only).
sudo tee /etc/security/limits.d/90-loadgen.conf >/dev/null <<'EOF'
*    soft nofile 1048576
*    hard nofile 1048576
EOF
# More ephemeral ports, and let new outbound connections reuse TIME_WAIT ones.
sudo tee /etc/sysctl.d/90-loadgen.conf >/dev/null <<'EOF'
net.ipv4.ip_local_port_range = 1024 65535
net.ipv4.tcp_tw_reuse = 1
net.core.somaxconn = 4096
EOF
sudo sysctl --system >/dev/null

echo "==> Source"
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
if [ -f "$SCRIPT_DIR/Cargo.toml" ] && grep -q '^name = "loadgen"' "$SCRIPT_DIR/Cargo.toml"; then
    LOADGEN_DIR=$SCRIPT_DIR
elif [ -d "$CHECKOUT/.git" ]; then
    git -C "$CHECKOUT" pull --ff-only
    LOADGEN_DIR=$CHECKOUT/demo/loadgen
else
    git clone "$REPO_URL" "$CHECKOUT"
    LOADGEN_DIR=$CHECKOUT/demo/loadgen
fi

echo "==> Build"
cd "$LOADGEN_DIR"
cargo build --release
cargo test --release --quiet

cat <<EOF

Built $LOADGEN_DIR/target/release/loadgen

Next:
  1. scp demo/outputs.json ec2-user@<this host>:$(dirname "$LOADGEN_DIR")/outputs.json
  2. (loadgen raises its own open-files soft limit to the hard limit; new logins get 1,048,576.)
  3. tmux new -s load
     cd $LOADGEN_DIR && ./target/release/loadgen --rps 2000 --ramp 30 --duration 2m   # smoke test
     ./target/release/loadgen --rps 20000 --ramp 90                                    # stage
EOF
