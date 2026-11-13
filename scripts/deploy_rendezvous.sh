#!/bin/bash
# Build and install Torrbaby's rendezvous (rendezvous/, binary tbrendezvous) on bronze.
# The source is copied over and built there (bronze is x86_64 Linux; it has rustup). The
# previous binary is kept as tbrendezvous.previous; --rollback puts it back. The unit runs
# as a dynamic user with its state (certificate, Diamond's secret) in /var/lib/tbrendezvous.
#
#   scripts/deploy_rendezvous.sh             build, install, restart, check
#   scripts/deploy_rendezvous.sh --rollback  restore the previous binary
#   scripts/deploy_rendezvous.sh --info      print the fingerprint (and the secret's path)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${TB_RENDEZVOUS_HOST:-bronze}"

check() {
    ssh "$HOST" 'set -e
        for i in $(seq 1 20); do systemctl is-active --quiet tbrendezvous && break; sleep 0.5; done
        systemctl is-active tbrendezvous
        ss -ulpn | grep -q ":3478 " && ss -ulpn | grep -q ":3479 " && ss -ulpn | grep -q ":7443 " || { echo "Ports are not open"; exit 1; }
        sudo journalctl -u tbrendezvous -n 3 --no-pager -o cat'
}

case "${1:-}" in
    --rollback)
        ssh "$HOST" 'set -e; test -x /usr/local/bin/tbrendezvous.previous
            sudo cp /usr/local/bin/tbrendezvous.previous /usr/local/bin/tbrendezvous.new
            sudo mv /usr/local/bin/tbrendezvous.new /usr/local/bin/tbrendezvous
            sudo systemctl restart tbrendezvous'
        check; exit 0 ;;
    --info)
        ssh "$HOST" 'sudo /usr/local/bin/tbrendezvous --state /var/lib/private/tbrendezvous info 2>/dev/null \
            || sudo /usr/local/bin/tbrendezvous --state /var/lib/tbrendezvous info'
        exit 0 ;;
    "") ;;
    *) echo "Unknown argument $1" >&2; exit 2 ;;
esac

(cd "$ROOT/rendezvous" && cargo test -q --locked 2>&1 | tail -3)
rsync -a --delete --exclude target/ "$ROOT/rendezvous/" "$HOST:tbrendezvous-src/"
ssh "$HOST" 'set -e
    cd ~/tbrendezvous-src
    ~/.cargo/bin/cargo build --release --locked -q
    sudo install -m 0644 deploy/tbrendezvous.service /etc/systemd/system/tbrendezvous.service
    if [ -x /usr/local/bin/tbrendezvous ]; then sudo cp /usr/local/bin/tbrendezvous /usr/local/bin/tbrendezvous.previous; fi
    sudo install -m 0755 target/release/tbrendezvous /usr/local/bin/tbrendezvous.new
    sudo mv /usr/local/bin/tbrendezvous.new /usr/local/bin/tbrendezvous
    sudo ufw status | grep -q "^3478/udp" || sudo ufw allow 3478/udp comment "Torrbaby STUN"
    sudo ufw status | grep -q "^7443/udp" || sudo ufw allow 7443/udp comment "Torrbaby rendezvous/relay"
    sudo ufw status | grep -q "^3479/udp" || sudo ufw allow 3479/udp comment "Torrbaby STUN (second port)"
    sudo systemctl daemon-reload
    sudo systemctl enable --quiet tbrendezvous
    sudo systemctl restart tbrendezvous'
if ! check; then
    echo "The rendezvous did not come up; rolling back" >&2
    "$0" --rollback || true
    exit 1
fi
