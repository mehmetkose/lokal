#!/bin/bash
# VERIFY V13 (SPEC §12): lokal on Linux end to end without sudo. Run it as
#   unshare -r -m -n --propagation private env LOKAL=target/release/lokal S=/tmp/lokal-v13 bash scripts/verify-linux-setup.sh
# It is root only in a private user, mount and network namespace, so the
# host's trust store, resolver and ports are untouched: setup's privileged
# steps run for real against bind-mounted copies (update-ca-certificates, a
# systemd-resolved DNS delegate read by a private resolved), then a .lokal
# name must resolve through resolved and serve HTTPS that verifies against
# the system bundle alone. Needs systemd 258+ (delegates) and iproute2.
set -euo pipefail
LOKAL=${LOKAL:?}
S=${S:?}
step() { printf '\n== %s\n' "$*"; }

rm -rf "$S"; mkdir -p "$S"
/usr/sbin/ip link set lo up
# resolved queries nothing while no link is routable (the host has eth0).
/usr/sbin/ip link add dummy0 type dummy
/usr/sbin/ip addr add 10.99.0.2/24 dev dummy0
/usr/sbin/ip link set dummy0 up
/usr/sbin/ip route add default via 10.99.0.1
echo 0 > /proc/sys/net/ipv4/ip_unprivileged_port_start

step "trust: a writable copy of the system store, then setup's commands"
mkdir -p "$S/anchors" "$S/certs" "$S/update.d"
cp -dR --preserve=mode,links /etc/ssl/certs/. "$S/certs/"
mount --bind "$S/anchors" /usr/local/share/ca-certificates
mount --bind "$S/certs" /etc/ssl/certs
mount --bind "$S/update.d" /etc/ca-certificates/update.d
export LOKAL_HOME="$S/home"
"$LOKAL" internal create-ca >/dev/null
SHA1=$(openssl x509 -in "$LOKAL_HOME/ca/root.pem" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d : | tr A-F a-f)
echo "before: $("$LOKAL" internal trust-status || true)"
/usr/bin/install -m 0644 "$LOKAL_HOME/ca/root.pem" "/usr/local/share/ca-certificates/lokal-$SHA1.crt"
/usr/sbin/update-ca-certificates 2>&1 | tail -2
echo "after:  $("$LOKAL" internal trust-status)"

step "resolver: lokal's DNS delegate, read by a private systemd-resolved"
cp -dR --preserve=mode,links /etc/systemd "$S/etc-systemd"
mkdir -p "$S/etc-systemd/dns-delegate.d"
printf '# Installed by lokal: .lokal names are answered by lokal'"'"'s DNS server.\n[Delegate]\nDNS=127.0.0.1:53535\nDomains=~lokal\n' \
  > "$S/etc-systemd/dns-delegate.d/lokal.dns-delegate"
cat "$S/etc-systemd/dns-delegate.d/lokal.dns-delegate"
mount --bind "$S/etc-systemd" /etc/systemd
mount -t tmpfs tmpfs /run/systemd/resolve
mount -t tmpfs tmpfs /run/dbus
printf 'nameserver 127.0.0.53\n' > "$S/resolv.conf"
mount --bind "$S/resolv.conf" "$(readlink -f /etc/resolv.conf)"
# Not root inside (any uid but 0), so resolved keeps its user; --keep-caps
# passes its own check for binding the stub on port 53.
unshare --user --map-user=1000 --map-group=1000 --keep-caps -- \
  env SYSTEMD_LOG_LEVEL=debug /usr/lib/systemd/systemd-resolved > "$S/resolved.log" 2>&1 &
RESOLVED=$!

step "the daemon on 443, 80 and 53535, and a dev server behind it"
"$LOKAL" daemon run --https-port 443 --http-port 80 --dns-port 53535 --name-backend none > "$S/daemon.log" 2>&1 &
DAEMON=$!
python3 -m http.server 8123 --bind 127.0.0.1 --directory "$S" > "$S/web.log" 2>&1 &
WEB=$!
trap 'kill $RESOLVED $DAEMON $WEB 2>/dev/null || true' EXIT
for _ in $(seq 50); do "$LOKAL" status >/dev/null 2>&1 && break; sleep 0.1; done
echo hello-from-the-dev-server > "$S/index.html"
"$LOKAL" project create demo
"$LOKAL" service add web 8123

for _ in $(seq 50); do curl -s -o /dev/null http://127.0.0.1:8123/ && break; sleep 0.1; done
step "checks"
grep -E "delegate|Delegate" "$S/resolved.log" | head -3 || true
echo "getent: $(getent ahosts web.demo.lokal | awk '{print $1}' | sort -u | tr '\n' ' ')"
echo "curl (system CA bundle, no -k):"
curl -sS --max-time 5 https://web.demo.lokal/index.html
curl -sS --max-time 5 -o /dev/null -w 'TLS verify result: %{ssl_verify_result}, HTTP %{http_code}\n' https://web.demo.lokal/
echo "an unknown name resolves too, but gets no certificate:"
curl -sS --max-time 5 https://nope.demo.lokal/ 2>&1 | head -1 || true
step "control: the same curl without lokal's CA in the bundle"
curl -sS --max-time 5 --cacert /usr/share/ca-certificates/mozilla/ISRG_Root_X1.crt --capath /var/empty https://web.demo.lokal/ 2>&1 | head -1 || true
