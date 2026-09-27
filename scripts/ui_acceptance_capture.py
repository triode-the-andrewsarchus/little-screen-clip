#!/usr/bin/env python3
"""Capture an isolated baseline run of the workbench under nested Niri/Weston.

This runner only starts processes in a private runtime/config directory. It does
not connect to the user's compositor or daemon and does not access the user's
clipboard or USB display hardware. Screenshots are observations, not proof of first-frame
fidelity or parity across every screen and appearance.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import wave
from pathlib import Path
from typing import Any


APP_ID = "little-clip-workbench"
STARTUP_SECONDS = 20.0
IMAGE_TOOL_KEYS = {
    "view": "v",
    "crop": "c",
    "deskew": "k",
    "transform": "f",
    "redact": "x",
    "markup": "m",
    "pick-color": "p",
    "resize": "s",
    "convert": "t",
    "ocr": "o",
    "barcode": "b",
    "remove-background": "r",
    "depth-map": "d",
    "film-look": "l",
}
IMAGE_ACTION_KEYS = {"enter": "Return"}


class FakeAudioTransitionDaemon:
    """Serve one stable audio item whose asynchronous preview becomes ready."""

    ITEM_ID = "9b1de26a-4101-4e08-93d0-2ab14dad415f"
    AUDIO_PATH = "/tmp/little-clip-acceptance-async-audio.wav"

    def __init__(self, runtime: Path, log_path: Path, ready_delay: float = 2.0):
        self.socket_path = runtime / "little-screen-clip.sock"
        self.log_path = log_path
        self.rolling_audio_path = log_path.parent / "fake-rolling-audio.wav"
        with wave.open(str(self.rolling_audio_path), "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(48_000)
            wav.writeframes(bytes(48_000 * 2))
        self.ready_at = time.monotonic() + ready_delay
        self.started_at = time.monotonic()
        self.stop_event = threading.Event()
        self.lock = threading.Lock()
        self.events: list[dict[str, Any]] = []
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(self.socket_path))
        os.chmod(self.socket_path, 0o600)
        self.listener.listen(8)
        self.listener.settimeout(0.2)
        self.thread = threading.Thread(target=self._serve, name="fake-audio-ipc", daemon=True)
        self.thread.start()

    def _item(self, ready: bool) -> dict[str, Any]:
        waveform = None
        if ready:
            waveform = {
                "channels": 2,
                "sample_rate": 48000,
                "duration_ms": 12000,
                "ch1_peaks": [0.1, 0.65, 0.3, 0.9, 0.2, 0.55, 0.8, 0.35],
                "ch2_peaks": [0.45, 0.2, 0.75, 0.4, 0.9, 0.3, 0.6, 0.15],
                "spectral_colors": [
                    [44, 174, 202], [62, 194, 126], [230, 177, 78], [226, 106, 113],
                    [155, 116, 220], [54, 183, 174], [234, 139, 74], [98, 151, 224],
                ],
                "is_snippet": False,
                "snippet_start_ms": 0,
                "snippet_duration_ms": 0,
            }
        return {
            "id": self.ITEM_ID,
            "created_at": int(time.time()),
            "payload": {
                "AudioRef": {
                    "path": self.AUDIO_PATH,
                    "duration_ms": 12000,
                    "sample_rate": 48000,
                    "channels": 2,
                    "format": "WAV",
                    "waveform": waveform,
                }
            },
            "title": "async-preview-demo.wav",
            "subtitle": "00:12 • 48.0 kHz • Stereo (WAV)",
            "details": ["00:12", "Stereo"],
            "handler_tag": "AUDIO (FILE)",
        }

    def _state(self, ready: bool) -> dict[str, Any]:
        item = self._item(ready)
        return {
            "undo": None,
            "left_hand": item,
            "left_prior1": None,
            "left_prior2": None,
            "left_prior3": None,
            "right_prior2": None,
            "right_prior1": None,
            "right_hand": None,
            "cursor": "LeftHand",
            "selected": ["LeftHand"],
        }

    def _summary(self, ready: bool) -> dict[str, Any]:
        return {
            "cards": [{
                "slot": "LeftHand",
                "item_id": self.ITEM_ID,
                "payload_kind": "Audio",
                "preview_phase": "Enhanced" if ready else "Pending",
            }],
            "cursor": "LeftHand",
            "selected": ["LeftHand"],
        }

    def _response(self, request: Any) -> tuple[Any, dict[str, Any]]:
        ready = time.monotonic() >= self.ready_at
        if request == "GetState":
            return {"State": self._state(ready)}, {"kind": "state", "ready": ready}
        if request == "GetStateSummary":
            return {"StateSummary": self._summary(ready)}, {"kind": "summary", "ready": ready}
        if request == {"GetSlotItem": "LeftHand"}:
            return {"SlotItem": self._item(ready)}, {"kind": "slot_item", "ready": ready}
        if request == "GetRollingAudio":
            return {
                "RollingAudio": {
                    "path": str(self.rolling_audio_path),
                    "duration_secs": 1.0,
                    "sample_rate": 48_000,
                    "channels": 1,
                }
            }, {"kind": "rolling_audio"}
        if request == "GetAutoActions":
            return {"AutoActions": {}}, {"kind": "auto_actions"}
        return {"Error": {"error": f"unsupported acceptance request: {request!r}"}}, {"kind": "unsupported"}

    def _serve(self) -> None:
        while not self.stop_event.is_set():
            try:
                connection, _ = self.listener.accept()
            except TimeoutError:
                continue
            except OSError:
                break
            with connection:
                try:
                    request_line = connection.makefile("rb").readline(1_048_577)
                    request = json.loads(request_line)
                    response, event = self._response(request)
                    event.update({"request": request, "elapsed_ms": round((time.monotonic() - self.started_at) * 1000)})
                    with self.lock:
                        self.events.append(event)
                    connection.sendall((json.dumps(response, separators=(",", ":")) + "\n").encode())
                except (OSError, ValueError, TypeError) as exc:
                    with self.lock:
                        self.events.append({"kind": "protocol_error", "error": str(exc)})

    def close(self) -> dict[str, Any]:
        self.stop_event.set()
        self.listener.close()
        self.thread.join(timeout=1)
        with self.lock:
            events = list(self.events)
        self.log_path.write_text(json.dumps(events, indent=2) + "\n", encoding="utf-8")
        pending_states = [
            event for event in events if event.get("kind") == "state" and not event.get("ready")
        ]
        pending_summaries = [
            event for event in events if event.get("kind") == "summary" and not event.get("ready")
        ]
        ready_summaries = [
            event for event in events if event.get("kind") == "summary" and event.get("ready")
        ]
        ready_items = [
            event for event in events if event.get("kind") == "slot_item" and event.get("ready")
        ]
        ready_summary_index = next(
            (i for i, event in enumerate(events) if event.get("kind") == "summary" and event.get("ready")),
            None,
        )
        ready_item_index = next(
            (i for i, event in enumerate(events) if event.get("kind") == "slot_item" and event.get("ready")),
            None,
        )
        summary_before_item = (
            ready_summary_index is not None
            and ready_item_index is not None
            and ready_summary_index < ready_item_index
        )
        return {
            "socket": str(self.socket_path),
            "log": str(self.log_path),
            "rolling_audio_fixture": str(self.rolling_audio_path),
            "ready_delay_ms": round((self.ready_at - self.started_at) * 1000),
            "initial_pending_state_observed": bool(pending_states),
            "initial_pending_summary_observed": bool(pending_summaries),
            "ready_summary_observed": bool(ready_summaries),
            "ready_slot_item_requested": bool(ready_items),
            "ready_summary_precedes_item_fetch": summary_before_item,
            "pass": bool(
                pending_states
                and pending_summaries
                and ready_summaries
                and ready_items
                and summary_before_item
            ),
        }


def command_version(command: str) -> str | None:
    executable = shutil.which(command)
    if executable is None:
        return None
    try:
        result = subprocess.run(
            [executable, "--version"], capture_output=True, text=True, timeout=3
        )
        return (result.stdout or result.stderr).strip().splitlines()[0]
    except (OSError, subprocess.TimeoutExpired, IndexError):
        return None


def run_checked(args: list[str], *, env: dict[str, str], timeout: float = 5) -> str:
    result = subprocess.run(args, capture_output=True, text=True, env=env, timeout=timeout)
    if result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}): {args!r}: "
            f"{(result.stderr or result.stdout).strip()}"
        )
    return result.stdout


def build_private_virtual_pointer(output: Path) -> Path:
    """Build the tiny client used only against this run's private Niri socket."""
    scanner, compiler, pkg_config = (
        shutil.which("wayland-scanner"), shutil.which("cc"), shutil.which("pkg-config")
    )
    if not scanner or not compiler or not pkg_config:
        raise RuntimeError("pointer input requires wayland-scanner, cc, and pkg-config")
    try:
        flags = subprocess.run(
            [pkg_config, "--cflags", "--libs", "wayland-client"],
            capture_output=True, text=True, timeout=5, check=True,
        ).stdout
    except (OSError, subprocess.SubprocessError) as exc:
        raise RuntimeError(f"pointer input requires wayland-client development files: {exc}") from exc

    build_dir = output / "private-pointer-build"
    build_dir.mkdir(mode=0o700)
    os.chmod(build_dir, 0o700)
    header = build_dir / "virtual-pointer-client-protocol.h"
    protocol_code = build_dir / "virtual-pointer-protocol.c"
    binary = build_dir / "virtual-pointer"
    repo = Path(__file__).resolve().parent.parent
    protocol_xml = repo / "scripts/virtual-pointer-unstable-v1.xml"
    client_code = repo / "scripts/ui_acceptance_virtual_pointer.c"
    run_checked([scanner, "client-header", str(protocol_xml), str(header)], env=os.environ.copy())
    run_checked([scanner, "private-code", str(protocol_xml), str(protocol_code)], env=os.environ.copy())
    try:
        subprocess.run(
            [compiler, "-std=c11", "-Wall", "-Wextra", "-I", str(build_dir), "-o", str(binary),
             str(client_code), str(protocol_code), *shlex.split(flags)],
            capture_output=True, text=True, timeout=15, check=True,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        detail = getattr(exc, "stderr", "")
        raise RuntimeError(f"failed to build private virtual-pointer helper: {detail or exc}") from exc
    return binary


def verify_private_wayland_target(env: dict[str, str], runtime: Path) -> None:
    """Refuse pointer input unless this environment targets this run's Niri socket."""
    if "WAYLAND_SOCKET" in env:
        raise RuntimeError("refusing pointer input: inherited WAYLAND_SOCKET could bypass the private display path")
    if Path(env.get("XDG_RUNTIME_DIR", "")).resolve() != runtime.resolve():
        raise RuntimeError("refusing pointer input: XDG_RUNTIME_DIR is not this run's private runtime")
    display = env.get("WAYLAND_DISPLAY", "")
    if not display or "/" in display or not (runtime / display).is_socket():
        raise RuntimeError("refusing pointer input: WAYLAND_DISPLAY is not a socket in the private runtime")
    niri_socket = Path(env.get("NIRI_SOCKET", ""))
    if niri_socket.parent.resolve() != runtime.resolve() or not niri_socket.is_socket():
        raise RuntimeError("refusing pointer input: NIRI_SOCKET is not this run's private control socket")


def start_process(
    args: list[str], *, env: dict[str, str], cwd: Path, log_path: Path | None = None
) -> subprocess.Popen[Any]:
    log = log_path.open("wb") if log_path else subprocess.DEVNULL
    try:
        return subprocess.Popen(
            args,
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    finally:
        if log_path:
            log.close()


def stop_process(proc: subprocess.Popen[Any] | None) -> None:
    if proc is None or proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait(timeout=3)


def parse_json(text: str, label: str) -> Any:
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"{label} returned invalid JSON: {exc}") from exc


def own_windows(niri: Path, env: dict[str, str], own_pids: set[int]) -> list[dict[str, Any]]:
    result = parse_json(
        run_checked([str(niri), "msg", "--json", "windows"], env=env), "niri windows"
    )
    if not isinstance(result, list):
        raise RuntimeError("niri windows response was not a JSON list")
    return [
        window
        for window in result
        if isinstance(window, dict)
        and window.get("app_id") == APP_ID
        and window.get("pid") in own_pids
    ]


def layout_size(window: dict[str, Any]) -> tuple[int, int] | None:
    layout = window.get("layout")
    size = layout.get("window_size") if isinstance(layout, dict) else None
    if isinstance(size, list) and len(size) == 2 and all(isinstance(value, (int, float)) for value in size):
        return round(size[0]), round(size[1])
    return None


def geometry_position(window: dict[str, Any]) -> tuple[float, float] | None:
    """Read Niri's position inside the workspace view."""
    layout = window.get("layout")
    if not isinstance(layout, dict):
        return None
    position = layout.get("tile_pos_in_workspace_view")
    if isinstance(position, list) and len(position) == 2 and all(isinstance(value, (int, float)) for value in position):
        return float(position[0]), float(position[1])
    return None


def layout_tile_size(window: dict[str, Any]) -> tuple[float, float] | None:
    layout = window.get("layout")
    size = layout.get("tile_size") if isinstance(layout, dict) else None
    if isinstance(size, list) and len(size) == 2 and all(isinstance(value, (int, float)) for value in size):
        return float(size[0]), float(size[1])
    return None


def output_logical_size(outputs: Any) -> tuple[int, int] | None:
    candidates = outputs.values() if isinstance(outputs, dict) else outputs if isinstance(outputs, list) else []
    for output in candidates:
        if isinstance(output, dict) and isinstance(output.get("logical"), dict):
            logical = output["logical"]
            width, height = logical.get("width"), logical.get("height")
            if isinstance(width, (int, float)) and isinstance(height, (int, float)):
                return round(width), round(height)
    return None


def read_json_events(path: Path) -> list[dict[str, Any]]:
    """Decode valid object records from Niri's newline-delimited event log."""
    if not path.exists():
        return []
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    decoded: list[dict[str, Any]] = []
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            decoded.append(event)
    return decoded


def window_layout_events(path: Path, app_pid: int) -> list[dict[str, Any]]:
    """Associate open/change and layout-change records with the app window."""
    matching: list[dict[str, Any]] = []
    app_window_id: int | None = None
    for event in read_json_events(path):
        payload = event.get("WindowOpenedOrChanged")
        if isinstance(payload, dict):
            candidate = payload.get("window", payload)
            if isinstance(candidate, dict) and candidate.get("pid") == app_pid and candidate.get("app_id") == APP_ID:
                app_window_id = candidate.get("id")
                matching.append({**candidate, "_event_type": "WindowOpenedOrChanged"})
                continue
        layout_change = event.get("WindowLayoutsChanged")
        changes = layout_change.get("changes", []) if isinstance(layout_change, dict) else []
        if app_window_id is not None and isinstance(changes, list):
            for change in changes:
                if not isinstance(change, list) or len(change) != 2 or change[0] != app_window_id:
                    continue
                previous = matching[-1]
                layout = change[1]
                if isinstance(layout, dict):
                    matching.append({**previous, "layout": layout, "_event_type": "WindowLayoutsChanged"})
    return matching


def wayland_size_hints(log_path: Path) -> dict[str, list[int] | None]:
    """Extract the initial xdg_toplevel min/max constraints from WAYLAND_DEBUG."""
    hints: dict[str, list[int] | None] = {"min_size": None, "max_size": None}
    if not log_path.exists():
        return hints
    try:
        lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return hints
    import re

    pattern = re.compile(r"xdg_toplevel[^\n]*(?:#|\.|\s)set_(min|max)_size\((-?\d+),\s*(-?\d+)\)")
    for line in lines:
        match = pattern.search(line)
        if match and hints[f"{match.group(1)}_size"] is None:
            hints[f"{match.group(1)}_size"] = [int(match.group(2)), int(match.group(3))]
    return hints


def screenshot(niri: Path, env: dict[str, str], path: Path) -> dict[str, Any]:
    started = time.monotonic()
    run_checked(
        [str(niri), "msg", "action", "screenshot-screen", "--show-pointer=false", "--path", str(path)],
        env=env,
        timeout=10,
    )
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if path.is_file() and path.stat().st_size:
            return {"path": str(path), "bytes": path.stat().st_size, "elapsed_ms": round((time.monotonic() - started) * 1000)}
        time.sleep(0.1)
    raise RuntimeError(f"screenshot did not appear: {path}")


def geometry_observation(window: dict[str, Any], logical_size: tuple[int, int]) -> dict[str, Any]:
    size = layout_size(window)
    position, tile_size = geometry_position(window), layout_tile_size(window)
    result: dict[str, Any] = {
        "window_size": list(size) if size else None,
        "position": list(position) if position else None,
        "tile_size": list(tile_size) if tile_size else None,
    }
    if position is not None and tile_size is not None:
        center = [position[0] + tile_size[0] / 2, position[1] + tile_size[1] / 2]
        expected = [logical_size[0] / 2, logical_size[1] / 2]
        error = [abs(center[index] - expected[index]) for index in range(2)]
        result.update({"center": center, "expected_center": expected, "center_error_px": error})
    result["centered"] = "center_error_px" in result and max(result["center_error_px"]) <= 1.0
    return result


def inspect_geometry(
    first_windows: list[dict[str, Any]],
    final_windows: list[dict[str, Any]],
    layout_events: list[dict[str, Any]],
    logical_size: tuple[int, int],
    hints: dict[str, list[int] | None],
    report: dict[str, Any],
) -> None:
    expected = [math.floor(logical_size[0] * 0.92), math.floor(logical_size[1] * 0.92)]
    first = first_windows[0] if first_windows else {}
    final = final_windows[0] if final_windows else {}
    initial_event = next((event for event in layout_events if event.get("_event_type") == "WindowOpenedOrChanged"), {})
    first_observation = geometry_observation(first, logical_size)
    final_observation = geometry_observation(final, logical_size)
    event_observations = [geometry_observation(event, logical_size) for event in layout_events]
    initial_observation = geometry_observation(initial_event, logical_size)
    event_history_ok = bool(event_observations) and all(
        item.get("window_size") == expected and item.get("centered") for item in event_observations
    )
    checks = {
        "first_observed_window_size_matches_floor_92_percent": first_observation["window_size"] == expected,
        "final_window_size_matches_floor_92_percent": final_observation["window_size"] == expected,
        "first_and_final_window_size_stable": first_observation["window_size"] is not None
        and first_observation["window_size"] == final_observation["window_size"],
        "initial_wayland_min_max_hints_match_expected_size": hints["min_size"] == expected and hints["max_size"] == expected,
        "initial_window_opened_event_size_matches_floor_92_percent": initial_observation.get("window_size") == expected,
        "initial_window_opened_event_centered_within_1_logical_pixel": initial_observation.get("centered") is True,
        "first_observed_tile_centered_within_1_logical_pixel": first_observation["centered"],
        "final_tile_centered_within_1_logical_pixel": final_observation["centered"],
        "all_app_layout_events_remained_expected_and_centered": event_history_ok,
    }
    report["initial_window_event"] = initial_event or None
    report["layout_event_history"] = [
        {"event": event.get("_event_type"), "window_id": event.get("id"), "geometry": observation}
        for event, observation in zip(layout_events, event_observations)
    ]
    report["initial_wayland_size_hints"] = hints
    report["geometry"] = {
        "expected_window_size": expected,
        "first_observed": first_observation,
        "final": final_observation,
        "initial_event": initial_observation,
        "layout_event_count": len(layout_events),
    }
    report["checks"] = checks
    for name, passed in checks.items():
        if not passed:
            report["failures"].append(f"runtime check failed: {name}")


class ChildProcesses:
    """Own every spawned process and clean up its isolated process group."""

    def __init__(self) -> None:
        self._processes: dict[str, subprocess.Popen[Any]] = {}

    def start(
        self,
        name: str,
        args: list[str],
        *,
        env: dict[str, str],
        cwd: Path,
        log_path: Path,
    ) -> subprocess.Popen[Any]:
        proc = start_process(args, env=env, cwd=cwd, log_path=log_path)
        self._processes[name] = proc
        return proc

    def cleanup(self) -> None:
        for proc in reversed(list(self._processes.values())):
            stop_process(proc)

    def exit_statuses(self) -> dict[str, int | None]:
        return {name: proc.returncode for name, proc in self._processes.items()}


def prepare_configuration(args: argparse.Namespace, output: Path) -> dict[str, Any]:
    runtime, config_home, scratch = (output / name for name in ("runtime", "config", "scratch"))
    for directory in (runtime, config_home, scratch):
        directory.mkdir(mode=0o700)
        os.chmod(directory, 0o700)
    config = f'[workbench]\nappearance = "{args.appearance}"\n'
    (scratch / "config.toml").write_text(config, encoding="utf-8")
    user_config = config_home / "little-screen-clip" / "config.toml"
    user_config.parent.mkdir(mode=0o700)
    user_config.write_text(config, encoding="utf-8")
    niri_config = output / "niri.kdl"
    niri_config.write_text(
        "hotkey-overlay { skip-at-startup; }\n"
        "prefer-no-csd\n"
        f'output "winit" {{ scale {args.scale}; }}\n'
        'window-rule { match app-id="little-clip-workbench"; open-floating true; }\n',
        encoding="utf-8",
    )
    clean_env = os.environ.copy()
    for key in ("DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "NIRI_SOCKET", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"):
        clean_env.pop(key, None)
    clean_env.update({"LIBGL_ALWAYS_SOFTWARE": "1", "XDG_RUNTIME_DIR": str(runtime), "XDG_CONFIG_HOME": str(config_home)})
    niri_bin, weston_bin = shutil.which("niri"), shutil.which("weston")
    if not niri_bin or not weston_bin:
        raise RuntimeError("both niri and weston must be installed")
    try:
        run_checked([niri_bin, "validate", "--config", str(niri_config)], env=clean_env, timeout=5)
    except (RuntimeError, subprocess.TimeoutExpired) as exc:
        raise RuntimeError(f"temporary Niri configuration validation failed: {exc}") from exc
    return {
        "runtime": runtime,
        "scratch": scratch,
        "clean_env": clean_env,
        "niri_bin": Path(niri_bin),
        "weston_bin": Path(weston_bin),
        "niri_config": niri_config,
    }


def wait_for_private_niri(
    niri_proc: subprocess.Popen[Any], niri_bin: Path, runtime: Path, env: dict[str, str]
) -> tuple[Path, dict[str, str], Any]:
    deadline = time.monotonic() + STARTUP_SECONDS
    niri_socket: Path | None = None
    while time.monotonic() < deadline:
        if niri_proc.poll() is not None:
            raise RuntimeError("Niri exited during startup")
        for candidate in runtime.glob("niri.*.sock"):
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                    probe.settimeout(0.15)
                    probe.connect(str(candidate))
                niri_socket = candidate
                break
            except OSError:
                continue
        if niri_socket:
            break
        time.sleep(0.1)
    if niri_socket is None or niri_socket.parent.resolve() != runtime.resolve():
        raise RuntimeError("timed out waiting for Niri's private control socket")
    parts = niri_socket.name.split(".")
    if len(parts) < 4 or parts[0] != "niri":
        raise RuntimeError(f"unexpected private Niri socket name: {niri_socket.name}")
    niri_env = env.copy()
    niri_env.update({"NIRI_SOCKET": str(niri_socket), "WAYLAND_DISPLAY": ".".join(parts[1:-2])})
    outputs: Any = None
    deadline = time.monotonic() + STARTUP_SECONDS
    while time.monotonic() < deadline:
        if niri_proc.poll() is not None:
            raise RuntimeError("Niri exited before reporting outputs")
        try:
            outputs = parse_json(
                run_checked([str(niri_bin), "msg", "--json", "outputs"], env=niri_env, timeout=3),
                "niri outputs",
            )
            if output_logical_size(outputs) is not None:
                return niri_socket, niri_env, outputs
        except (OSError, RuntimeError, subprocess.TimeoutExpired):
            time.sleep(0.1)
    raise RuntimeError("timed out waiting for Niri outputs on the private control socket")


def start_compositors(
    config: dict[str, Any], args: argparse.Namespace, output: Path, children: ChildProcesses
) -> tuple[Path, dict[str, str], Any]:
    runtime, scratch = config["runtime"], config["scratch"]
    weston_env = config["clean_env"].copy()
    weston_env["WAYLAND_DISPLAY"] = "clip-e1-parent"
    weston_socket = runtime / "clip-e1-parent"
    physical_width = math.floor(args.width * args.scale + 0.5)
    physical_height = math.floor(args.height * args.scale + 0.5)
    weston = children.start(
        "weston",
        [str(config["weston_bin"]), "--backend=headless", "--renderer=pixman", "--shell=kiosk",
         f"--width={physical_width}", f"--height={physical_height}", "--socket=clip-e1-parent",
         "--no-config", "--idle-time=0", "--fake-seat"],
        env=weston_env, cwd=scratch, log_path=output / "weston.log",
    )
    deadline = time.monotonic() + STARTUP_SECONDS
    while not weston_socket.exists():
        if weston.poll() is not None:
            raise RuntimeError(f"Weston exited during startup; see {output / 'weston.log'}")
        if time.monotonic() >= deadline:
            raise RuntimeError("timed out waiting for private Weston socket")
        time.sleep(0.1)

    nested_env = config["clean_env"].copy()
    nested_env["WAYLAND_DISPLAY"] = "clip-e1-parent"
    niri = children.start(
        "niri", [str(config["niri_bin"]), "--config", str(config["niri_config"])],
        env=nested_env, cwd=scratch, log_path=output / "niri.log",
    )
    socket_path, niri_env, outputs = wait_for_private_niri(
        niri, config["niri_bin"], runtime, config["clean_env"]
    )
    return socket_path, niri_env, outputs


def app_arguments(args: argparse.Namespace, binary: Path) -> list[str]:
    command = [str(binary)]
    if args.component_gallery:
        command.append("--component-gallery")
    if args.image:
        command.extend(["--image", str(args.image.resolve())])
    if args.audio:
        command.extend(["--audio", str(args.audio.resolve())])
    if args.pdf:
        command.extend(["--pdf", str(args.pdf.resolve())])
    if args.midi:
        command.extend(["--midi", str(args.midi.resolve())])
    return command


def wait_for_app_window(
    niri_bin: Path, niri_env: dict[str, str], app: subprocess.Popen[Any], output: Path
) -> tuple[list[dict[str, Any]], int]:
    started = time.monotonic()
    deadline = started + STARTUP_SECONDS
    while time.monotonic() < deadline:
        if app.poll() is not None:
            raise RuntimeError(
                f"workbench exited before mapping (status {app.returncode}); see {output / 'workbench.log'}"
            )
        windows = own_windows(niri_bin, niri_env, {app.pid})
        if windows:
            return windows, round((time.monotonic() - started) * 1000)
        time.sleep(0.1)
    raise RuntimeError("timed out waiting for workbench window owned by this run")


def check_singleton(
    app: subprocess.Popen[Any],
    second: subprocess.Popen[Any],
    niri_bin: Path,
    niri_env: dict[str, str],
) -> dict[str, Any]:
    try:
        second.wait(timeout=3)
        second_status = second.returncode
    except subprocess.TimeoutExpired:
        second_status = None
    time.sleep(0.25)
    windows = own_windows(niri_bin, niri_env, {app.pid, second.pid})
    focused = len(windows) == 1 and windows[0].get("pid") == app.pid and windows[0].get("is_focused") is True
    return {
        "second_pid": second.pid,
        "second_exit_status_within_3s": second_status,
        "windows_after_second_invocation": windows,
        "single_window_remained": len(windows) == 1,
        "focused_first_process_window": focused,
        "pass": focused and second_status == 0,
    }


def capture_workbench(
    args: argparse.Namespace,
    binary: Path,
    output: Path,
    config: dict[str, Any],
    niri_env: dict[str, str],
    outputs: Any,
    children: ChildProcesses,
    report: dict[str, Any],
) -> None:
    niri_bin, scratch = config["niri_bin"], config["scratch"]
    logical_size = output_logical_size(outputs)
    if logical_size is None:
        raise RuntimeError("Niri outputs response did not include logical width and height")
    report["outputs"] = outputs
    report["actual_logical_size"] = list(logical_size)
    app_env = niri_env | {"WAYLAND_DEBUG": "client"}
    pointer_helper: Path | None = None
    if args.click is not None:
        verify_private_wayland_target(niri_env, config["runtime"])
        pointer_helper = build_private_virtual_pointer(output)
        probe = run_checked([str(pointer_helper), "probe"], env=niri_env)
        report["pointer_input"] = {
            "transport": "zwlr_virtual_pointer_v1",
            "target_runtime": str(config["runtime"]),
            "target_wayland_display": niri_env["WAYLAND_DISPLAY"],
            "protocol_probe": probe.strip(),
            "coordinate_space": "private Niri output logical coordinates",
        }
    command = app_arguments(args, binary)
    children.start(
        "event_stream", [str(niri_bin), "msg", "--json", "event-stream"],
        env=niri_env, cwd=scratch, log_path=output / "niri-events.jsonl",
    )
    app = children.start(
        "workbench", command, env=app_env, cwd=scratch, log_path=output / "workbench.log",
    )
    first_windows, mapped_ms = wait_for_app_window(niri_bin, niri_env, app, output)
    report["mapped_elapsed_ms"] = mapped_ms
    report["first_observed_windows"] = first_windows
    report["screenshots"]["first_observed"] = screenshot(
        niri_bin, niri_env, output / "first-observed.png"
    )
    if args.image_tool:
        run_checked(["wtype", "-k", IMAGE_TOOL_KEYS[args.image_tool]], env=niri_env)
        if args.image_action:
            time.sleep(0.1)
            run_checked(["wtype", "-k", IMAGE_ACTION_KEYS[args.image_action]], env=niri_env)
    def send_private_scroll() -> None:
        assert pointer_helper is not None and args.scroll is not None
        x, y, delta = args.scroll
        run_checked(
            [str(pointer_helper), "scroll", str(logical_size[0]), str(logical_size[1]),
             str(x), str(y), str(delta)],
            env=niri_env,
        )
        report["pointer_input"]["scroll"] = {
            "x": x, "y": y, "delta_logical_units": delta, "axis": "vertical"
        }
        time.sleep(0.5)

    if args.click is not None and pointer_helper is not None:
        report["pointer_input"]["clicks"] = []
        for click_index, (x, y) in enumerate(args.click, start=1):
            run_checked(
                [str(pointer_helper), "click", str(logical_size[0]), str(logical_size[1]), str(x), str(y)],
                env=niri_env,
            )
            report["pointer_input"]["clicks"].append({"x": x, "y": y, "button": "left"})
            time.sleep(0.5)
            if args.scroll_after_click == click_index:
                send_private_scroll()
        if args.scroll is not None and args.scroll_after_click is None:
            send_private_scroll()
    time.sleep(args.settle_ms / 1000)
    final_windows = own_windows(niri_bin, niri_env, {app.pid})
    report["final_windows"] = final_windows
    capture_name = f"image-{args.image_tool}" if args.image_tool else "settled"
    if args.image_action:
        capture_name = f"{capture_name}-{args.image_action}"
    report["screenshots"][capture_name] = screenshot(
        niri_bin, niri_env, output / f"{capture_name}.png"
    )

    second = children.start(
        "second_invocation", command, env=app_env, cwd=scratch,
        log_path=output / "second-invocation.log",
    )
    report["singleton_check"] = check_singleton(app, second, niri_bin, niri_env)
    if not report["singleton_check"]["pass"]:
        report["failures"].append("second launch did not cleanly focus/reuse the single existing popup")

    layout_events = window_layout_events(output / "niri-events.jsonl", app.pid)
    hints = wayland_size_hints(output / "workbench.log")
    inspect_geometry(first_windows, final_windows, layout_events, logical_size, hints, report)

    if args.verify_return:
        run_checked(["wtype", "-k", "Escape"], env=niri_env)
        try:
            app.wait(timeout=STARTUP_SECONDS)
            app_exited = True
        except subprocess.TimeoutExpired:
            app_exited = False
        time.sleep(0.25)
        remaining_windows = own_windows(niri_bin, niri_env, {app.pid, second.pid})
        compositor_outputs = parse_json(
            run_checked([str(niri_bin), "msg", "--json", "outputs"], env=niri_env),
            "niri outputs after workbench close",
        )
        compositor_responsive = output_logical_size(compositor_outputs) == logical_size
        report["return_check"] = {
            "escape_sent": True,
            "workbench_exited": app_exited,
            "workbench_exit_status": app.returncode,
            "remaining_workbench_windows": remaining_windows,
            "niri_responsive_with_same_output": compositor_responsive,
            "pass": app_exited and not remaining_windows and compositor_responsive,
        }
        report["screenshots"]["after-close"] = screenshot(
            niri_bin, niri_env, output / "after-close.png"
        )
        if not report["return_check"]["pass"]:
            report["failures"].append("Escape did not return cleanly to the responsive private Niri session")


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path, help="new directory for report, logs, and PNG captures")
    parser.add_argument("--width", type=int, default=1280, help="Weston logical width (default: 1280)")
    parser.add_argument("--height", type=int, default=800, help="Weston logical height (default: 800)")
    parser.add_argument("--scale", type=float, default=1.0, help="nested Niri output scale (default: 1)")
    parser.add_argument("--appearance", choices=("light", "dark"), required=True)
    parser.add_argument("--component-gallery", action="store_true", help="open the workbench component gallery directly")
    parser.add_argument(
        "--image-tool",
        choices=tuple(IMAGE_TOOL_KEYS),
        help="select an Image Studio tool by its existing keyboard shortcut before the settled capture",
    )
    parser.add_argument(
        "--image-action",
        choices=tuple(IMAGE_ACTION_KEYS),
        help="send one supported key action after selecting the image tool",
    )
    parser.add_argument("--image", type=Path, help="optional existing image fixture")
    parser.add_argument(
        "--click", nargs=2, type=int, metavar=("X", "Y"), action="append",
        help="send a left click at logical screen coordinates through private Niri; repeat to perform ordered UI interactions",
    )
    parser.add_argument(
        "--scroll", nargs=3, type=int, metavar=("X", "Y", "DELTA"),
        help="send one vertical wheel step at logical screen coordinates through private Niri; positive DELTA scrolls down",
    )
    parser.add_argument(
        "--scroll-after-click", type=int,
        help="place --scroll after this one-based --click number (default: after all clicks)",
    )
    parser.add_argument("--audio", type=Path, help="optional existing audio fixture")
    parser.add_argument("--pdf", type=Path, help="optional existing PDF fixture")
    parser.add_argument("--midi", type=Path, help="optional existing MIDI fixture")
    parser.add_argument(
        "--fake-audio-transition",
        action="store_true",
        help="serve a private pending-to-ready audio slot over IPC while the workbench stays open",
    )
    parser.add_argument("--verify-return", action="store_true", help="send Escape and verify the popup closes while private Niri remains responsive")
    parser.add_argument("--binary", type=Path, help="workbench binary (default: target/debug/clip-workbench)")
    parser.add_argument("--settle-ms", type=int, default=500, help="settling delay before second capture (default: 500)")
    return parser


def validate_cli(args: argparse.Namespace, repo: Path) -> Path:
    if min(args.width, args.height, args.scale) <= 0 or not math.isfinite(args.scale) or args.settle_ms < 0:
        raise SystemExit("width, height, and scale must be positive; settle-ms cannot be negative")
    for fixture in (args.image, args.audio, args.pdf, args.midi):
        if fixture is not None and not fixture.is_file():
            raise SystemExit(f"fixture does not exist: {fixture}")
    if args.fake_audio_transition and any((args.image, args.audio, args.pdf, args.midi)):
        raise SystemExit("--fake-audio-transition cannot be combined with file-entry fixtures")
    if args.image_tool and not args.image:
        raise SystemExit("--image-tool requires --image")
    if args.click is not None and any(
        not (0 <= x < args.width) or not (0 <= y < args.height) for x, y in args.click
    ):
        raise SystemExit("--click coordinates must be inside the requested logical output")
    if args.scroll is not None and (
        not (0 <= args.scroll[0] < args.width) or not (0 <= args.scroll[1] < args.height)
        or args.scroll[2] == 0 or abs(args.scroll[2]) > 1000
    ):
        raise SystemExit("--scroll coordinates must be inside the requested output and DELTA must be 1..1000 in magnitude")
    if args.scroll is not None and args.click is None:
        raise SystemExit("--scroll requires --click so the pointer action remains an explicit post-selection interaction")
    if args.scroll_after_click is not None and args.scroll is None:
        raise SystemExit("--scroll-after-click requires --scroll")
    if args.scroll_after_click is not None and (
        args.click is None or not (1 <= args.scroll_after_click <= len(args.click))
    ):
        raise SystemExit("--scroll-after-click must identify an existing one-based --click")
    if args.image_action and not args.image_tool:
        raise SystemExit("--image-action requires --image-tool")
    if args.verify_return and shutil.which("wtype") is None:
        raise SystemExit("--verify-return requires wtype")
    if args.image_tool and shutil.which("wtype") is None:
        raise SystemExit("--image-tool requires wtype")
    binary = (args.binary or (repo / "target/debug/clip-workbench")).resolve()
    if not binary.is_file():
        raise SystemExit(f"workbench binary does not exist: {binary}")
    return binary


def main() -> int:
    args = create_parser().parse_args()
    repo = Path(__file__).resolve().parent.parent
    binary = validate_cli(args, repo)
    output = args.output_dir.expanduser().resolve()
    try:
        output.mkdir(parents=True, exist_ok=False, mode=0o700)
    except FileExistsError:
        raise SystemExit(f"output directory already exists: {output}")
    os.chmod(output, 0o700)

    started = time.time()
    report: dict[str, Any] = {
        "status": "running",
        "started_at_unix": started,
        "configuration": {
            "logical_size": [args.width, args.height],
            "scale": args.scale,
            "appearance": args.appearance,
            "image_tool": args.image_tool,
            "image_action": args.image_action,
            "image_fixture": str(args.image.resolve()) if args.image else None,
            "click": args.click,
            "scroll": args.scroll,
            "scroll_after_click": args.scroll_after_click,
            "audio_fixture": str(args.audio.resolve()) if args.audio else None,
            "pdf_fixture": str(args.pdf.resolve()) if args.pdf else None,
            "midi_fixture": str(args.midi.resolve()) if args.midi else None,
            "fake_audio_transition": args.fake_audio_transition,
            "verify_return": args.verify_return,
            "binary": str(binary),
            "settle_ms": args.settle_ms,
        },
        "versions": {
            "niri": command_version("niri"),
            "weston": command_version("weston"),
            "python": sys.version.split()[0],
        },
        "commit": None,
        "result_scope": "capture and explicit runtime checks only; screenshots do not establish visual or performance parity",
        "screenshots": {},
        "failures": [],
        "limitations": [
            "The first-observed screenshot is not proof of the first visible frame.",
            "This run records observations and timing; it does not prove Light/Dark parity or a full performance profile.",
            "--verify-return checks popup dismissal and Niri responsiveness, not focus restoration to another application.",
        ],
    }
    children = ChildProcesses()
    fake_audio_daemon: FakeAudioTransitionDaemon | None = None
    try:
        report["commit"] = run_checked(
            ["git", "-C", str(repo), "rev-parse", "HEAD"], env=os.environ.copy(), timeout=3
        ).strip()
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        pass

    try:
        config = prepare_configuration(args, output)
        socket_path, niri_env, outputs = start_compositors(config, args, output, children)
        report["runtime"] = {
            "socket": str(socket_path),
            "wayland_display": niri_env["WAYLAND_DISPLAY"],
        }
        if args.fake_audio_transition:
            fake_audio_daemon = FakeAudioTransitionDaemon(
                config["runtime"], output / "fake-daemon-events.json"
            )
        capture_workbench(args, binary, output, config, niri_env, outputs, children, report)
    except Exception as exc:
        report["status"] = "failed"
        report["failures"].append(str(exc))
    finally:
        children.cleanup()
        if fake_audio_daemon is not None:
            transition = fake_audio_daemon.close()
            report["fake_audio_transition"] = transition
            if not transition["pass"]:
                report["failures"].append("fake daemon did not observe a ready-summary hydration request")
            elif report.get("mapped_elapsed_ms") is None:
                transition["first_observed_before_ready_transition"] = False
            elif report["mapped_elapsed_ms"] < transition["ready_delay_ms"]:
                transition["first_observed_before_ready_transition"] = True
            else:
                transition["first_observed_before_ready_transition"] = False
                report["failures"].append("workbench mapped after the fake preview was already ready")
        if report["status"] == "running":
            report["status"] = "passed" if not report["failures"] else "failed"
        report["processes"] = children.exit_statuses()
        report["finished_at_unix"] = time.time()
        report["duration_ms"] = round((report["finished_at_unix"] - started) * 1000)
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(json.dumps({"status": report["status"], "report": str(output / "report.json"), "failures": report["failures"]}, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
