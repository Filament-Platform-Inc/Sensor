#!/bin/bash
# Verifies the install/purge promise: that removing the package leaves the
# machine as it was found. Runs in a throwaway container, so it can be trusted
# to start from a known-clean state.
#
# Cannot test dictation itself -- that needs real audio and input devices --
# only that the packaging does and undoes exactly what it claims.
set -uo pipefail

DEB="${1:-dist/sensor_0.1.0_amd64.deb}"
IMAGE="ubuntu:24.04"
DOCKER="${DOCKER:-docker}"

[ -f "$DEB" ] || { echo "no such package: $DEB" >&2; exit 1; }

echo "testing $DEB in $IMAGE"
echo

$DOCKER run --rm -i \
  -v "$(realpath "$DEB")":/tmp/sensor.deb:ro \
  "$IMAGE" bash -s <<'CONTAINER'
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive

pass=0; fail=0
check() { # check <description> <0-or-1>
  if [ "$2" -eq 0 ]; then echo "  ok    $1"; pass=$((pass+1));
  else echo "  FAIL  $1"; fail=$((fail+1)); fi
}

echo "=== preparing ==="
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq adduser >/dev/null 2>&1
# A non-root user, since postinst adds *the installing user* to the groups.
useradd -m -s /bin/bash tester
# And a second user already in 'input', to prove purge leaves alone the
# memberships it did not create.
useradd -m -s /bin/bash veteran
addgroup --system input >/dev/null 2>&1
adduser veteran input >/dev/null 2>&1

echo "=== snapshot before ==="
find /usr /etc /var/lib -xdev 2>/dev/null | sort > /tmp/files.before
getent group | sort > /tmp/groups.before
echo "  $(wc -l < /tmp/files.before) files, $(wc -l < /tmp/groups.before) groups"

echo
echo "=== installing ==="
# postinst reads SUDO_USER to know who to add to the groups; setting it is
# what `sudo apt install` does, without needing sudo inside the container.
SUDO_USER=tester apt-get install -y -qq /tmp/sensor.deb > /tmp/install.log 2>&1
echo "  apt exit: $?"
grep -E "Adding group|Adding user|sensor is installed" /tmp/install.log | sed 's/^/  /' || true

echo
echo "=== after install ==="
for f in /usr/bin/sensord /usr/bin/sensorctl /usr/bin/sensor-gui \
         /usr/lib/udev/rules.d/99-sensor.rules \
         /usr/lib/systemd/user/sensord.service \
         /usr/share/applications/sensor.desktop \
         /usr/share/icons/hicolor/256x256/apps/sensor.png; do
  [ -f "$f" ]; check "installed $f" $?
done

getent group uinput >/dev/null; check "created the 'uinput' group" $?
id -nG tester | tr ' ' '\n' | grep -qx input;  check "added tester to 'input'" $?
id -nG tester | tr ' ' '\n' | grep -qx uinput; check "added tester to 'uinput'" $?
[ -f /var/lib/sensor/added-groups ]; check "recorded the group changes" $?

# The binaries must at least start; a missing shared library shows up here.
su tester -c "/usr/bin/sensorctl help" >/dev/null 2>&1; check "sensorctl runs" $?
su tester -c "/usr/bin/sensord --help" >/dev/null 2>&1; check "sensord runs" $?

echo
echo "=== purging ==="
SUDO_USER=tester apt-get purge -y -qq sensor > /tmp/purge.log 2>&1
echo "  apt exit: $?"

echo
echo "=== after purge ==="
for f in /usr/bin/sensord /usr/bin/sensorctl /usr/bin/sensor-gui \
         /usr/lib/udev/rules.d/99-sensor.rules \
         /usr/lib/systemd/user/sensord.service \
         /usr/share/applications/sensor.desktop; do
  [ ! -e "$f" ]; check "removed $f" $?
done

! id -nG tester | tr ' ' '\n' | grep -qx input;  check "removed tester from 'input'" $?
! id -nG tester | tr ' ' '\n' | grep -qx uinput; check "removed tester from 'uinput'" $?
! getent group uinput >/dev/null; check "removed the 'uinput' group" $?
[ ! -e /var/lib/sensor ]; check "removed /var/lib/sensor" $?
[ ! -e /home/tester/.config/sensor ]; check "removed the user's config" $?
[ ! -e /home/tester/.local/share/sensor ]; check "removed the model directory" $?

# The central promise: only our own changes are reversed.
id -nG veteran | tr ' ' '\n' | grep -qx input
check "left alone a membership it did not create" $?

echo
echo "=== diff against the pre-install snapshot ==="
find /usr /etc /var/lib -xdev 2>/dev/null | sort > /tmp/files.after
getent group | sort > /tmp/groups.after

# Ignore paths apt itself churns; we are testing our package, not dpkg.
leftovers=$(diff /tmp/files.before /tmp/files.after \
  | grep '^>' \
  | grep -vE '/var/lib/(dpkg|apt)|/var/cache|/etc/ld.so.cache' \
  | grep -E '(^|/)sensor(d|ctl|-gui)?($|[./])|99-sensor|sensor\.(desktop|png|conf)' \
  | grep -vE 'libsensors|sensors\.d|sensors3?\.conf' || true)

if [ -z "$leftovers" ]; then
  check "no sensor files left behind" 0
else
  check "no sensor files left behind" 1
  echo "$leftovers" | sed 's/^/        /'
fi

groupdiff=$(diff /tmp/groups.before /tmp/groups.after | grep -E '^[<>]' | grep -i sensor || true)
[ -z "$groupdiff" ]; check "no group changes left behind" $?

echo
echo "================================"
echo "  passed: $pass    failed: $fail"
echo "================================"
[ "$fail" -eq 0 ] || exit 1
CONTAINER
