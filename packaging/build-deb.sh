#!/bin/bash
# Assembles the .deb from a release build. No install steps live here --
# everything the package does is declared in debian/, so dpkg can reverse it.
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="$(grep -m1 '^Version:' packaging/debian/control | cut -d' ' -f2)"
ARCH="$(dpkg --print-architecture)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "building sensor ${VERSION} (${ARCH})"
cargo build --release

install -Dm755 target/release/sensord   "$STAGE/usr/bin/sensord"
install -Dm755 target/release/sensorctl "$STAGE/usr/bin/sensorctl"

install -Dm644 packaging/debian/99-sensor.rules \
  "$STAGE/usr/lib/udev/rules.d/99-sensor.rules"
install -Dm644 packaging/debian/sensor-modules.conf \
  "$STAGE/usr/lib/modules-load.d/sensor.conf"
install -Dm644 packaging/debian/sensord.service \
  "$STAGE/usr/lib/systemd/user/sensord.service"

install -Dm644 README.md "$STAGE/usr/share/doc/sensor/README.md"
install -Dm644 packaging/debian/README.Debian \
  "$STAGE/usr/share/doc/sensor/README.Debian"

install -Dm644 packaging/debian/control "$STAGE/DEBIAN/control"
for script in postinst postrm prerm; do
  install -Dm755 "packaging/debian/$script" "$STAGE/DEBIAN/$script"
done

# Installed-Size is what apt shows before downloading; keep it honest.
SIZE_KB="$(du -sk "$STAGE" | cut -f1)"
sed -i "s/^Priority:/Installed-Size: ${SIZE_KB}\nPriority:/" "$STAGE/DEBIAN/control"

chmod 755 "$STAGE"
OUT="dist/sensor_${VERSION}_${ARCH}.deb"
mkdir -p dist
dpkg-deb --build --root-owner-group "$STAGE" "$OUT"
echo
dpkg-deb --info "$OUT" | sed 's/^/  /'
echo "built $OUT"
