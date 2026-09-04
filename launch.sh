#!/usr/bin/env bash
# Little Screen Clip Management Script
# Usage: ./launch.sh [start|stop|restart|status|ui|logs|test|build]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PID_FILE="$SCRIPT_DIR/.little-clipd.pid"
LOG_FILE="$SCRIPT_DIR/little-clipd.log"
BIN_DIR="$SCRIPT_DIR/target/debug"
DAEMON_BIN="$BIN_DIR/little-clipd"
CLIPCTL_BIN="$BIN_DIR/clipctl"
WORKBENCH_BIN="$BIN_DIR/clip-workbench"
TEST_BIN="$BIN_DIR/test_screen"

SOCKET_PATH="${XDG_RUNTIME_DIR:-/tmp}/little-screen-clip.sock"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

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
        echo -e "${BLUE}Building workspace binaries...${NC}"
        cargo build --quiet
    fi
}

function start_daemon() {
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
        echo -e "${GREEN}✓ little-clipd started successfully!${NC} (PID: $daemon_pid)"
        echo -e "  Logs: ${CYAN}$LOG_FILE${NC}"
        echo -e "  IPC:  ${CYAN}$SOCKET_PATH${NC}"
    else
        echo -e "${RED}✗ little-clipd failed to stay alive.${NC} Check logs:"
        tail -n 15 "$LOG_FILE"
        rm -f "$PID_FILE"
        return 1
    fi
}

function stop_daemon() {
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
    echo -e "${GREEN}✓ little-clipd stopped.${NC}"
}

function status_daemon() {
    echo -e "${BOLD}${CYAN}=== LITTLE SCREEN CLIP STATUS ===${NC}"
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
    diff)
        shift
        run_workbench --diff "$@"
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
        echo -e "${BLUE}Building workspace...${NC}"
        cargo build
        echo -e "${GREEN}Build complete!${NC}"
        ;;
    *)
        status_daemon
        echo ""
        echo -e "${BOLD}Usage:${NC} ./launch.sh [command]"
        echo ""
        echo "Commands:"
        echo "  start     - Start little-clipd in the background"
        echo "  stop      - Stop running little-clipd daemon"
        echo "  restart   - Stop and restart little-clipd"
        echo "  reload    - Hot reload configuration file and refresh screen"
        echo "  status    - Show daemon status and current clipboard slots"
        echo "  ui        - Open the main monitor Crafting Workbench modal (supports --last)"
        echo "  diff      - Open the interactive Diff Viewer modal"
        echo "  audio <f> - Open audio file in full waveform preview & slicer"
        echo "  last      - Re-run last operation or recipe directly"
        echo "  logs      - Tail daemon output logs"
        echo "  test      - Run direct USB display hardware test"
        echo "  build     - Recompile all Rust workspace crates"
        ;;
esac

