# Semantic `/dev` Device Previews

Implement specialized previews and inspection for Linux `/dev` entries in the file manager.

**Review gate:** This document is a proposal for review. Do not start implementation until the user has reviewed and approved the revised plan. The unified implementation phases in the `/sys` section govern both trees.

## Goal

Make browsing `/dev` genuinely useful for understanding the system, debugging hardware, and navigating relationships between devices, processes, files, and kernel interfaces.

The file manager uses compact, approximately horizontal preview strips, with richer information available in an expanded inspector.

The preview should answer:

1. **What is this device or interface?**
2. **What is it connected to, backed by, or controlling?**
3. **What useful state does Linux already know about it?**
4. **Where can I navigate to inspect it further?**

Do not build a generic hardware dashboard, historical utilization graphs, or decorative visualizations. Prioritize actionable, current information.

Most functionality must work **without root**.

---

## 1. Architecture and safety requirements

### Device classification

Use `stat()`/`lstat()` to distinguish:

- Character devices
- Block devices
- Symlinks
- Directories
- Ordinary files, if encountered

For character and block devices, extract the major/minor numbers from `st_rdev`.

Resolve their sysfs associations where possible:

- `/sys/dev/char/MAJOR:MINOR`
- `/sys/dev/block/MAJOR:MINOR`

Supplement this with udev properties, sysfs class directories, and subsystem-specific metadata.

Use the Rust `udev` crate where appropriate. Prefer reading sysfs directly for simple, stable attributes rather than spawning subprocesses.

### Passive by default

**A preview must not open a device node merely to see what happens.**

Opening or reading some devices can:

- Consume terminal input
- Reset microcontrollers through DTR/RTS
- Interfere with exclusive device ownership
- Block indefinitely
- Consume events from input devices
- Change hardware or firmware state
- Trigger device-specific side effects

Default preview generation should use:

1. `stat` and filesystem metadata
2. Sysfs
3. Udev properties
4. `/proc` metadata
5. Existing mount information
6. Known device semantics

Opening device nodes and issuing ioctls should be an explicit, separately implemented inspection operation, limited to understood and safe operations.

Never automatically write to devices.

### Routing and ordinary file-manager operations

Classify kernel interfaces before extension-based preview selection or content sniffing. A sysfs attribute can report itself as a regular file; that is not permission to pass it to an ordinary content reader.

- Identify character/block nodes from file type and `st_rdev`, including nodes outside `/dev`.
- Identify sysfs and related kernel-interface files using filesystem/mount context and resolved object identity, including aliases and bind mounts. Path prefixes alone are insufficient.
- Apply the same safety routing to `/proc`, cgroupfs, tracefs, debugfs, and efivarfs when encountered through browsing or relationships. Full semantic support for these trees can come later.
- Recognized interfaces use the semantic collector and its explicit read allowlist.
- Unknown interfaces receive metadata and relationship previews only. Do not fall through to generic text/binary/media readers.
- Ordinary files encountered under `/dev`, such as files in `/dev/shm`, retain normal handling when classification establishes that they are ordinary content rather than a kernel interface.
- Recheck the opened resource's identity and classification. Directory enumeration is not a guarantee that a symlink or target remains unchanged.

This policy covers listing previews, hover previews, expanded viewers, text editing, hashing, comparisons, Shelf capture, clipboard content import, drag/drop content extraction, and file operations. Reuse one routing decision across these entry points.

For this implementation, kernel interfaces are inspection-only:

- Navigation, selection, copying a pathname/reference, and copying already collected display values remain available. Reference copying must not trigger later automatic content hydration.
- Copying an explicitly collected attribute value produces a captured snapshot in an ordinary destination; it does not stream the kernel interface again.
- Disable generic content copying, recursive capture, hashing, editing, Save/Save As through ordinary viewers, and writes or file mutations targeting protected kernel interfaces/directories. Reject mixed recursive jobs before admission if they would enter a protected interface; do not silently read or skip it.
- Ordinary backing files and mount directories reached through relationships retain their ordinary operation rules after classification.
- Double-click opens the semantic inspector or navigates a directory as specified below; it must not automatically hand a kernel interface to an external program. Any external handoff is a separate explicit action whose reads cannot be governed by this collector.

### Permissions

Treat permission errors as normal, not exceptional.

Display available metadata even if some fields cannot be read. Avoid prompting for root.

Distinguish:

- Device exists
- Access suggested by observed permissions/ACLs, with actual openability unverified
- Device is currently in use, where determinable
- Additional inspection requires permission

Do not confuse permission bits with guaranteed accessibility: ACLs, device cgroups, LSMs, and other restrictions can matter. Do not open a node to populate an accessibility badge. An explicit inspection may report that a particular operation succeeded or was denied at a particular time; neither result guarantees future access or access for every operation.

### Performance

Preview generation must be asynchronous and nonblocking for the egui UI.

Cache relatively static information such as USB identity, device topology, driver names, and capabilities. Invalidate on relevant udev events and revalidation. Apply the shared identity/lifetime and refresh policies below; ordinary file timestamps and filesystem notifications are not sufficient for kernel interfaces.

Do not repeatedly scan all of `/proc` for every device row. Build shared, throttled process/device associations when needed.

Do not poll for historical metrics. Current state is enough.

### Collector isolation and resource budgets

A byte limit bounds output size, not the time a kernel callback takes. The existing `BackgroundJobs` timeout is checked after an operation returns and cannot interrupt a blocked read. Do not rely on it as an I/O deadline.

- Keep semantic collection separate from ordinary preview workers and the UI thread.
- Perform kernel-interface reads in a bounded helper-process facility with request deadlines, cancellation, bounded responses, and tracked process ownership. Optional device-opening/ioctl inspections use a separate admission budget from passive collection.
- Start with at most two passive helpers and one optional-inspection helper. Never create replacement helpers without accounting for helpers that have not exited.
- On deadline/cancellation, stop waiting in the UI, reject late results, request helper termination, and reap asynchronously. A task stuck in uninterruptible kernel I/O may not exit immediately; keep it counted against the budget. If the budget is exhausted, report collection unavailable rather than spawn indefinitely.
- Application shutdown requests termination of owned helpers without waiting indefinitely for blocked kernel I/O. Use parent-death signaling or equivalent lifecycle handling so a helper cannot resume userspace probing after its owner exits; arrange reaping by the appropriate surviving parent/system process. Helpers must not become untracked background device probes.
- Default bounded text reads: 64 KiB per attribute. Default collection limits: 64 attribute reads and 1 MiB of returned data per object request, with a two-second response deadline. Mark truncated or incomplete results explicitly.
- Larger binary attributes, directory/process scans, and subsystem-specific requests require their own documented bounds. A process scan must bound PID/FD visits and wall-clock work rather than inherit per-attribute limits.
- Coalesce requests for the same object, prioritize visible rows and the active inspector, and cancel superseded work. Preserve partial results when some fields fail.
- These are initial limits to measure and tune during implementation; changes must retain bounded admission, output, and UI waiting.

### Identity, lifetime, and namespace context

Keep alias identity separate from the lifetime of the object being observed.

- Record the observation context for `/dev`, `/sys`, `/proc`, and mount information. Initially inspect the application's current mount/PID/network namespace views; do not silently enter another namespace or label this a complete host inventory.
- Canonical paths and major/minor numbers are lookup keys, not permanent identities. Include object type, relevant filesystem/mount identity, namespace context, and a collector-managed lifetime generation in cached object and relationship keys.
- Use device add/remove events and fresh identity validation to retire generations. Reappearance at the same path or device number must not inherit the removed object's measurements, process associations, or pending completions.
- When events are unavailable or incomplete, revalidate on refresh and expose uncertainty; do not promise continuity across an unobserved replacement.
- Distinguish independent devpts instances. Matching `tty_nr` or major/minor alone must not merge PTYs from different mounts/namespace views.
- Process identity includes PID and process start time in the observed PID namespace. Reused PIDs must not inherit cached relationships.
- Resolve mounts and backing-file paths in their observed namespace context. A path unavailable in the current view is an unavailable relationship, not proof that the backing resource is absent.
- Physical identity, kernel objects, access interfaces, aliases, and processes remain distinct linked entities. A serial number or shared parent alone must not collapse them.

### Partial observations and provenance

Every collected fact must carry its source, observation timestamp, object generation, and collection outcome. Model outcomes such as observed, permission denied, unsupported, disappeared, timed out, truncated, and not requested. Retained values can additionally be stale.

A successful observation of `false` or an empty set differs from failed or incomplete collection. Record process-scan coverage; an empty result from a restricted scan must not become `No processes use this device`.

Display unavailable/stale status where it affects interpretation. The expanded inspector exposes provenance, age, and collection errors; compact rows show the useful values with a concise freshness/unavailability indication.

Accessibility follows the same rule: passive permission information is an estimate, and verified operation results are timestamped observations.

### Cross-navigation

The most important architectural feature is navigating relationships:

- `/dev` node → associated `/sys` device
- Device → driver
- Device → parent bus/controller
- Device → attached processes, when accessible
- Loop device → backing file → mount point
- Block device → partitions → filesystems
- DRM device → GPU → connectors/monitors
- VFIO group → PCI devices
- TTY → shell and foreground job
- MTD → flash partitions → UBI volumes

Prefer clickable relationships over duplicating every detail in a 50-pixel preview.

### Inspector entry points and navigation

- Ordinary double-click on a directory continues to navigate into it. Provide an embedded inspection icon and an `Inspect system object` context-menu action for semantic directory rows.
- Double-click on a device node or recognized kernel attribute opens the shared semantic inspector. Unknown kernel interfaces open a metadata-only inspector.
- The inspector is a read-only expanded view owned by the file manager's existing single-viewer lifecycle. Returning to the folder preserves selection, scroll, and browser history.
- Opening an inspector does not create a folder-history entry. Relationship navigation that changes the browser's location uses normal Back/Forward history and folder-change hooks.
- Give relationship links explicit targets and actions: browse/reveal a path or inspect an object. Process links initially inspect identity and available relationships rather than launch a process or scrape arbitrary `/proc` content.
- A plain click/release on a relationship link activates it. Ctrl/Shift clicks retain the file manager's selection behavior, and a drag cancels link activation and retains normal file-reference dragging. Non-link parts of a strip behave like the rest of the row.
- Inspector links can replace the inspected object within a local inspector navigation stack; provide an inspector Back action distinct from `Back to folder`. Leaving the inspector restores the saved browser state.
- Missing, restricted, or removed relationship targets show a clear status without navigating to a different object that reused the path.

---

## 2. Generic `/dev` entry preview

Every device should have a useful fallback, even if no specialized handler recognizes it.

Show:

- Filename
- Character/block device classification
- Major/minor numbers
- Permissions and owner/group
- Resolved driver/subsystem, if available
- Sysfs target, if available
- Udev-provided human-readable identity, if available

For symlinks, show the resolved destination and reuse the target device's semantic preview.

Recognize stable naming directories such as:

- `/dev/disk/by-id/`
- `/dev/disk/by-uuid/`
- `/dev/disk/by-label/`
- `/dev/disk/by-path/`
- `/dev/serial/by-id/`
- `/dev/serial/by-path/`
- `/dev/dri/by-path/`
- `/dev/input/by-id/`
- `/dev/input/by-path/`

Do not independently reimplement previews for every alias. Resolve the target and optionally display why that alias exists.

---

## 3. TTYs, PTYs, and virtual consoles

### `/dev/tty0`, `/dev/tty1`–`tty63`

Distinguish:

- `/dev/tty0`: Alias for the currently active Linux virtual console
- `/dev/ttyN`: Specific virtual console
- `/dev/tty`: Calling process's controlling terminal
- `/dev/console`: System console interface

For virtual consoles, show:

- Whether it is the active VT
- Whether a console is allocated, where reliably determinable
- Associated login/session information
- Processes with that controlling terminal
- Foreground process group
- Shell or session leader, when identifiable

The active VT is exposed through `/sys/class/tty/tty0/active`.

Use `/proc/PID/stat` to associate processes with a TTY using `tty_nr`, and determine session and foreground process groups using `session`, `pgrp`, and `tpgid`.

Do not assume the session leader is always a shell.

An unallocated virtual console should receive a muted, simple preview such as `Unused virtual console`.

There is no useful historical distinction between a never-used console and a previously used console whose state has been released. Do not try to reconstruct that history.

### `/dev/pts/N`

PTY entries are dynamically allocated terminal endpoints, usually associated with graphical terminal emulators, SSH sessions, or multiplexers.

Show:

- PTY identifier
- Associated processes
- Likely shell/session leader
- Current foreground process or job
- Process group and session relationships
- Optionally associated terminal emulator, if reliably identifiable

For example:

`pts/3 | fish → cargo build --release | 3 processes`

The foreground process group is more useful than simply displaying the shell.

Do not claim to retrieve terminal output or scrollback. The kernel PTY does not retain previously displayed lines.

**Never read `/dev/pts/N` to generate a preview.** This could consume pending terminal input.

Potential optional integrations:

- `tmux capture-pane`
- GNU Screen capture
- Terminal-emulator-specific IPC, if supported

These are separate integrations, not generic PTY capabilities.

### `/dev/ptmx`

Explain that opening this interface allocates a PTY master/slave pair.

Show a static semantic preview. Do not open it.

### `/dev/vcsN`, `/dev/vcsaN`, `/dev/vcsuN`

These expose virtual-console screen contents:

- `vcsN`: Character data
- `vcsaN`: Character data plus attributes, dimensions, and cursor information
- `vcsuN`: Unicode character data

Unnumbered variants refer to the active virtual console.

A miniature screen snapshot is available only through an explicit `Capture console snapshot` action in the inspector. Ordinary row/hover previews identify the associated VT without opening a VCS node. This preserves the default rule that previews never open device nodes.

The snapshot action reads only the selected, understood VCS interface through the optional-inspection helper, with bounded dimensions/data, permission checks, and a response deadline. Capture once; do not enable a live feed or background refresh. Keep the captured content session-local and do not persist or export it without a separate explicit user action.

However, these nodes are often permission-restricted because console contents can be sensitive.

Do not change permissions, request root, or attempt privileged access.

If a requested snapshot is denied, show the associated VT and a clear `Screen contents restricted` indication. Before attempting capture, report access as unverified rather than claiming the contents are readable or restricted.

They contain current screen state, not scrollback history.

---

## 4. USB serial devices

Handle:

- `/dev/ttyUSB*`
- `/dev/ttyACM*`

These deserve a dedicated preview.

Use udev and sysfs to retrieve:

- USB vendor ID and product ID
- Manufacturer
- Product name
- Serial number, when present
- USB interface
- Kernel driver (`cp210x`, `ftdi_sio`, `ch341`, `cdc_acm`, etc.)
- Physical USB port path/topology
- Device permissions

Example:

`ttyUSB0 | Silicon Labs CP2102 | 10C4:EA60 | cp210x | USB 1-3`

Use `udevadm info --query=property --name=/dev/ttyUSB0` as a reference for the properties available.

For direct sysfs inspection, resolve `/sys/class/tty/ttyUSB0/device` and walk to the parent USB device containing `idVendor`, `idProduct`, `manufacturer`, `product`, and `serial`.

**Do not automatically open the serial device.**

Baud rate requires special treatment:

- The currently configured baud rate can often be queried using termios.
- The correct baud rate expected by the attached device generally cannot be discovered.
- Opening the port can assert DTR/RTS, reset hardware, or interfere with another process.
- CDC ACM devices may ignore the configured line rate entirely.

Therefore, do not include a supposedly discovered baud rate in the default passive preview.

An optional explicit `Inspect serial configuration` action may query termios, with appropriate warnings and error handling.

The USB VID:PID identifies the USB-facing interface, not necessarily a downstream microcontroller connected to a USB-to-UART adapter.

---

## 5. Block devices and storage

Handle conventional block devices, including:

- `/dev/sd*`
- `/dev/nvme*`
- `/dev/vd*`
- `/dev/mmcblk*`
- Partitions of these devices
- `/dev/disk/*` aliases

Show:

- Device identity/model
- Capacity
- Transport/controller, where available
- Partition layout
- Filesystem types
- Labels and UUIDs
- Mount points
- Read-only state
- Associated sysfs device

A proportional partition map is an excellent horizontal preview.

Use sysfs, udev, `/proc/self/mountinfo`, and existing filesystem identification metadata. Do not automatically read arbitrary block-device contents.

Distinguish a physical disk, partition, virtual block device, and device-mapper volume.

### `/dev/loopN`

Implement a dedicated loop-device preview.

Use `/sys/block/loopN/loop/` and related sysfs attributes to obtain:

- Backing file
- Offset
- Size limit
- Exposed device size
- Read-only state
- Autoclear
- Direct I/O
- Partition scanning
- Child partitions

Also show:

- Filesystem type, when already known or safely available
- Mount points
- Backing-file location

Example:

`loop2 | SquashFS | /var/lib/snapd/snaps/firefox_7084.snap | mounted /snap/firefox/7084`

The backing file should be clickable.

An unattached loop device should simply show `Unattached loop device`. Do not invent historical usage.

Support reverse navigation from an image file to any currently associated loop device when useful.

### `/dev/mapper/*` and `/dev/dm-*`

Use sysfs/device-mapper metadata to identify:

- Device-mapper name and UUID
- Underlying block devices
- Holders/slaves relationships
- Filesystem and mount point
- Device-mapper target type, when safely available

Represent the dependency chain.

Recognize common LUKS, LVM, and other mapped-storage arrangements where metadata permits.

Avoid attempting to unlock, mount, or reconfigure anything automatically.

---

## 6. Audio devices: `/dev/snd/*`

Recognize ALSA devices such as:

- `controlC*`
- `pcmC*D*p`
- `pcmC*D*c`
- `midiC*D*`
- `seq`
- `timer`

Show:

- Sound card name
- Card/device number
- Playback versus capture
- PCM versus MIDI versus control interface
- Associated USB/PCI device
- Driver
- Available stream information, where safely obtainable

Use `/proc/asound/`, sysfs, and udev for passive metadata.

Supported sample rates, formats, channel counts, and other capabilities may require ALSA queries or opening the device. Keep those as optional deeper inspection rather than assuming they are passively available.

Do not automatically open capture devices or generate live audio meters.

For a multichannel USB interface, show the relationship between its ALSA nodes and the same physical USB device.

---

## 7. Video devices: `/dev/video*` and `/dev/media*`

Recognize V4L2 and media-controller nodes.

Show passively:

- Device name
- Driver
- USB/PCI identity
- Associated physical device
- Related video/media nodes

Optional explicit inspection through V4L2 read-only capability ioctls can retrieve:

- Capture/output capabilities
- Pixel formats
- Supported resolutions
- Frame rates
- Camera controls

Do not automatically start streaming or capture frames.

Where supported, media-controller topology can reveal sensor → processing → capture relationships.

---

## 8. Input and HID devices

Handle:

- `/dev/input/event*`
- `/dev/input/mouse*`
- `/dev/input/js*`
- `/dev/hidraw*`

Show:

- Device name
- Manufacturer/vendor/product IDs
- Driver
- Physical USB/Bluetooth connection
- Input capability categories, when accessible
- Related nodes for the same physical device

For evdev, useful categories include:

- `EV_KEY`
- `EV_REL`
- `EV_ABS`
- `EV_SW`
- `EV_LED`
- `EV_FF`

For HID, optionally decode the HID report descriptor to summarize input, output, and feature reports.

Do not automatically read event streams. Reading input nodes can consume events and may expose sensitive input.

Prefer passive sysfs/udev information first. Use ioctl-based inspection only when appropriate and permitted.

---

## 9. Raw USB devices: `/dev/bus/usb/BBB/DDD`

Show:

- VID:PID
- Manufacturer/product/serial
- USB speed
- Bus and device address
- Port topology
- Configuration and interface count
- Interface classes
- Bound drivers
- Endpoint descriptors

Most of this can be read from sysfs without opening the raw USB device.

Do not confuse USB interfaces or endpoints with separate `/dev` nodes.

An interface/endpoint layout can make a useful expanded visualization, especially for composite audio, MIDI, HID, and communication devices.

---

## 10. DRM devices: `/dev/dri/*`

Handle:

- `cardN`
- `renderDN`
- Legacy `controlDN`, if present
- `by-path` symlinks

Distinguish primary DRM nodes from render nodes.

Show:

- Associated GPU/display controller
- Driver
- PCI identity
- Related primary/render nodes
- Current user access permissions

For primary nodes, also show connectors from `/sys/class/drm/`:

- DP
- HDMI
- eDP
- Other connector types
- Connected/disconnected status
- Enabled state, with correct interpretation
- Available modes
- EDID monitor identity, when readable

A compact connector map is particularly useful.

Example:

`card0 | NVIDIA GPU | DP-1 connected | HDMI-A-1 connected`

EDID-supported modes are not necessarily the currently active mode. Do not claim otherwise.

Active display modes may require DRM/KMS state queries or compositor-specific APIs.

Optionally associate processes with DRM nodes by matching open file descriptors through `/proc`, subject to permissions.

An open DRM FD does not prove active GPU utilization.

Do not acquire DRM master, change display modes, read framebuffer contents, or attempt screen capture.

---

## 11. I²C adapters: `/dev/i2c-*`

These nodes represent I²C bus adapters, not individual devices.

Show:

- Adapter number
- Adapter name
- Parent controller
- Kernel driver
- Registered devices and their addresses
- Bound drivers for registered devices
- I²C multiplexer relationships

Useful sysfs locations:

- `/sys/class/i2c-dev/i2c-N/`
- `/sys/bus/i2c/devices/`

Registered devices are commonly represented as `N-00XX`, where `XX` is the address.

A compact address map can be useful, but it must be labeled as **registered devices**, not a complete scan of everything physically connected.

I²C has no universal discovery or VID:PID protocol.

**Never run `i2cdetect` or probe arbitrary addresses automatically.** Some I²C transactions can disturb or damage device state.

When an I²C device has an associated `hwmon` interface, link to the sensor's sysfs attributes.

---

## 12. CPU devices: `/dev/cpu/*`

On x86, recognize:

- `/dev/cpu/N/cpuid`
- `/dev/cpu/N/msr`

Each numbered directory represents a logical CPU.

### CPUID

Show:

- Logical CPU number
- Physical package/core/thread topology
- CPU model
- Supported instruction sets
- Cache-sharing relationships

Prefer querying CPUID directly using Rust's x86 intrinsics or `raw-cpuid` rather than opening `/dev/cpu/N/cpuid`.

When CPU-specific results matter, pin the querying thread to the appropriate logical CPU.

Cross-link to `/sys/devices/system/cpu/cpuN/`.

### MSR

Identify `/dev/cpu/N/msr` as model-specific register access.

Do not automatically read or write arbitrary MSRs.

Most access is privileged, and register interpretation depends on CPU model.

Prefer safer sysfs sources for CPU frequency, topology, caches, temperature, and power information.

---

## 13. VFIO devices: `/dev/vfio/*`

Recognize:

- `/dev/vfio/vfio`
- Numbered VFIO group devices
- Related modern VFIO/IOMMUFD interfaces where present

For numbered group devices, use:

`/sys/kernel/iommu_groups/N/devices/`

Show:

- IOMMU group number
- PCI devices in the group
- Vendor/device names
- PCI addresses
- Bound drivers
- Whether devices are bound to `vfio-pci`

Example:

`vfio/12 | IOMMU group 12 | GPU 01:00.0 + Audio 01:00.1 | vfio-pci`

Provide links to each PCI device and its driver.

Optionally find processes holding the VFIO group device open using `/proc`, where permitted.

Do not assume that an IOMMU group or `vfio-pci` binding proves a device is currently assigned to a running VM.

Account for newer VFIO device-centric interfaces and `/dev/iommu` where available.

---

## 14. WMI devices: `/dev/wmi/*`

WMI devices expose vendor-specific ACPI firmware interfaces.

Use `/sys/bus/wmi/devices/` and related sysfs metadata to show:

- WMI GUID
- Bound kernel driver
- Associated ACPI device
- Vendor-specific interface identity, when known
- Related sysfs interfaces

Where a WMI driver already exposes standard Linux interfaces, link to them:

- `hwmon`
- `leds`
- `rfkill`
- `platform_profile`
- Input devices
- Power-supply controls

Do not invoke arbitrary WMI firmware methods.

For unknown GUIDs, a simple identity/driver preview is sufficient.

---

## 15. Raw flash: `/dev/mtd*`

Recognize:

- `/dev/mtdN`
- `/dev/mtdNro`
- `/dev/mtdblockN`

Use `/sys/class/mtd/mtdN/` and `/proc/mtd`.

Show:

- Partition/device name
- Flash type (NOR, NAND, etc.)
- Capacity
- Erase-block size
- Write size
- OOB size, when applicable
- ECC capabilities, when exposed
- Bad-block count, when exposed
- Read-only state
- Parent flash device and partition relationships

Example:

`mtd3 | rootfs | NAND | 59 MiB | 64 KiB erase blocks`

Build a proportional flash partition map where partition relationships and offsets are known.

Do not fabricate per-block health from an aggregate bad-block count.

Optionally support explicit bad-block inspection through documented MTD ioctls, but never automatically scan flash.

Recognize associated UBI devices and volumes:

- `/sys/class/ubi/`
- `/dev/ubiX`
- `/dev/ubiX_Y`

Cross-link MTD devices to UBI volumes, showing volume names, sizes, and available eraseblock/wear metadata.

Treat raw flash as inspection-only by default. Never automatically erase or write it.

---

## 16. Pseudo-devices and special character devices

Create a registry of well-known device names with concise semantic previews.

These should generally be **static descriptions of their read/write behavior**, not previews produced by actually reading them.

### `/dev/null`

- Reads return EOF.
- Writes discard data.
- Show `Discard sink / EOF source`.

### `/dev/zero`

- Reads produce zero bytes.
- Writes discard data.
- Show `Infinite zero-byte source`.

### `/dev/full`

- Reads produce zero bytes.
- Writes fail with `ENOSPC`.
- Show `Simulated full device`.

### `/dev/random` and `/dev/urandom`

- Kernel random-number interfaces.
- Modern Linux uses a cryptographically secure RNG.
- Explain their semantics without consuming random data for a preview.
- Do not show fabricated entropy levels.

### `/dev/kmsg`

- Kernel message interface.
- Often restricted.
- Do not open or consume messages automatically.
- Link to appropriate journal/kernel-log inspection where permitted.

### `/dev/fuse`

- Interface used by userspace filesystem implementations.
- Static semantic preview.
- Do not open it.

### `/dev/tty` and `/dev/console`

Use the TTY semantics described earlier.

### `/dev/ptmx`

Use the PTY allocator semantics described earlier.

For other recognized pseudo-devices, add registry entries only when their behavior can be explained accurately and concisely.

---

## 17. Directory previews

Some `/dev` directories should receive useful summaries instead of a plain file count.

### `/dev/pts/`

Show:

- Number of currently allocated PTYs
- Associated shells and foreground jobs, where accessible
- Links to individual PTYs

### `/dev/dri/`

Show:

- Number of GPUs/display controllers
- Primary and render nodes
- Connected display connectors

### `/dev/snd/`

Group nodes by ALSA sound card and distinguish playback, capture, MIDI, and control interfaces.

### `/dev/input/`

Group event, mouse, joystick, and HID-related nodes by physical device where possible.

### `/dev/vfio/`

Show available VFIO groups and their member PCI devices.

### `/dev/cpu/`

Summarize logical CPU entries and the availability of CPUID/MSR interfaces.

### `/dev/mapper/`

Show mapped block devices and their underlying storage relationships.

### `/dev/bus/usb/`

Group raw USB nodes by bus and physical device identity.

### `/dev/disk/` and `/dev/serial/`

Present stable aliases and their resolved targets without treating aliases as independent hardware.

For other directories, use the existing generic directory preview.

---

## 18. Optional process associations

Where useful, associate devices with processes that have them open.

This is especially valuable for:

- PTYs
- DRM render nodes
- Audio devices
- Video devices
- Serial ports
- VFIO group devices

Use `/proc/PID/fd/` and related metadata, subject to permission restrictions.

Prefer matching the underlying device identity rather than relying solely on symlink text.

Do not perform a full `/proc` scan separately for every preview. Share results and throttle refreshes.

Distinguish **has an open file descriptor** from **is actively using the hardware**.

Do not infer exclusive ownership unless the relevant subsystem actually exposes it.

---

## 19. Preview layout

Target a compact horizontal strip with three conceptual regions:

**Left:** Device name and type.

**Center:** Most useful semantic information.

**Right:** State, identity, and short secondary metadata.

Examples:

- `ttyUSB0 | Silicon Labs CP2102 | 10C4:EA60 | cp210x`
- `pts/3 | fish → cargo build --release | Foreground`
- `loop2 | firefox_7084.snap | SquashFS | /snap/firefox/7084`
- `card0 | NVIDIA GPU | DP-1 ● HDMI-A-1 ●`
- `i2c-0 | Intel I801 SMBus | 3 registered devices`
- `vfio/12 | GPU + HDMI Audio | 2 PCI functions`
- `mtd3 | rootfs | NAND | 59 MiB | 64 KiB erase`
- `cpu/6 | Core 3 / Thread 0 | AVX2 · AES-NI`
- `null | Discard writes · EOF on read`

Use small diagrams only when they convey actual structure:

- Partition maps
- DRM connector maps
- I²C registered-address maps
- VFIO group membership
- Flash partition layouts
- Device dependency relationships

Do not use fake activity graphs, fake waveform data, or fabricated hardware state.

The expanded inspector should contain the full metadata and navigable relationships.

---

## 20. Implementation approach

Build this as a modular registry of semantic device inspectors.

Suggested interface concepts:

- `DeviceIdentity`: path, type, major/minor, permissions, sysfs association
- `DeviceKind`: classified device family
- `DeviceMetadata`: structured facts and relationships
- `DeviceInspector`: matches a device and gathers metadata
- `DevicePreview`: compact UI representation
- `DeviceRelation`: navigable relationship to another file, process, or kernel object

Keep collection separate from rendering.

Inspectors should return partial results when information is unavailable.

A handler must never claim that an unavailable property is false. For example, `No processes found` and `Cannot inspect processes` are different states.

Favor a data model that can later be reused for semantic `/sys` and `/proc` previews.

### Unified implementation order

Use the single phase plan in `/sys` section 37 for both `/dev` and `/sys`. Shared infrastructure is implemented before subsystem-specific collectors. PTY/process association is admitted only after the shared, throttled process-association collector is available; it is not a later retrofit.

## Acceptance criteria

- Browsing `/dev` never blocks the UI.
- Default previews do not open device nodes or alter device state; VCS snapshots require an explicit capture action.
- Common devices show meaningful metadata without root.
- Unavailable information is omitted or labeled clearly.
- Device aliases resolve to the same underlying identity.
- Device-to-sysfs and other cross-navigation works.
- Detached, unavailable, and unallocated devices receive sensible previews.
- No fabricated telemetry or guessed hardware capabilities.
- Unsupported devices still receive a useful metadata-only preview without generic content reads.
- The implementation is modular and reusable for `/sys` and `/proc`.

The goal is not to implement a complete hardware-management application. It is to make `/dev` understandable and useful to browse, while preserving the  fast, lightweight file-manager experience.


# Semantic `/sys` Previews and Kernel Device Topology

Implement specialized previews and inspection for Linux **sysfs (`/sys`)** in the file manager.

This should extend the existing `/dev` semantic-preview architecture, not become a separate implementation of hardware discovery.

## 1. Core distinction: `/dev` versus `/sys`

The two trees frequently describe the **same underlying hardware**, but serve different purposes.

| Aspect | `/dev` | `/sys` |
|---|---|---|
| Primary purpose | Userspace access to devices | Kernel object model, attributes, and relationships |
| Typical entries | Character/block devices | Directories, attributes, symlinks |
| Main question | "What can I do with this device?" | "What is this object, how is it connected, and what does the kernel know about it?" |
| Identity | Major/minor, udev properties, device node | Kernel object path, subsystem, parentage |
| Richest information | Device capabilities and access semantics | Hardware topology, drivers, configuration, current state |
| Preview emphasis | Access, capabilities, backing resources | Identity, relationships, attributes, configuration |
| Typical permissions | Device opening often restricted | Many attributes readable without root |
| Mutations | Device-specific operations | Writes to sysfs attributes, often immediately affecting hardware |

### Critical architectural rule

**Do not implement two separate hardware inspectors.**

For example:

`/dev/dri/card0` and `/sys/class/drm/card0/` refer to the same DRM device.

Both should share:

- GPU identity
- Kernel driver
- PCI address
- Connected monitors
- Associated render nodes
- Device relationships

But their previews should emphasize different things.

**`/dev/dri/card0` preview:**

`card0 | NVIDIA GPU | Primary DRM node | DP-1, HDMI-A-1`

**`/sys/class/drm/card0/` preview:**

`NVIDIA GPU | PCI 01:00.0 → nvidia → DRM | 2 connected displays`

The `/dev` preview identifies an access interface. The `/sys` preview explains its position in the kernel's hardware model.

The expanded inspector can reuse almost everything.

---

## 2. Overall goals

Make `/sys` useful for:

1. Understanding physical and virtual hardware topology.
2. Identifying drivers, buses, devices, and interfaces.
3. Inspecting current kernel-exposed state.
4. Discovering relationships between hardware components.
5. Understanding configuration and supported options.
6. Navigating between `/sys`, `/dev`, `/proc`, and ordinary files.
7. Debugging hardware and driver problems.

Prioritize:

- Hardware identity
- Current state
- Parent/child relationships
- Meaningful numerical values
- Configuration and supported options
- Driver binding
- Anomalies and actionable information

Avoid:

- Generic utilization dashboards
- Historical charts requiring continuous sampling
- Decorative representations without semantic meaning
- Reimplementing complete hardware-management utilities
- Expensive probing merely to populate a file preview

## 3. Understand the structure of sysfs

The major top-level directories include:

```text
/sys/
├── block/
├── bus/
├── class/
├── dev/
├── devices/
├── firmware/
├── fs/
├── kernel/
├── module/
└── power/
```

These aren't independent collections of hardware.

A single physical device can appear through multiple paths, often involving symlinks.

For example:

```text
/sys/devices/.../0000:01:00.0/
          │
          ├── /sys/bus/pci/devices/0000:01:00.0
          │
          ├── /sys/class/drm/card0
          │
          └── /sys/class/drm/renderD128
```

The exact hierarchy varies by hardware and driver.

### Canonical identity

Use the canonical sysfs device path and associated kernel identifiers to avoid treating aliases as separate devices.

However, **do not collapse distinct kernel objects merely because they share a parent**.

For example, a GPU's PCI function, DRM primary node, render node, and display connectors are related but distinct objects.

Represent these as linked entities with appropriate types.

### Directory classification

Classify sysfs directories into categories such as:

- Physical or virtual device
- Bus
- Device class
- Driver
- Kernel module
- Device interface
- Attribute collection
- Firmware object
- Kernel subsystem
- Filesystem instance
- Power-management object

A directory's preview should summarize the object represented by the directory, not merely count its children.

---

## 4. Generic sysfs directory preview

Every recognized sysfs directory should have a generic semantic preview.

Show, where available:

- Kernel object name
- Subsystem
- Device class
- Parent device
- Bound driver
- Bus type
- Associated `/dev` node
- Number and types of child devices
- Current state, if exposed through common attributes

For example:

`0000:01:00.0 | NVIDIA GPU | PCIe | Driver: nvidia | DRM card0`

Or:

`1-3:1.0 | USB interface 0 | AudioControl | Driver: snd-usb-audio`

Use the `subsystem` and `driver` symlinks when present.

Do not assume every sysfs directory has a `uevent` file, driver, or corresponding device node.

### Navigable breadcrumbs

A particularly valuable feature is displaying hardware ancestry:

`PCI root → xHCI controller → USB hub → USB device → interface`

Or:

`PCI root → NVMe controller → namespace → partition`

These relationships are much more useful in `/sys` than a generic tree of directory names.

---

## 5. Generic sysfs attribute preview

Individual sysfs files deserve specialized handling.

Examples:

```text
/sys/class/net/enp5s0/operstate
/sys/class/thermal/thermal_zone0/temp
/sys/class/power_supply/BAT0/capacity
/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq
```

These are not ordinary text documents. They are interfaces to kernel attributes.

### Implement a typed attribute registry

Recognize attributes by **subsystem + relative path + attribute name**, not filename alone.

For example, `temp` in one subsystem may not have the same meaning or units as `temp` elsewhere.

Support semantic types including:

- Boolean
- Integer
- Signed integer
- Enumeration
- Bitmask
- Unit-bearing measurement
- Identifier
- State
- Range
- List of supported values
- CPU mask or CPU list
- Device reference
- Binary data
- Opaque text

### Semantic rendering

Examples:

| Attribute | Raw value | Semantic display |
|---|---|---|
| `temp1_input` | `47500` | 47.5 °C |
| `scaling_cur_freq` | `4200000` | 4.2 GHz |
| `capacity` | `82` | 82% battery |
| `online` | `1` | Online |
| `operstate` | `up` | Interface up |
| `current_link_width` | `16` | PCIe ×16 |
| `brightness` | `128` | 128 / maximum |
| `cpulist` | `0-3,8-11` | Eight logical CPUs |

Apply conversions only when the attribute's documented units are known.

Do not assume all sysfs measurements use the same scaling.

### Supported values

Some sysfs attributes expose available options in separate files, or mark the selected option using brackets.

For example:

```text
none mq-deadline [kyber] bfq
```

Parse this as a selection where appropriate, rather than displaying it as arbitrary text.

### Write-capable attributes

Many sysfs files are writable and control real hardware.

Examples:

- CPU frequency governors
- Device power management
- LED brightness
- Fan PWM
- Device driver binding
- PCI device reset
- USB authorization
- Network interface settings
- Kernel tracing configuration

**Never write to sysfs automatically.**

Do not expose generic editable text fields for arbitrary sysfs attributes as part of this implementation.

If configuration editing is later supported, it should use explicit, typed operations with validation and clear descriptions of side effects.

---

## 6. USB devices and interfaces

Implement rich USB topology previews.

Relevant locations:

```text
/sys/bus/usb/devices/
/sys/devices/.../usbN/
/sys/class/usb_device/
```

The exact class entries vary by kernel.

### USB device directories

For a device such as `1-3`, show:

- Vendor ID and product ID
- Manufacturer
- Product name
- Serial number, if available
- USB version
- Negotiated connection speed
- Bus number
- Device number
- Physical port path
- Power attributes, when available
- Number of configurations
- Interfaces
- Bound interface drivers

Example:

`1-3 | Arturia AudioFuse | USB High Speed | 3 interfaces`

### USB interface directories

For an interface such as `1-3:1.0`, show:

- Interface number
- Alternate setting
- Interface class/subclass/protocol
- Bound driver
- Associated endpoints
- Associated `/dev` interfaces

Example:

`1-3:1.2 | USB Audio Streaming | snd-usb-audio | 2 endpoints`

### USB endpoint information

Expose:

- Endpoint address
- Direction (IN/OUT)
- Transfer type
- Maximum packet size
- Polling interval, where relevant

A compact USB interface/endpoint topology is worthwhile in the expanded inspector.

### Difference from `/dev`

`/dev/bus/usb/001/007` is a raw USB access node.

`/sys/bus/usb/devices/1-3/` describes the USB device's identity, interfaces, configuration, drivers, and position in the physical USB tree.

These should share USB identity data.

Do not invent a separate `/dev` node for each endpoint.

---

## 7. PCI and PCIe devices

Relevant location:

`/sys/bus/pci/devices/`

PCIe is one of the best opportunities for specialized `/sys` previews.

Show:

- PCI address/domain/bus/device/function
- Vendor/device/subsystem IDs
- Human-readable identity, when available
- PCI class
- Bound driver
- Parent bridge
- IOMMU group
- NUMA node
- PCIe negotiated link speed
- PCIe negotiated link width
- Supported maximum link speed/width, when exposed
- Resource/BAR summary
- Associated `/dev` nodes and child devices

Example:

`01:00.0 | NVIDIA RTX 3090 | PCIe 4.0 ×16 | nvidia | IOMMU 12`

### Useful visualization

A compact hierarchy or relationship diagram:

`PCI root → Bridge → GPU → DRM + Audio`

Show PCIe link information only when available. Avoid assuming every PCI device exposes meaningful PCIe link attributes.

### IOMMU groups

Link PCI devices to:

`/sys/kernel/iommu_groups/N/`

Display all devices sharing the group.

This must reuse the VFIO inspector's group membership information.

A PCI device's IOMMU group is not equivalent to its VFIO assignment status.

---

## 8. DRM and connected monitors

Relevant location:

`/sys/class/drm/`

Show:

- GPU/display-controller identity
- Driver
- Primary and render nodes
- Connector types
- Connector status
- EDID identity
- Available modes
- Associated PCI/platform device

For connector directories such as `card0-DP-1`, show:

`DP-1 | Connected | Monitor identity | Available modes`

Parse EDID when readable.

Distinguish:

- Connected versus disconnected
- Enabled versus disabled
- Advertised modes versus active mode

Do not assume the highest advertised refresh rate is currently active.

### Difference from `/dev`

The `/dev/dri/cardN` preview emphasizes the DRM access node.

The sysfs connector preview emphasizes the **specific physical output and monitor relationship**.

Reuse the same monitor and GPU data.

---

## 9. Block storage topology

Relevant locations:

```text
/sys/block/
/sys/class/block/
/sys/devices/.../block/
```

Show:

- Device name
- Capacity
- Logical/physical sector sizes
- Rotational status
- Read-only state
- I/O scheduler
- Request queue properties
- Discard/TRIM capabilities
- Parent controller
- Partition relationships
- Device holders and slaves
- Mount and filesystem associations, where available

### Disk and partition previews

Use proportional partition maps, but avoid expensive scanning.

Show block-device topology:

`NVMe controller → namespace → partition → filesystem → mount`

For device-mapper, MD RAID, and loop devices, show their backing relationships.

### NVMe

Recognize NVMe controllers and namespaces.

Show:

- Controller identity
- Model and serial, where exposed
- Firmware revision
- Namespace identity
- Capacity
- PCIe association
- Available health information, where exposed

Do not assume SMART health information is universally available through sysfs. More detailed NVMe health may require explicit NVMe commands or other interfaces.

### Device-mapper

Use:

- `/sys/block/dm-N/`
- `/sys/block/dm-N/dm/`
- `holders/`
- `slaves/`

Show mapped-device names and dependencies.

### MD RAID

Recognize:

`/sys/block/mdN/md/`

Show:

- RAID level
- Array state
- Member devices
- Degraded status
- Resync/recovery state, if present

A member/status map is useful.

### Loop devices

Reuse the `/dev/loopN` inspector.

The sysfs view should emphasize backing-file relationships and block-device topology, while `/dev/loopN` emphasizes the exposed block interface.

---

## 10. Network interfaces

Relevant location:

`/sys/class/net/`

Show:

- Interface name
- Physical/virtual interface type
- Driver
- MAC address
- Carrier state
- Operational state
- MTU
- Negotiated speed and duplex, when exposed
- Parent physical device
- TX/RX queue counts
- Bridge/bond/VLAN relationships, where discoverable

Example:

`enp5s0 | Ethernet | 10 Gb/s full duplex | Carrier up | MTU 9000`

Or:

`wlan0 | Wi-Fi | Driver: iwlwifi | Carrier up`

### Important distinction

Sysfs does not provide a complete network-configuration model.

IP addresses, routes, neighbor tables, and many network relationships are better obtained through **rtnetlink**, not by scraping sysfs.

If the existing application already has netlink support, reuse it to enrich the inspector.

Avoid historical bandwidth graphs or continuous polling.

### Virtual networking

Recognize:

- Bridges
- VLAN interfaces
- Bonds
- Virtual Ethernet devices
- TUN/TAP interfaces
- WireGuard interfaces, where identifiable

Show meaningful relationships without assuming all interfaces have physical hardware.

---

## 11. CPU topology and caches

Relevant location:

`/sys/devices/system/cpu/`

Show:

- Online/offline status
- Logical CPU number
- Physical package ID
- Core ID
- SMT siblings
- Core siblings
- NUMA relationships
- Cache levels and sharing
- Current frequency, where available
- Frequency governor
- Frequency limits
- CPU isolation information, where exposed

### Individual logical CPU

Example:

`cpu6 | Package 0 · Core 3 · Thread 0 | Online | 4.2 GHz`

### Cache directories

For entries under `cpuN/cache/indexM/`, show:

- Cache level
- Instruction/data/unified
- Size
- Associativity, when exposed
- Shared CPU list

Example:

`L3 | Unified | 64 MiB | Shared CPUs 0–31`

### CPU masks

Parse Linux CPU-list formats such as:

`0-3,8-11`

Render them as compact topology or membership maps when useful.

### Difference from `/dev/cpu/N`

`/dev/cpu/N/cpuid` and `msr` are low-level CPU access interfaces.

Sysfs is the preferred source for CPU topology, online status, cache relationships, and frequency configuration.

Reuse the CPU inspector and avoid requiring access to MSRs.

---

## 12. NUMA topology

Relevant location:

`/sys/devices/system/node/`

Show:

- NUMA node number
- Associated CPUs
- Total/free memory, where exposed
- Node distance matrix
- Memory and CPU relationships

A small NUMA node topology visualization can be useful.

Example:

`node0 | CPUs 0–15 | 32 GiB memory | Distance to node1: 32`

Do not assume every machine has multiple NUMA nodes.

---

## 13. Hardware monitoring and sensors

Relevant location:

`/sys/class/hwmon/`

This is one of the highest-priority sysfs-specific preview families.

Recognize:

- Temperatures
- Voltages
- Currents
- Power
- Energy
- Fan RPM
- PWM controls
- Alarm flags
- Sensor labels
- Critical/minimum/maximum thresholds

### Important implementation detail

Use the `name` attribute and channel labels to identify the monitoring device and sensors.

A file named `temp1_input` is not inherently "CPU temperature".

Its meaning depends on the associated driver and labels.

### Semantic preview examples

`CPU Package | 61.4 °C | Critical 95 °C`

`Fan 2 | 1,240 RPM | PWM 42%`

`GPU Power | 210 W`

Display measurement units correctly:

- Temperature inputs commonly use millidegrees Celsius
- Power inputs commonly use microwatts
- Voltage inputs commonly use millivolts

But verify the relevant hwmon ABI for each attribute family.

### Visualization

A compact threshold bar is useful when both a current value and meaningful limits are available.

Do not fabricate safe ranges or warning thresholds.

### Difference from `/dev`

Many hwmon sensors have **no corresponding `/dev` node**.

They may be backed by PCI, I²C, ACPI, WMI, or platform devices.

Sysfs is the primary interface for their measurements.

---

## 14. Thermal zones and cooling devices

Relevant location:

`/sys/class/thermal/`

Recognize:

- `thermal_zoneN`
- `cooling_deviceN`

For thermal zones, show:

- Type
- Current temperature
- Trip points
- Trip types
- Associated cooling devices, where discoverable

For cooling devices, show:

- Type
- Current state
- Maximum state

Example:

`thermal_zone0 | CPU thermal | 58 °C | Passive trip 80 °C`

Do not treat cooling-device state as a percentage unless that interpretation is documented.

Cross-link thermal zones to hwmon sensors and underlying hardware where associations exist.

---

## 15. Power supplies and batteries

Relevant location:

`/sys/class/power_supply/`

Recognize:

- Batteries
- AC adapters
- USB power sources
- Other supported supply types

Show:

- Supply type
- Present/online status
- Charging/discharging status
- Capacity percentage
- Current energy or charge
- Full capacity
- Design capacity
- Voltage/current/power, where available
- Battery health derived from full/design capacity, when meaningful
- Manufacturer/model

Example:

`BAT0 | 82% | Discharging | 42 Wh / 51 Wh full`

### Important unit handling

Different devices expose energy in µWh and charge in µAh.

Do not mix charge and energy values without voltage information and a valid conversion.

Do not assume every power supply exposes every property.

### Difference from `/dev`

These objects often have no meaningful character-device counterpart.

Sysfs is their primary public interface.

---

## 16. Power management and wakeup

Relevant locations include:

```text
/sys/power/
/sys/devices/.../power/
```

Show:

- Runtime power-management policy
- Current runtime status
- Autosuspend settings
- Wakeup configuration
- Supported system sleep states
- Runtime suspend/resume counters, where available

Example:

`USB device | Runtime suspended | Autosuspend enabled`

These attributes are useful for debugging devices that refuse to suspend or wake unexpectedly.

Do not change power-management settings automatically.

Do not assume that a writable `power/control` file is safe to manipulate generically.

---

## 17. I²C buses and registered devices

Relevant location:

`/sys/bus/i2c/devices/`

Reuse the `/dev/i2c-*` inspector.

### Bus adapter

Show:

- Adapter name
- Parent controller
- Driver
- Registered devices
- Multiplexer topology

### Registered device

Show:

- Bus number
- I²C address
- Registered name
- Bound driver
- Parent adapter
- Related hwmon/input/other subsystem objects

Example:

`0-0048 | Temperature sensor | 0x48 | Driver: lm75`

Only identify the device as a temperature sensor when driver/name metadata supports that identification.

### Difference from `/dev`

`/dev/i2c-0` is the userspace I²C bus access interface.

`/sys/bus/i2c/devices/0-0048` is a specific kernel-registered device at address `0x48`.

A registered device does not necessarily have its own `/dev` node.

Never automatically probe unregistered I²C addresses.

---

## 18. SPI devices and controllers

Relevant location:

`/sys/bus/spi/devices/`

Show:

- SPI controller
- Bus number
- Chip-select number
- Device name
- Bound driver
- Parent controller
- Related device nodes, if any

Example:

`spi0.0 | SPI NOR flash | Chip select 0 | spi-nor`

SPI is not generally self-enumerating.

Only show devices registered with the kernel.

Do not attempt generic SPI probing.

This is particularly valuable for embedded Linux systems.

---

## 19. MTD and UBI flash

Relevant locations:

```text
/sys/class/mtd/
/sys/class/ubi/
```

Reuse the `/dev/mtd*` inspector.

### MTD

Show:

- Device/partition name
- Flash type
- Capacity
- Erase size
- Write size
- OOB/ECC metadata
- Bad-block counts, where available
- Parent device and partition relationships

### UBI

Show:

- UBI device
- Backing MTD device
- Volume names
- Volume types
- Logical eraseblock size
- Volume sizes
- Available/free eraseblocks
- Wear-leveling-related statistics, where exposed

Example:

`ubi0 | Backed by mtd3 | 2 volumes | rootfs + data`

### Difference from `/dev`

`/dev/mtd3` provides raw flash access.

`/sys/class/mtd/mtd3` describes flash geometry, identity, and relationships.

They should share the same metadata and flash-layout visualization.

---

## 20. Input devices

Relevant locations:

```text
/sys/class/input/
/sys/bus/hid/devices/
```

Show:

- Device name
- Physical connection
- Bus type
- Vendor/product identity
- Associated event, mouse, or joystick nodes
- Input capability bitmaps
- Driver
- Parent USB/Bluetooth/platform device

For HID devices, link the HID interface to its parent physical device.

### Difference from `/dev`

`/dev/input/eventN` is an event stream.

`/sys/class/input/eventN` identifies the input device, its capabilities, and its relationships.

Do not read input events for a preview.

---

## 21. ALSA sound hardware

Relevant locations:

```text
/sys/class/sound/
/sys/devices/.../sound/
```

Show:

- ALSA card number
- Card identity
- Driver
- Parent PCI/USB/platform device
- Playback/capture devices
- MIDI interfaces
- Associated `/dev/snd/*` nodes

Reuse the `/dev/snd` inspector.

The sysfs preview should emphasize the sound-card hardware hierarchy, while the `/dev` preview emphasizes the individual PCM, MIDI, or control interfaces.

Avoid opening audio streams or generating live meters.

---

## 22. WMI, ACPI, and platform devices

Relevant locations:

```text
/sys/bus/wmi/devices/
/sys/bus/acpi/devices/
/sys/bus/platform/devices/
```

Show:

- Device identity
- WMI GUID or ACPI HID/CID, where applicable
- Bound driver
- Parent firmware/platform device
- Associated standard Linux interfaces

These may lead to:

- hwmon sensors
- LEDs
- Input devices
- Battery/power controls
- Thermal zones
- Platform profiles
- RF kill switches

For recognized vendor-specific hardware, reuse existing Linux-exposed metadata.

For unknown firmware interfaces, a driver/identity preview is sufficient.

Do not invoke arbitrary firmware methods.

---

## 23. LEDs, backlights, and platform controls

Relevant locations:

```text
/sys/class/leds/
/sys/class/backlight/
/sys/firmware/acpi/platform_profile
```

The platform-profile path is not universal; also recognize supported device-specific platform-profile interfaces.

### LEDs

Show:

- LED name
- Current brightness
- Maximum brightness
- Active trigger
- Supported triggers

Example:

`input3::capslock | Off | Trigger: input3::capslock`

### Backlights

Show:

- Current brightness
- Maximum brightness
- Actual brightness, when exposed
- Backlight type

Example:

`amdgpu_bl1 | 65% brightness`

### Platform profiles

Where exposed, show:

- Current profile
- Available profiles

Example:

`Platform profile | Balanced | Quiet / Balanced / Performance`

Do not automatically change these values.

---

## 24. GPIO and PWM

Relevant locations:

```text
/sys/class/gpio/
/sys/class/pwm/
```

These are particularly interesting for embedded systems, but their availability depends on kernel configuration and driver support.

### GPIO

The legacy sysfs GPIO ABI is deprecated in favor of GPIO character devices (`/dev/gpiochipN`).

Recognize existing sysfs GPIO exports, but do not assume every GPIO line is visible there.

Show:

- GPIO controller
- Exported line number
- Direction
- Value, where safely readable
- Active-low configuration
- Edge configuration

Do not automatically export lines or change their direction.

For modern GPIO chips, reuse a `/dev/gpiochipN` inspector when implemented.

### PWM

Show:

- PWM controller
- Exported channel
- Period
- Duty cycle
- Polarity
- Enabled state

Example:

`pwmchip0/pwm0 | 20 kHz | 35% duty | Enabled`

Do not automatically export channels or modify PWM outputs.

---

## 25. USB Type-C and Thunderbolt

Relevant locations:

```text
/sys/class/typec/
/sys/bus/thunderbolt/devices/
```

These can expose particularly useful connection topology.

### USB Type-C

Where supported, show:

- Port identity
- Partner identity
- Power role
- Data role
- Orientation
- Alternate modes
- Cable/partner relationships
- USB Power Delivery information, where available

Example:

`port0 | Connected | DisplayPort Alt Mode | Source`

Do not assume every USB-C port exposes all these properties.

### Thunderbolt / USB4

Show:

- Domain
- Router/device identity
- Vendor/device
- Authorization state
- Connection topology
- Parent/child routers
- Driver

Example:

`USB4 router | Authorized | Downstream device connected`

These are especially well suited to topology diagrams.

---

## 26. Kernel modules and drivers

Relevant locations:

```text
/sys/module/
/sys/bus/*/drivers/
```

### Kernel modules

Show:

- Module name
- Version, when available
- Parameters
- Reference count, when exposed
- Holders/dependencies
- Associated drivers or devices, where discoverable

Example:

`amdgpu | Loaded | 3 parameters | Bound to PCI GPU`

Do not assume every module is dynamically loadable or that every driver corresponds to a module.

Built-in kernel drivers may appear without a separately loaded module.

### Driver directories

Show:

- Driver name
- Bus/subsystem
- Bound devices
- Associated module, if any

Example:

`cp210x | USB serial driver | 2 bound interfaces`

This is one of the most useful ways to navigate sysfs:

`Driver → Bound devices → Physical hardware → /dev interfaces`

Do not write to `bind`, `unbind`, or `new_id` automatically.

---

## 27. Firmware information

Relevant locations:

```text
/sys/firmware/
/sys/firmware/dmi/
/sys/firmware/efi/
/sys/firmware/devicetree/
```

### DMI/SMBIOS

Show available machine identity:

- System manufacturer
- Product/model
- BIOS/UEFI vendor
- Firmware version
- Board identity

Avoid unnecessarily exposing or indexing machine serial numbers and other unique identifiers.

### EFI

Recognize:

- EFI presence
- Runtime-service availability
- EFI variable interface

Do not read every EFI variable automatically.

Some firmware variables contain sensitive information, and writing them can affect boot behavior.

### Device tree

On embedded systems, recognize device-tree nodes and properties.

Show:

- Node name
- `compatible` strings
- `reg` addresses
- Interrupt relationships
- Status
- Parent/child hierarchy
- Associated platform device, where possible

Device-tree properties are often binary, big-endian encoded values, not ordinary UTF-8 text.

Use appropriate device-tree parsing rather than treating everything as plain text.

A compact device-tree hierarchy preview would be valuable.

---

## 28. Filesystem-specific sysfs objects

Relevant location:

`/sys/fs/`

This is not a hardware directory, but it still contains useful kernel objects.

Recognize supported filesystem-specific structures, particularly:

- Btrfs
- FUSE connections
- Cgroup hierarchy
- Other kernel-exposed filesystem instances

### Btrfs

Where exposed, show:

- Filesystem UUID
- Associated block devices
- Allocation/space information
- Feature flags
- Relevant filesystem state

Do not assume sysfs exposes the complete filesystem or subvolume inventory.

For richer Btrfs information, reuse existing filesystem inspection code or supported ioctls where appropriate.

### FUSE connections

Where exposed, show:

- Connection identifier
- Relevant state and limits

Do not abort or modify FUSE connections.

### Cgroups

If handling `/sys/fs/cgroup/`, implement a dedicated cgroup inspector rather than treating it as a generic hardware device.

Show:

- Hierarchy position
- Controllers
- CPU and memory limits
- Current resource usage
- Process membership
- Parent/child relationships

This is useful for debugging containers and systemd services.

Do not recursively enumerate enormous cgroup trees for every preview.

---

## 29. Kernel subsystem and system state

Relevant location:

`/sys/kernel/`

This contains various kernel interfaces rather than physical devices.

Recognize useful objects such as:

- IOMMU groups
- Security subsystem state
- Debug/tracing interfaces
- Kernel notes and metadata
- Live kernel state exposed through supported interfaces

### IOMMU groups

Reuse the VFIO/IOMMU inspector.

### Tracing

Recognize tracefs directories when mounted.

Show a simple description of tracing facilities and their current configuration where readable.

Do not enable tracing, consume trace streams, or modify tracepoints automatically.

### Security-related state

Treat security and lockdown attributes as potentially sensitive.

Use passive inspection only.

Do not imply that a readable attribute gives a complete picture of the system's security posture.

---

## 30. RTC and watchdog devices

Relevant locations:

```text
/sys/class/rtc/
/sys/class/watchdog/
```

### RTC

Show:

- RTC identity
- Driver
- Current RTC time/date, when exposed
- Wake-alarm capability/state, when available
- Parent device

Example:

`rtc0 | Hardware clock | UTC | Wake alarm supported`

### Watchdog

Show:

- Watchdog identity
- Driver
- Timeout
- Time left, when exposed
- Running/active state, where available

Example:

`watchdog0 | iTCO_wdt | Timeout 30 s`

**Do not open `/dev/watchdogN` for a preview.**

Opening a watchdog can activate watchdog behavior, and failing to service it can reboot the system.

This is a particularly important example of why `/sys` metadata is preferable to probing `/dev`.

---

## 31. Other subsystem families

The architecture should also support lightweight inspectors for less common sysfs classes and buses, including:

- `rfkill`: Radio block state and associated hardware
- `powercap`: Energy domains, counters, power limits
- `regulator`: Voltage regulators and constraints
- `devfreq`: Device frequency scaling
- `remoteproc`: Remote processor identity and state
- `rpmsg`: Messaging endpoints for remote processors
- `scsi_host`: SCSI host adapters and drivers
- `ata_port`: ATA ports and links
- `nvme`: Controller and namespace topology
- `mdio_bus`: Registered MDIO/PHY devices
- `iio`: Industrial I/O sensors and channels
- `pps`: Pulse-per-second sources
- `ptp`: Precision time protocol clock devices

These should initially receive generic semantic previews, with deeper specialized handling only where the exposed metadata is particularly valuable.

For IIO, PTP, PPS, remoteproc, and similar subsystems, avoid automatically activating data streams or changing device state.

---

## 32. Directory-specific summaries

Top-level sysfs directories deserve distinct summaries.

| Directory | Preview emphasis |
|---|---|
| `/sys/devices/` | Physical/virtual device hierarchy |
| `/sys/bus/` | Bus types, registered devices, drivers |
| `/sys/class/` | Functional device classes |
| `/sys/block/` | Block-device inventory and relationships |
| `/sys/dev/` | Major/minor → device mappings |
| `/sys/module/` | Loaded/built-in module objects and parameters |
| `/sys/firmware/` | Firmware interfaces and platform identity |
| `/sys/fs/` | Filesystem and cgroup kernel interfaces |
| `/sys/kernel/` | Kernel subsystems and global state |
| `/sys/power/` | Supported sleep states and system power configuration |

### `/sys/bus/`

Summarize available bus types and their device counts.

### `/sys/class/`

Summarize device classes such as DRM, net, sound, thermal, hwmon, and power_supply.

### `/sys/devices/`

Emphasize physical hierarchy, with optional collapsing of uninteresting intermediate directories.

### `/sys/dev/`

Recognize major/minor lookup symlinks and link them to corresponding `/dev` nodes.

---

## 33. Symlink handling and navigation

Sysfs relies heavily on symlinks.

Treat them as meaningful relationships, not clutter.

Important relationship types:

- Parent device
- Child device
- Bound driver
- Subsystem
- Class interface
- Block holder/slave
- IOMMU group
- Physical connection
- Associated device node
- Firmware node
- Power-management parent

Allow navigation in both directions when possible.

For example:

`/dev/ttyUSB0 → USB interface → USB device → USB hub → xHCI controller → PCI device`

Or:

`/sys/bus/pci/devices/0000:01:00.0 → DRM card0 → DP-1 → monitor`

A compact relationship inspector could be shared across `/dev`, `/sys`, and `/proc`.

---

## 34. Read safety and performance

Sysfs is generally safe to inspect, but it is **not an ordinary collection of static text files**.

Some attributes:

- Are generated dynamically when read
- Can disappear between directory enumeration and opening
- Are binary
- Are large or variable-sized
- Are restricted
- Represent live kernel state
- Have side effects or unusual read semantics
- Are writable controls for hardware

### Reading rules

- Use bounded reads.
- Do not assume `stat().st_size` represents the actual content length; many sysfs attributes report size zero.
- Do not blindly read every attribute in a directory.
- Avoid opening event/notification/stream interfaces automatically.
- Use documented attributes and known subsystem handlers.
- Handle device removal and `ENOENT` races gracefully.
- Handle permission failures gracefully.
- Do not assume every file is newline-terminated UTF-8.
- Do not follow arbitrary symlink chains indefinitely.
- Never write to sysfs during preview generation.

A generic fallback may read small, explicitly allowlisted textual attributes through the semantic collector. An unknown attribute receives metadata-only inspection until a handler documents its semantics and read policy. Apply the routing and collector-isolation rules in `/dev` section 1 to all file-manager entry points.

### Refresh behavior

Classify metadata by expected volatility.

**Mostly static:**

- Hardware identity
- Parent topology
- Driver
- Supported capabilities
- Firmware version

**Occasionally changing:**

- USB connection status
- DRM connector status
- Network carrier
- CPU online status
- Device power state

**Frequently changing:**

- Temperature
- Fan RPM
- CPU frequency
- Battery current/power
- Energy counters

Apply this initial refresh policy, with configurable internal limits rather than per-row controls:

- On entering a folder or opening an inspector, collect visible objects and revalidate cached identities. Show valid cached values while replacements load, with their age where relevant.
- A normal folder Refresh and the inspector's Refresh action request fresh visible facts, coalescing existing requests. Neither action starts optional device-opening inspections or repeats a VCS snapshot.
- Use udev events for structural changes where available. Detect event-source failure, invalidate affected relationships, and reject completions for retired object generations.
- Do not rely on inotify for `/proc`, `/sys`, or `/dev/pts`, or on attribute size/mtime for live-value freshness.
- With no usable event source, revalidate visible structural objects on entry/manual refresh and at most once every five seconds while visible. Stop this work when the folder/inspector is hidden; disclose that inventory is observed rather than exhaustive.
- For frequently changing measurements, refresh visible rows at most once every five seconds and the active inspector at most once per second. Share readings when several aliases display the same fact. No hidden-row telemetry collection.
- Mostly static values remain cached until identity revalidation or relevant invalidation; occasionally changing values use available events and the visible fallback interval.
- Mark dynamic values stale after two scheduled refresh intervals without a successful observation, and immediately on a relevant invalidation. Keep the last observation separate from the latest error; never present it as current.
- Show collection time/age in the inspector. Multi-attribute collections are not necessarily atomic; do not claim a single coherent snapshot unless the source guarantees one.
- Bound total collection/cache work and use backoff after repeated failures. Refresh rates are upper limits, not guarantees; collector admission budgets take precedence.

No historical sampling or background metric retention is required.

---

## 35. Shared architecture with `/dev`

Introduce the shared semantic model for both trees, reusing the existing file-manager preview and viewer lifecycle where appropriate. There is currently no complete device-inspector model to assume. Do not create an independent `/dev` model first and retrofit `/sys` afterward.

Suggested concepts:

### `KernelObject`

Represents a kernel-exposed entity.

Fields might include:

- Canonical sysfs path
- Observation namespace/mount context and lifetime generation
- Kernel object name
- Subsystem
- Bus
- Parent
- Driver
- Related device nodes
- Relationships
- Typed attributes

### `DeviceIdentity`

Represents the underlying hardware or virtual device identity.

Includes:

- Vendor/product
- Human-readable name
- Bus address
- Physical connection path
- Serial number, where appropriate
- Device class

### `DeviceInterface`

Represents a userspace access interface, such as:

- Character device
- Block device
- DRM render node
- ALSA PCM node
- V4L2 node
- USB device node
- Input event node

### `SysfsAttribute`

Represents a typed kernel attribute:

- Name
- Raw value
- Semantic value
- Units
- Allowed values, if known
- Readability
- Write capability
- Refresh policy
- Observation outcome, provenance, timestamp, and freshness

### `DeviceRelation`

Represents navigable links:

- Parent/child
- Driver binding
- Backing device
- Class interface
- Device node
- Firmware association
- Filesystem/mount
- Process association

### `SemanticPreview`

A compact presentation derived from the shared data model.

**The same hardware identity must not be independently rediscovered and cached by separate `/dev` and `/sys` implementations.**

Collectors should own metadata. Preview renderers should decide which facts matter for the current path.

---

## 36. Preview layout

Use the existing compact horizontal preview design.

For sysfs directories, emphasize:

**Object → relationship/topology → current state**

Examples:

- `1-3 | USB Audio Device | 3 interfaces | 480 Mb/s`
- `01:00.0 | RTX 3090 | PCIe 4.0 ×16 | nvidia`
- `card0-DP-1 | Connected | Monitor name | 3840×2160 supported`
- `enp5s0 | Ethernet | Carrier up | 10 Gb/s`
- `cpu6 | Core 3 · Thread 0 | Online | 4.2 GHz`
- `thermal_zone0 | CPU thermal | 58 °C | Trip 80 °C`
- `BAT0 | 82% | Discharging | 42 Wh remaining`
- `0-0048 | I²C 0x48 | lm75 | Temperature sensor`
- `mtd3 | rootfs | NAND | 59 MiB | 64 KiB erase`
- `vfio-pci | PCI driver | 2 bound devices`

For individual attributes, emphasize:

**Semantic label → formatted value → units/limits/context**

Examples:

- `temp1_input | CPU Package | 58.4 °C`
- `scaling_governor | schedutil | Available: performance, powersave, schedutil`
- `power/control | auto | Runtime PM enabled`
- `current_link_width | PCIe ×16`
- `operstate | Up`
- `brightness | 128 / 255`

All values must come from actual device data, not illustrative defaults.

Use diagrams only when they convey real relationships or limits.

---

## 37. Unified implementation order for `/dev` and `/sys`

This replaces separate implementation schedules. Each phase delivers a reviewable subset in both browsing trees; later subsystems reuse the same model, routing policy, helpers, and inspector.

### Phase 1: Shared foundation and generic inspection

1. Kernel-interface classification and routing across all preview, viewer, Shelf, clipboard, and operation entry points.
2. Namespace/mount context, alias resolution, object lifetime generations, and stale-completion rejection.
3. Bounded collector/helper admission, response deadlines, cancellation, partial observations, and provenance.
4. Shared `/dev` ↔ `/sys` object/interface relationships, parent/subsystem/driver discovery, and udev availability handling.
5. Typed attribute registry with a small documented read allowlist; generic device/directory previews and metadata-only unknown-attribute fallbacks.
6. Shared expanded inspector, directory inspection affordance, relationship actions, and browser/inspector history behavior.
7. Cache ownership, concrete refresh rules, and explicit read-only operation policy.
8. Static pseudo-device descriptions and the acceptance harness for routing and stalled collectors.

### Phase 2: High-value devices and process relationships

9. USB identity/interface topology and USB serial nodes.
10. PCI/PCIe topology, DRM nodes, connectors, and EDID.
11. Block devices, partitions, loop backing files, mount relationships, and basic device-mapper dependencies.
12. Shared throttled process/device association collector, including PID/start-time identity, namespace context, bounded scan coverage, and permission outcomes.
13. PTY/TTY shell/session/foreground-job relationships, using that shared collector. Associate other supported devices with open FDs without claiming active use.
14. Network interfaces, CPU topology/caches/frequency, hwmon, thermal zones, and power supplies/batteries.

### Phase 3: Debugging and embedded hardware

15. ALSA, V4L2/media, input/HID, and richer raw USB relationships.
16. I²C registered devices/muxes, SPI, MTD/UBI, and IOMMU/VFIO groups.
17. Driver/module relationships, WMI/ACPI/platform devices, firmware/device tree, and GPIO/PWM.
18. RTC/watchdog identity, USB-C/Thunderbolt, and deeper NVMe, MD RAID, device-mapper, and filesystem-specific state.
19. Explicit console snapshot capture and individually documented optional capability inspections. These require the optional-inspection helper and must not weaken passive routing.

### Phase 4: Specialized kernel objects and expanded topology

20. NUMA, cgroups, powercap/regulators, remoteproc/RPMsg, and IIO/PTP/PPS.
21. Kernel tracing/security metadata and additional supported subsystem families.
22. Expanded topology visualizations built from observed relationships.

Do not make a phase dependent on every possible device being present on the development machine. Unsupported hardware must remain navigable through generic metadata inspection.

---

## 38. Acceptance criteria

- `/sys` previews work without root wherever the kernel permits.
- Preview generation never writes to sysfs.
- Reads are bounded and limited to understood, safe attributes.
- The UI never blocks on sysfs inspection.
- Hardware identity is shared between `/dev` and `/sys`.
- Symlink aliases do not create duplicate physical-device identities.
- Distinct interfaces and child devices remain distinct objects.
- Parent/child, driver, bus, and device-node relationships are navigable.
- Individual sysfs attributes display appropriate types and units.
- Unknown attributes fall back gracefully without fabricated interpretations.
- Disconnected or disappearing hardware does not crash the inspector.
- Refresh is bounded and visibility-scoped; ordinary previews collect no hidden telemetry.
- No invented telemetry, thresholds, capabilities, or device state.
- Unsupported sysfs objects still receive a useful generic preview.

### Testable acceptance cases

Use injectable collector data and temporary filesystem fixtures for deterministic coverage, plus read-only integration checks on available hardware. UI automation uses the repository's private compositor runner. Tests must not write kernel attributes or open real device/event/watchdog nodes to manufacture fixtures.

- **Routing:** A regular-looking zero-size sysfs attribute, an alias/bind-mounted kernel interface, and unknown binary kernel attributes never reach generic text/media/hash/editor/capture readers. Ordinary files in `/dev/shm` retain ordinary handling.
- **Operations:** Mixed recursive copy/capture jobs touching a protected interface are rejected before admission; copying references/display snapshots performs no device read or automatic hydration. Protected destinations cannot receive ordinary writes.
- **Allowed reads:** Known attributes are bounded; oversized/non-UTF-8 data is marked truncated or unsupported. Malformed units, masks, EDID, and relationship data produce partial/error results without invented interpretations.
- **Removal/replacement:** Remove an object during collection, then reuse its path/device number. Old values and late completions must not attach to the replacement. Test reused PID/start-time pairs and independent devpts contexts as well.
- **Permissions/coverage:** Denied `/proc` FD access, denied attributes, incomplete scans, and vanished processes remain distinct from successful empty results. Passive access badges never claim verified openability.
- **Missing events:** With udev absent, failed, or incomplete, navigation/manual refresh and the bounded visible revalidation policy still work. Attribute mtime changes are not required for fresh values.
- **Stalled collectors:** Simulate a read that never returns and a helper that does not exit promptly. The UI response deadline holds, ordinary previews remain usable, late results are rejected, and live/stuck helpers cannot exceed admission budgets.
- **Refresh:** Hidden objects stop measurement work; aliases share reads; repeated failures back off; stale retained values show age/status and survive partial refresh without becoming current.
- **Interactions:** Directory double-click navigates; the inspection action opens the inspector; Ctrl/Shift and dragging preserve selection/reference behavior; browser and inspector Back actions restore the correct state.
- **Explicit inspection:** Ordinary row/hover/manual refresh never opens a VCS or other device node. Only a specifically requested optional inspection uses the separately budgeted helper. Console captures are bounded, session-local, and not automatically repeated or exported.
- **Regression:** Existing media/font/binary/text previews and ordinary file operations continue to use their established lifecycle outside protected interfaces.

Record read attempts and routing decisions in the fixture backend so absence of forbidden I/O is an assertion, not inferred from a lack of visible side effects.

## Final design principle

**`/dev` and `/sys` should be two views into one shared model of the system.**

`/dev` answers:

"What interface is available, what does it do, and what can access it?"

`/sys` answers:

"What kernel object is this, how is it connected, what driver manages it, and what attributes does it expose?"

Sometimes both views will show nearly identical information. That's desirable when the information is genuinely relevant to both.

Do not duplicate discovery logic merely to force the previews to look different.

The real benefit of `/sys` is the ability to navigate the Linux kernel's device graph and understand the relationships between hardware, drivers, buses, virtual devices, and their userspace interfaces.
