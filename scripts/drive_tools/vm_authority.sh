#!/bin/sh
# Guest-only authority services; never expose a host system bus to this fixture.
set -eu
test "${LITTLE_CLIP_DRIVE_VM:-}" = 1
test -f /work/.owned-vm-fixture
test "$(cat /proc/sys/kernel/random/boot_id)" != "$LITTLE_CLIP_HOST_BOOT_ID"
mkdir -p /etc/polkit-1/rules.d /run/udev /run/udisks2 /run/dbus /work/authority
printf 'root:x:0:0:root:/:/bin/sh\nnobody:x:65534:65534:Fixture denial:/:/bin/sh\npolkitd:x:102:102:PolicyKit:/:/bin/sh\n' > /etc/passwd
printf 'root:x:0:\nnobody:x:65534:\npolkitd:x:102:\n' > /etc/group
/usr/bin/dbus-uuidgen --ensure=/etc/machine-id
cat > /work/authority/system.conf <<'XML'
<busconfig>
  <type>system</type>
  <user>root</user>
  <listen>unix:path=/run/dbus/system_bus_socket</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow user="*"/>
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>
XML
cat > /etc/polkit-1/rules.d/00-owned-drive-fixture.rules <<'JS'
polkit.addRule(function(action, subject) {
    if (action.id.indexOf("org.freedesktop.udisks2.") === 0) {
        return subject.user === "root" ? polkit.Result.YES : polkit.Result.NO;
    }
});
JS
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/dbus/system_bus_socket
/usr/bin/dbus-daemon --config-file=/work/authority/system.conf --fork --print-pid > /work/authority/dbus.pid
/usr/lib/systemd/systemd-udevd --daemon > /work/authority/udev.log 2>&1
/usr/bin/udevadm trigger --action=add --subsystem-match=block
/usr/bin/udevadm settle --timeout=10
/usr/lib/polkit-1/polkitd --no-debug > /work/authority/polkit.log 2>&1 &
/usr/lib/udisks2/udisksd --no-debug > /work/authority/udisks.log 2>&1 &
for service in org.freedesktop.UDisks2 org.freedesktop.PolicyKit1; do
    case "$service" in
        org.freedesktop.UDisks2) object=/org/freedesktop/UDisks2 ;;
        org.freedesktop.PolicyKit1) object=/org/freedesktop/PolicyKit1/Authority ;;
    esac
    ready=0
    for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
        if /usr/bin/dbus-send --system --print-reply --reply-timeout=1000 --dest="$service" "$object" org.freedesktop.DBus.Peer.Ping; then
            ready=1
            break
        fi
        /usr/bin/sleep 0.1
    done
    if [ "$ready" != 1 ]; then
        cat /work/authority/polkit.log /work/authority/udisks.log
        exit 1
    fi
done
echo OWNED_AUTHORITY_READY
