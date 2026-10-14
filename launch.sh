#!/usr/bin/env bash
# Little Screen Clip Management Script
# Usage: ./launch.sh [start|stop|restart|status|ui|logs|test|build|deps|distro]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PID_FILE="$SCRIPT_DIR/.little-clipd.pid"
LOG_FILE="$SCRIPT_DIR/little-clipd.log"
# Prefer release build if available, otherwise fallback to debug
if [ -x "$SCRIPT_DIR/target/release/little-clipd" ] || [ -x "$SCRIPT_DIR/target/release/clip-workbench" ]; then
    BIN_DIR="$SCRIPT_DIR/target/release"
else
    BIN_DIR="$SCRIPT_DIR/target/debug"
fi
DAEMON_BIN="$BIN_DIR/little-clipd"
CLIPCTL_BIN="$BIN_DIR/clipctl"
WORKBENCH_BIN="$BIN_DIR/clip-workbench"
TEST_BIN="$BIN_DIR/test_screen"

SOCKET_PATH="${XDG_RUNTIME_DIR:-/tmp}/little-screen-clip.sock"
SYSTEMD_SERVICE_NAME="little-screen-clip.service"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Distribution and Dependency Management
function detect_distro_id() {
    local distro="unknown"
    if [ -f /etc/os-release ]; then
        distro=$(. /etc/os-release 2>/dev/null && echo "${ID:-unknown}")
    elif [ -f /usr/lib/os-release ]; then
        distro=$(. /usr/lib/os-release 2>/dev/null && echo "${ID:-unknown}")
    elif command -v lsb_release >/dev/null 2>&1; then
        distro=$(lsb_release -is 2>/dev/null | tr '[:upper:]' '[:lower:]')
    elif [ -f /etc/debian_version ]; then
        distro="debian"
    elif [ "$(uname -s)" = "Darwin" ]; then
        distro="macos"
    elif [ "$(uname -s)" = "Linux" ]; then
        distro="linux"
    fi
    echo "$distro"
}

function detect_distro() {
    detect_distro_id
}

function detect_distro_name() {
    local name=""
    if [ -f /etc/os-release ]; then
        name=$(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-${NAME:-${ID:-unknown}}}")
    elif [ -f /usr/lib/os-release ]; then
        name=$(. /usr/lib/os-release 2>/dev/null && echo "${PRETTY_NAME:-${NAME:-${ID:-unknown}}}")
    elif command -v lsb_release >/dev/null 2>&1; then
        name=$(lsb_release -ds 2>/dev/null || echo "Linux")
    elif [ -f /etc/debian_version ]; then
        name="Debian $(cat /etc/debian_version 2>/dev/null)"
    elif [ "$(uname -s)" = "Darwin" ]; then
        name="macOS"
    else
        name="$(uname -s)"
    fi
    echo "$name"
}

function is_ubuntu_or_debian() {
    local id="" id_like=""
    if [ -f /etc/os-release ]; then
        id=$(. /etc/os-release 2>/dev/null && echo "${ID:-}")
        id_like=$(. /etc/os-release 2>/dev/null && echo "${ID_LIKE:-}")
    elif [ -f /usr/lib/os-release ]; then
        id=$(. /usr/lib/os-release 2>/dev/null && echo "${ID:-}")
        id_like=$(. /usr/lib/os-release 2>/dev/null && echo "${ID_LIKE:-}")
    elif [ -f /etc/debian_version ]; then
        return 0
    fi

    case "$id" in
        ubuntu|debian|pop|linuxmint|elementary|zorin)
            return 0
            ;;
    esac

    for like in $id_like; do
        if [ "$like" = "ubuntu" ] || [ "$like" = "debian" ]; then
            return 0
        fi
    done

    return 1
}

UBUNTU_DEPS=(
    "libheif-dev"
    "libasound2-dev"
    "libgstreamer1.0-dev"
    "libgstreamer-plugins-base1.0-dev"
    "gstreamer1.0-plugins-base"
    "gstreamer1.0-plugins-good"
    "gstreamer1.0-plugins-bad"
    "gstreamer1.0-plugins-ugly"
    "gstreamer1.0-libav"
    "libgstrtspserver-1.0-dev"
    "libges-1.0-dev"
)

DEPS_CHECKED=0

function ensure_dependencies() {
    local verbose="${1:-0}"
    if [ "$DEPS_CHECKED" = "1" ] && [ "$verbose" != "1" ]; then
        return 0
    fi
    DEPS_CHECKED=1

    local distro_name distro_id
    distro_name="$(detect_distro_name)"
    distro_id="$(detect_distro_id)"

    if is_ubuntu_or_debian; then
        if ! command -v dpkg-query >/dev/null 2>&1; then
            if [ "$verbose" = "1" ]; then
                echo -e "${YELLOW}dpkg-query not available; skipping Debian/Ubuntu dependency check.${NC}"
            fi
            return 0
        fi

        local missing_deps=()
        for pkg in "${UBUNTU_DEPS[@]}"; do
            if [ "$(dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null)" != "install ok installed" ]; then
                missing_deps+=("$pkg")
            fi
        done

        if [ ${#missing_deps[@]} -eq 0 ]; then
            if [ "$verbose" = "1" ]; then
                echo -e "${GREEN}All required dependencies are installed for ${distro_name}.${NC}"
            fi
            return 0
        fi

        echo -e "${YELLOW}Detected missing dependencies for ${distro_name}:${NC}"
        for pkg in "${missing_deps[@]}"; do
            echo -e "  ${CYAN}-${NC} $pkg"
        done

        local sudo_cmd=""
        if [ "$EUID" -ne 0 ]; then
            if command -v sudo >/dev/null 2>&1; then
                sudo_cmd="sudo"
            else
                echo -e "${YELLOW}Warning: 'sudo' not available and not root; unable to auto-install dependencies.${NC}"
                echo -e "${YELLOW}Please install them manually:${NC}"
                echo -e "  sudo apt-get install -y ${missing_deps[*]}"
                return 0
            fi
        fi

        echo -e "${BLUE}Attempting best-effort installation of missing dependencies...${NC}"
        if $sudo_cmd apt-get install -y "${missing_deps[@]}"; then
            echo -e "${GREEN}Dependencies installed successfully!${NC}"
        elif $sudo_cmd apt-get update && $sudo_cmd apt-get install -y "${missing_deps[@]}"; then
            echo -e "${GREEN}Dependencies installed successfully!${NC}"
        else
            echo -e "${YELLOW}Warning: Failed to install some dependencies automatically.${NC}"
            echo -e "${YELLOW}You can try installing them manually:${NC}"
            echo -e "  sudo apt-get install -y ${missing_deps[*]}"
            echo -e "${YELLOW}Continuing build anyway (best-effort)...${NC}"
        fi
    else
        if [ "$verbose" = "1" ]; then
            echo -e "${BLUE}Detected distro: ${distro_name} (${distro_id})${NC}"
            echo -e "${YELLOW}Automatic dependency installation is currently configured for Ubuntu/Debian.${NC}"
            echo -e "Ensure development packages for libheif, ALSA, and GStreamer 1.0 are installed if build issues arise."
        fi
    fi

    return 0
}

function is_systemd_run() {
    [ -n "$LITTLE_SCREEN_CLIP_SYSTEMD" ]
}

function stop_systemd_service_if_active() {
    if ! is_systemd_run && command -v systemctl >/dev/null 2>&1; then
        if systemctl --user is-active --quiet "$SYSTEMD_SERVICE_NAME" 2>/dev/null; then
            echo -e "${YELLOW}Detected active systemd service '$SYSTEMD_SERVICE_NAME'. Stopping it for development session...${NC}"
            systemctl --user stop "$SYSTEMD_SERVICE_NAME" 2>/dev/null || true
            sleep 0.5
        fi
    fi
}

function is_running() {
    if [ -f "$PID_FILE" ]; then
        local pid
        pid=$(cat "$PID_FILE" 2>/dev/null || true)
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
    fi
    # Fallback to pgrep
    if pgrep -x "little-clipd" >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

function get_pid() {
    if [ -f "$PID_FILE" ]; then
        cat "$PID_FILE" 2>/dev/null || true
    else
        pgrep -x "little-clipd" | head -n 1
    fi
}

function build_if_needed() {
    if [ ! -x "$DAEMON_BIN" ] || [ ! -x "$WORKBENCH_BIN" ] || [ ! -x "$CLIPCTL_BIN" ]; then
        ensure_dependencies
        echo -e "${BLUE}Building workspace binaries (release mode)...${NC}"
        cargo build --release
        sweep_target
        BIN_DIR="$SCRIPT_DIR/target/release"
        DAEMON_BIN="$BIN_DIR/little-clipd"
        CLIPCTL_BIN="$BIN_DIR/clipctl"
        WORKBENCH_BIN="$BIN_DIR/clip-workbench"
        TEST_BIN="$BIN_DIR/test_screen"
    fi
}

function start_daemon() {
    stop_systemd_service_if_active

    if is_systemd_run && is_running; then
        echo -e "${YELLOW}Stopping manual little-clipd instance before systemd takeover...${NC}"
        stop_daemon
    fi

    if is_running; then
        echo -e "${YELLOW}little-clipd is already running${NC} (PID: $(get_pid))"
        return 0
    fi

    build_if_needed

    echo -e "${BLUE}Starting little-clipd in background...${NC}"
    # Remove stale socket
    rm -f "$SOCKET_PATH"

    setsid "$DAEMON_BIN" > "$LOG_FILE" 2>&1 < /dev/null &
    local daemon_pid=$!
    disown "$daemon_pid" 2>/dev/null || true
    echo "$daemon_pid" > "$PID_FILE"

    # Wait up to 3 seconds for socket creation
    local waited=0
    while [ ! -S "$SOCKET_PATH" ] && [ $waited -lt 15 ]; do
        sleep 0.2
        waited=$((waited + 1))
    done

    if kill -0 "$daemon_pid" 2>/dev/null; then
        echo -e "${GREEN}little-clipd started successfully!${NC} (PID: $daemon_pid)"
        echo -e "  Logs: ${CYAN}$LOG_FILE${NC}"
        echo -e "  IPC:  ${CYAN}$SOCKET_PATH${NC}"
    else
        echo -e "${RED}little-clipd failed to stay alive.${NC} Check logs:"
        tail -n 15 "$LOG_FILE"
        rm -f "$PID_FILE"
        return 1
    fi
}

function stop_daemon() {
    if ! is_systemd_run && command -v systemctl >/dev/null 2>&1; then
        if systemctl --user is-active --quiet "$SYSTEMD_SERVICE_NAME" 2>/dev/null; then
            echo -e "${YELLOW}Stopping systemd service '$SYSTEMD_SERVICE_NAME'...${NC}"
            systemctl --user stop "$SYSTEMD_SERVICE_NAME" 2>/dev/null || true
        fi
    fi

    if ! is_running; then
        echo -e "${YELLOW}little-clipd is not running.${NC}"
        rm -f "$PID_FILE"
        return 0
    fi

    local pid
    pid=$(get_pid)
    echo -e "${YELLOW}Stopping little-clipd${NC} (PID: $pid)..."

    kill "$pid" 2>/dev/null || true
    pkill -x "little-clipd" 2>/dev/null || true

    local count=0
    while is_running && [ $count -lt 10 ]; do
        sleep 0.2
        count=$((count + 1))
    done

    if is_running; then
        echo -e "${RED}Force killing little-clipd...${NC}"
        pkill -9 -x "little-clipd" 2>/dev/null || true
    fi

    rm -f "$PID_FILE" "$SOCKET_PATH"
    echo -e "${GREEN}little-clipd stopped.${NC}"
}

function status_daemon() {
    echo -e "${BOLD}${CYAN}=== LITTLE SCREEN CLIP STATUS ===${NC}"
    echo -e "Distro:  $(detect_distro_name) ($(detect_distro_id))"
    if is_running; then
        local pid
        pid=$(get_pid)
        echo -e "Daemon:  ${GREEN}RUNNING${NC} (PID: $pid)"
        echo -e "Socket:  ${GREEN}ACTIVE${NC} ($SOCKET_PATH)"
        echo -e "Log:     $LOG_FILE"
        echo ""
        if [ -x "$CLIPCTL_BIN" ]; then
            "$CLIPCTL_BIN" status 2>/dev/null || echo -e "${YELLOW}(clipctl could not query daemon state)${NC}"
        fi
    else
        echo -e "Daemon:  ${RED}STOPPED${NC}"
        echo -e "Socket:  $( [ -S "$SOCKET_PATH" ] && echo -e "${YELLOW}STALE${NC}" || echo "INACTIVE" )"
    fi
}

function show_logs() {
    if [ -f "$LOG_FILE" ]; then
        echo -e "${BLUE}Following daemon logs ($LOG_FILE)... (Ctrl+C to exit)${NC}"
        tail -f -n 30 "$LOG_FILE"
    else
        echo -e "${YELLOW}No log file found at $LOG_FILE${NC}"
    fi
}

function run_workbench() {
    build_if_needed
    if ! is_running; then
        echo -e "${YELLOW}Daemon is not running. Starting daemon first...${NC}"
        start_daemon
    fi
    echo -e "${GREEN}Launching Crafting Workbench modal...${NC}"
    "$WORKBENCH_BIN" "$@"
}

function run_file_manager() {
    ensure_dependencies
    "$SCRIPT_DIR/scripts/build_timelens.sh"
    "$SCRIPT_DIR/scripts/build_video_strip.sh"
    local file_manager_bin="$SCRIPT_DIR/target/release/clip-file-manager"
    if [ ! -x "$file_manager_bin" ]; then
        file_manager_bin="$SCRIPT_DIR/target/debug/clip-file-manager"
    fi
    if [ ! -x "$file_manager_bin" ]; then
        cargo build --release --bin clip-file-manager
        sweep_target
        file_manager_bin="$SCRIPT_DIR/target/release/clip-file-manager"
    fi
    "$file_manager_bin" "$@"
}

function run_test() {
    if is_running; then
        echo -e "${YELLOW}Stopping daemon temporarily to release USB device for test...${NC}"
        stop_daemon
        local restart_needed=1
    fi

    echo -e "${BLUE}Running direct screen hardware test...${NC}"
    cargo run --bin test_screen

    if [ "$restart_needed" = "1" ]; then
        echo -e "${BLUE}Restarting daemon...${NC}"
        start_daemon
    fi
}

function copy_right() {
    build_if_needed
    "$CLIPCTL_BIN" copy --hand right "$@"
}

function paste_right() {
    build_if_needed
    "$CLIPCTL_BIN" paste --hand right "$@"
}

function install_service() {
    local service_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    mkdir -p "$service_dir"
    local target_unit="$service_dir/$SYSTEMD_SERVICE_NAME"
    local src_unit="$SCRIPT_DIR/little-screen-clip.service"

    echo -e "${BLUE}Installing systemd service to $target_unit...${NC}"
    cp "$src_unit" "$target_unit"

    systemctl --user daemon-reload
    systemctl --user enable "$SYSTEMD_SERVICE_NAME"
    echo -e "${GREEN}Systemd service installed and enabled!${NC}"
    echo -e "  To start:  ${CYAN}systemctl --user start $SYSTEMD_SERVICE_NAME${NC} or ${CYAN}./launch.sh service start${NC}"
    echo -e "  To status: ${CYAN}systemctl --user status $SYSTEMD_SERVICE_NAME${NC} or ${CYAN}./launch.sh service status${NC}"
}

function manage_service() {
    local cmd="${1:-status}"
    shift || true
    case "$cmd" in
        start|stop|restart|status|enable|disable)
            systemctl --user "$cmd" "$SYSTEMD_SERVICE_NAME" "$@"
            ;;
        logs)
            journalctl --user -u "$SYSTEMD_SERVICE_NAME" -f "$@"
            ;;
        *)
            echo "Usage: ./launch.sh service [start|stop|restart|status|enable|disable|logs]"
            ;;
    esac
}

function sweep_target() {
    local keep_n="${1:-2}"
    local maxsize="${LITTLE_CLIP_TARGET_MAXSIZE:-20GB}"
    if [[ ! "$keep_n" =~ ^[0-9]+$ ]]; then
        echo -e "${RED}Incremental retention must be a non-negative integer.${NC}" >&2
        return 1
    fi
    local target_dir
    target_dir=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
    echo -e "${BLUE}Sweeping target artifacts with cargo-sweep...${NC}"
    if command -v cargo-sweep >/dev/null 2>&1; then
        # --installed alone keeps every artifact made by the current compiler.
        # Size eviction must run first: toolchain inspection touches fingerprints
        # and would otherwise make old artifacts look recently used.
        cargo sweep --maxsize "$maxsize"
        cargo sweep --installed
    else
        echo -e "${YELLOW}cargo-sweep unavailable; clearing debug artifacts with cargo clean --profile dev. Install cargo-sweep to retain a bounded cache.${NC}"
        cargo clean --profile dev
    fi

    echo -e "${BLUE}Pruning stale incremental compilation sessions (retaining newest ${keep_n})...${NC}"
    python3 - "$target_dir" "$keep_n" <<'PY'
import shutil
import sys
from pathlib import Path

target_dir = Path(sys.argv[1])
keep_n = int(sys.argv[2])
total_freed = 0
total_removed = 0
for inc_root in target_dir.glob('**/incremental'):
    for crate_dir in inc_root.iterdir():
        if not crate_dir.is_dir() or crate_dir.is_symlink():
            continue
        # rustc stores sessions inside each crate/configuration directory.
        sessions = [p for p in crate_dir.glob('s-*') if p.is_dir() and not p.is_symlink()]
        sessions.sort(key=lambda p: p.stat().st_mtime, reverse=True)
        for p in sessions[keep_n:]:
            sz = sum(f.stat().st_size for f in p.rglob('*') if f.is_file())
            shutil.rmtree(p)
            # Session lock names omit the final working-directory hash.
            (crate_dir / (p.name.rsplit('-', 1)[0] + '.lock')).unlink(missing_ok=True)
            total_freed += sz
            total_removed += 1
if total_freed > 0:
    print(f'Pruned {total_removed} stale incremental session(s), freeing {total_freed / (1024**3):.2f} GB.')
else:
    print('Incremental cache is clean (no sessions beyond retention threshold).')
PY
    echo -e "${GREEN}Target sweep completed!${NC}"
}

# Subcommand routing
case "${1:-}" in
    start)
        start_daemon
        ;;
    stop)
        stop_daemon
        ;;
    restart)
        stop_daemon
        sleep 0.5
        start_daemon
        ;;
    reload)
        if [ -x "$CLIPCTL_BIN" ]; then
            "$CLIPCTL_BIN" reload
        else
            echo "clipctl not built yet. Run ./launch.sh build first."
        fi
        ;;
    status)
        status_daemon
        ;;
    ui|workbench)
        shift
        run_workbench "$@"
        ;;
    files)
        shift
        run_file_manager "$@"
        ;;
    copy-right|copy_right)
        shift
        copy_right "$@"
        ;;
    paste-right|paste_right)
        shift
        paste_right "$@"
        ;;
    install-service)
        install_service
        ;;
    service)
        shift
        manage_service "$@"
        ;;
    diff)
        shift
        run_workbench --diff "$@"
        ;;
    bdiff|binary-diff)
        shift
        run_workbench --binary "$@"
        ;;
    audio)
        shift
        run_workbench --audio "$@"
        ;;
    last)
        shift
        run_workbench --last "$@"
        ;;
    logs)
        show_logs
        ;;
    test)
        run_test
        ;;
    build)
        shift || true
        ensure_dependencies
        echo -e "${BLUE}Building workspace (release mode)...${NC}"
        cargo build --release "$@"
        sweep_target
        if [ -x "$SCRIPT_DIR/target/release/clip-file-manager" ]; then
            "$SCRIPT_DIR/scripts/build_timelens.sh"
            "$SCRIPT_DIR/scripts/build_video_strip.sh"
        fi
        echo -e "${GREEN}Build complete! (target/release)${NC}"
        ;;
    sweep)
        shift || true
        sweep_target "${1:-2}"
        ;;
    deps|dependencies)
        ensure_dependencies 1
        ;;
    distro)
        echo -e "Distro ID:   $(detect_distro_id)"
        echo -e "Distro Name: $(detect_distro_name)"
        ;;
    *)
        status_daemon
        echo ""
        echo -e "${BOLD}Usage:${NC} ./launch.sh [command]"
        echo ""
        echo "Commands:"
        echo "  start           - Start little-clipd in the background (auto-stops systemd service)"
        echo "  stop            - Stop running little-clipd daemon (also stops systemd if active)"
        echo "  restart         - Stop and restart little-clipd"
        echo "  reload          - Hot reload configuration file and refresh screen"
        echo "  status          - Show daemon status and current clipboard slots"
        echo "  ui              - Open the main monitor Crafting Workbench modal (supports --last)"
        echo "  files [folder]  - Open the standalone file manager (defaults to home)"
        echo "  copy-right      - Capture active window selection to the Right-Hand buffer"
        echo "  paste-right     - Paste content from the Right-Hand buffer into active window"
        echo "  install-service - Install and enable systemd user service ($SYSTEMD_SERVICE_NAME)"
        echo "  service <cmd>   - Control systemd service (start, stop, status, restart, logs)"
        echo "  diff            - Open the interactive Diff Viewer modal"
        echo "  bdiff           - Open the interactive Biodiff-style Binary Diff Viewer modal"
        echo "  audio <f>       - Open audio file in full waveform preview & slicer"
        echo "  last            - Re-run last operation or recipe directly"
        echo "  logs            - Tail daemon output logs"
        echo "  test            - Run direct USB display hardware test"
        echo "  build           - Recompile all Rust workspace crates in release mode and sweep stale artifacts"
        echo "  sweep [N]       - Bound Cargo cache (LITTLE_CLIP_TARGET_MAXSIZE, default: 20GB) and keep N incremental sessions (default: 2)"
        echo "  deps            - Check and install required system dependencies (Ubuntu/Debian best-effort)"
        echo "  distro          - Display detected Linux distribution"
        ;;
esac
