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
import struct
import subprocess
import sys
import threading
import time
import wave
import zipfile
import zlib
from pathlib import Path
from typing import Any

from ui_acceptance_fixtures import pdf_fixture


APP_ID = "little-clip-workbench"
STARTUP_SECONDS = 20.0
FALLBACK_WINDOW_SIZE = (960, 540)
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
KEY_ACTIONS = {
    "ctrl-z": ["-M", "ctrl", "-k", "z", "-m", "ctrl"],
    "ctrl-y": ["-M", "ctrl", "-k", "y", "-m", "ctrl"],
    "enter": ["-k", "Return"],
    "f3": ["-k", "F3"],
    "f6": ["-k", "F6"],
    "r": ["-k", "r"],
    "ctrl-f": ["-M", "ctrl", "-k", "f", "-m", "ctrl"],
}


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




class FakeTextPreviewDaemon:
    """Serve one Markdown clipboard item from the private acceptance runtime."""

    ITEM_ID = "9b1de26a-4101-4e08-93d0-2ab14dad415e"
    DIFF_LEFT_ITEM_ID = "3f6ea64c-9b6a-4db8-a3dd-108c92b70b21"
    DIFF_RIGHT_ITEM_ID = "aa64938e-27ae-4f16-861f-8a24dfd5d2e2"
    PDF_ITEM_ID = "5d74ae02-65cb-44db-991a-5b5837642d09"
    TEXT = (
        "# Markdown Preview\n\n"
        "A body paragraph with **emphasis**, `inline code`, and a [link](https://example.invalid).\n\n"
        "## Lists and quotes\n\n"
        "- First item with enough words to demonstrate wrapping inside the text viewer.\n"
        "- Second item with a nested detail:\n  - Nested item remains aligned under its parent.\n\n"
        "> A block quote should have a clear boundary and readable line spacing at both host sizes.\n\n"
        "1. Ordered item one\n2. Ordered item two\n\n"
        "```rust\nfn preview() -> &'static str { \"ready\" }\n```\n"
    )
    CSV = (
        "Name,Category,Amount,Notes\n"
        "Widget,Office,12.50,Standard item\n"
        "Gadget,Tools,38.00,Includes small parts\n"
        "Notebook,Office,4.25,Plain ruled pages\n"
        "Cable,Electronics,9.99,Two meter braided cable\n"
    )

    def __init__(
        self,
        runtime: Path,
        log_path: Path,
        csv_preview: bool = False,
        xlsx_preview: bool = False,
        model3d_variant: str | None = None,
        binary_diff_preview: bool = False,
        diff_preview: bool = False,
        image_diff_preview: bool = False,
        pdf_preview: bool = False,
        reload_config_error: bool = False,
        midi_export: bool = False,
    ):
        self.socket_path = runtime / "little-screen-clip.sock"
        self.log_path = log_path
        self.csv_preview = csv_preview
        self.xlsx_preview = xlsx_preview
        self.model3d_variant = model3d_variant
        self.binary_diff_preview = binary_diff_preview
        self.diff_preview = diff_preview
        self.image_diff_preview = image_diff_preview
        self.pdf_preview = pdf_preview
        self.reload_config_error = reload_config_error
        self.midi_export = midi_export
        self.runtime = runtime
        self.midi_export_result: dict[str, Any] | None = None
        self.xlsx_path = log_path.parent / "acceptance-multisheet.xlsx"
        if xlsx_preview:
            self._write_xlsx_fixture()
        self.model3d_path = log_path.parent / (
            f"acceptance-{model3d_variant}.stl" if model3d_variant else ""
        )
        if model3d_variant:
            self._write_model3d_fixture()
        self.binary_left_path = log_path.parent / "acceptance-left.bin"
        self.binary_right_path = log_path.parent / "acceptance-right.bin"
        self.image_left_path = log_path.parent / "acceptance-image-left.png"
        self.image_right_path = log_path.parent / "acceptance-image-right.png"
        self.pdf_path = log_path.parent / "acceptance-mirror.pdf"
        if pdf_preview:
            pdf_fixture(log_path.parent, 2, self.pdf_path.name)
        if image_diff_preview:
            self._write_png_fixture(self.image_left_path, (42, 128, 220), (220, 80, 52))
            self._write_png_fixture(self.image_right_path, (220, 80, 52), (42, 128, 220))
        if binary_diff_preview:
            self.binary_left_path.write_bytes(b"little-screen-clip\x00\x01\x02\x03\x04\noriginal\n")
            self.binary_right_path.write_bytes(b"little-screen-clip\x00\x01\x09\x03\x04\nupdated\n")
        self.rolling_audio_path = log_path.parent / "fake-rolling-audio.wav"
        with wave.open(str(self.rolling_audio_path), "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(48_000)
            wav.writeframes(bytes(48_000 * 2))
        self.events: list[dict[str, Any]] = []
        self.stop_event = threading.Event()
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(self.socket_path))
        os.chmod(self.socket_path, 0o600)
        self.listener.listen(8)
        self.listener.settimeout(0.2)
        self.thread = threading.Thread(target=self._serve, name="fake-text-ipc", daemon=True)
        self.thread.start()

    @staticmethod
    def _write_png_fixture(path: Path, first: tuple[int, int, int], second: tuple[int, int, int]) -> None:
        width, height = 96, 64

        def chunk(kind: bytes, data: bytes) -> bytes:
            body = kind + data
            return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

        scanlines = bytearray()
        for y in range(height):
            scanlines.append(0)
            color = first if y < height // 2 else second
            for x in range(width):
                pixel = color if x < width // 2 else tuple(255 - value for value in color)
                scanlines.extend((*pixel, 255))
        png = (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(scanlines), level=9))
            + chunk(b"IEND", b"")
        )
        path.write_bytes(png)

    def _write_xlsx_fixture(self) -> None:
        content_types = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            '<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            '</Types>'
        )
        package_rels = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
            '</Relationships>'
        )
        workbook = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
            'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
            '<sheets><sheet name="Quarterly" sheetId="1" r:id="rId1"/>'
            '<sheet name="Inventory" sheetId="2" r:id="rId2"/></sheets></workbook>'
        )
        workbook_rels = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
            '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>'
            '</Relationships>'
        )
        quarter = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
            '<row r="1"><c r="A1" t="inlineStr"><is><t>Quarter</t></is></c>'
            '<c r="B1" t="inlineStr"><is><t>Revenue</t></is></c></row>'
            '<row r="2"><c r="A2" t="inlineStr"><is><t>Q1</t></is></c><c r="B2"><v>12500</v></c></row>'
            '<row r="3"><c r="A3" t="inlineStr"><is><t>Q2</t></is></c><c r="B3"><v>14750</v></c></row>'
            '</sheetData></worksheet>'
        )
        inventory = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
            '<row r="1"><c r="A1" t="inlineStr"><is><t>Item</t></is></c>'
            '<c r="B1" t="inlineStr"><is><t>Units</t></is></c></row>'
            '<row r="2"><c r="A2" t="inlineStr"><is><t>Widgets</t></is></c><c r="B2"><v>34</v></c></row>'
            '<row r="3"><c r="A3" t="inlineStr"><is><t>Gadgets</t></is></c><c r="B3"><v>18</v></c></row>'
            '</sheetData></worksheet>'
        )
        with zipfile.ZipFile(self.xlsx_path, "w", compression=zipfile.ZIP_DEFLATED) as workbook_zip:
            workbook_zip.writestr("[Content_Types].xml", content_types)
            workbook_zip.writestr("_rels/.rels", package_rels)
            workbook_zip.writestr("xl/workbook.xml", workbook)
            workbook_zip.writestr("xl/_rels/workbook.xml.rels", workbook_rels)
            workbook_zip.writestr("xl/worksheets/sheet1.xml", quarter)
            workbook_zip.writestr("xl/worksheets/sheet2.xml", inventory)

    def _write_model3d_fixture(self) -> None:
        if self.model3d_variant == "error":
            self.model3d_path.write_text(
                "solid invalid_fixture\nthis is not valid STL facet data\nendsolid invalid_fixture\n",
                encoding="ascii",
            )
            return
        facet_count = 1 if self.model3d_variant == "valid" else 100_000
        with self.model3d_path.open("w", encoding="ascii") as stl:
            stl.write("solid acceptance_mesh\n")
            for index in range(facet_count):
                x = index % 200
                y = index // 200
                stl.write(
                    "facet normal 0 0 1\n outer loop\n"
                    f"vertex {x} {y} 0\nvertex {x + 1} {y} 0\nvertex {x} {y + 1} 0\n"
                    "endloop\nendfacet\n"
                )
            stl.write("endsolid acceptance_mesh\n")

    def _item(self) -> dict[str, Any]:
        if self.pdf_preview:
            return {
                "id": self.PDF_ITEM_ID,
                "created_at": int(time.time()),
                "payload": {"Text": str(self.pdf_path)},
                "title": self.pdf_path.name,
                "subtitle": "PDF • 2 pages",
                "details": ["PDF", "2 pages"],
                "handler_tag": "PDF",
            }
        if self.image_diff_preview:
            return {
                "id": self.DIFF_LEFT_ITEM_ID,
                "created_at": int(time.time()),
                "payload": {"Image": {
                    "path": str(self.image_left_path), "width": 96, "height": 64,
                    "mime": "image/png", "thumbnail": None,
                }},
                "title": "acceptance-image-left.png",
                "subtitle": "Image • Diff fixture",
                "details": ["Image", "96×64"],
                "handler_tag": "IMAGE (FILE)",
            }
        if self.diff_preview:
            return {
                "id": self.DIFF_LEFT_ITEM_ID,
                "created_at": int(time.time()),
                "payload": {"Text": "left item: common line\nleft-only value\n"},
                "title": "acceptance-left.txt",
                "subtitle": "Text • Diff fixture",
                "details": ["Text", "Left"],
                "handler_tag": "TEXT",
            }
        if self.binary_diff_preview:
            text = f"{self.binary_left_path}\n{self.binary_right_path}"
            title = "acceptance-binary-pair.txt"
        elif self.model3d_variant:
            text = str(self.model3d_path)
            title = self.model3d_path.name
        elif self.xlsx_preview:
            text = str(self.xlsx_path)
            title = self.xlsx_path.name
        else:
            text = self.CSV if self.csv_preview else self.TEXT
            title = "acceptance-table.csv" if self.csv_preview else "acceptance-markdown.md"
        payload_kind = (
            "Text" if self.csv_preview or self.xlsx_preview or self.model3d_variant else "Markdown"
        )
        if self.binary_diff_preview:
            payload_kind = "Text"
        return {
            "id": self.ITEM_ID,
            "created_at": int(time.time()),
            "payload": {payload_kind: text},
            "title": title,
            "subtitle": (
                "Binary • 2 fixtures" if self.binary_diff_preview
                else "STL • acceptance mesh" if self.model3d_variant
                else "Excel • 2 sheets" if self.xlsx_preview
                else "CSV • 5 rows × 4 columns" if self.csv_preview
                else "Markdown • 15 lines"
            ),
            "details": (["Binary", "2 files"] if self.binary_diff_preview
                        else ["3D", "STL"] if self.model3d_variant
                        else ["XLSX", "2 sheets"] if self.xlsx_preview
                        else ["CSV", "Text"] if self.csv_preview
                        else ["Markdown", "Text"]),
            "handler_tag": "TEXT",
        }

    def _state(self) -> dict[str, Any]:
        return {
            "undo": None, "left_hand": self._item(),
            "right_hand": ({
                "id": self.DIFF_RIGHT_ITEM_ID,
                "created_at": int(time.time()),
                "payload": {"Text": "right item: common line\nright-only value\n"},
                "title": "acceptance-right.txt",
                "subtitle": "Text • Diff fixture",
                "details": ["Text", "Right"],
                "handler_tag": "TEXT",
            } if self.diff_preview else ({
                "id": self.DIFF_RIGHT_ITEM_ID,
                "created_at": int(time.time()),
                "payload": {"Image": {
                    "path": str(self.image_right_path), "width": 96, "height": 64,
                    "mime": "image/png", "thumbnail": None,
                }},
                "title": "acceptance-image-right.png",
                "subtitle": "Image • Diff fixture",
                "details": ["Image", "96×64"],
                "handler_tag": "IMAGE (FILE)",
            } if self.image_diff_preview else None)), "left_prior1": None,
            "left_prior2": None, "left_prior3": None, "right_prior2": None,
            "right_prior1": None, "cursor": "LeftHand",
            "selected": ["LeftHand"],
        }

    def _response(self, request: Any) -> tuple[Any, str]:
        if self.midi_export and isinstance(request, dict) and "PushFile" in request:
            push = request["PushFile"]
            path = Path(push["path"])
            expected_dir = self.runtime / "little-screen-clip" / "midi_slices"
            inside_private_runtime = path.parent == expected_dir
            midi_bytes = path.read_bytes() if inside_private_runtime and path.is_file() else b""
            declared_tracks = struct.unpack_from(">H", midi_bytes, 10)[0] if len(midi_bytes) >= 14 else 0
            track_chunks = midi_bytes.count(b"MTrk")
            self.midi_export_result = {
                "path": str(path),
                "hand": push["hand"],
                "inside_private_runtime": inside_private_runtime,
                "bytes": len(midi_bytes),
                "midi_header": midi_bytes.startswith(b"MThd\x00\x00\x00\x06"),
                "declared_tracks": declared_tracks,
                "track_chunks": track_chunks,
                "contains_note_on": any(bytes([status, 60]) in midi_bytes for status in range(0x90, 0xA0)),
            }
            if (push["hand"] == "Left" and inside_private_runtime
                    and self.midi_export_result["midi_header"]
                    and declared_tracks > 0 and declared_tracks == track_chunks
                    and self.midi_export_result["contains_note_on"]):
                return {"Success": {"message": "Accepted private MIDI export"}}, "midi_export"
            return {"Error": {"error": "Invalid private MIDI export"}}, "midi_export_rejected"
        if request == "ReloadConfig" and self.reload_config_error:
            return {"Error": {"error": "acceptance reload rejected"}}, "reload_config_error"
        if request == "GetState":
            state = self._state()
            if self.pdf_preview:
                # Force one normal state-poller hydration so the capture can
                # distinguish startup item loading from any later refresh.
                state["left_hand"] = None
            return {"State": state}, "state"
        if request == "GetStateSummary":
            cards = [{
                "slot": "LeftHand",
                "item_id": self.DIFF_LEFT_ITEM_ID if self.diff_preview or self.image_diff_preview else self.PDF_ITEM_ID if self.pdf_preview else self.ITEM_ID,
                "payload_kind": "Image" if self.image_diff_preview else "Text" if self.csv_preview or self.xlsx_preview or self.model3d_variant or self.diff_preview or self.pdf_preview else "Markdown",
                "preview_phase": "NotApplicable" if self.pdf_preview else "Ready",
            }]
            if self.diff_preview or self.image_diff_preview:
                cards.append({
                    "slot": "RightHand",
                    "item_id": self.DIFF_RIGHT_ITEM_ID,
                    "payload_kind": "Image" if self.image_diff_preview else "Text",
                    "preview_phase": "NotApplicable",
                })
            return {"StateSummary": {"cards": cards, "cursor": "LeftHand", "selected": ["LeftHand"]}}, "summary"
        if request in ({"GetSlotItem": "LeftHand"}, {"GetSlotItem": "RightHand"}):
            if request == {"GetSlotItem": "RightHand"} and not (self.diff_preview or self.image_diff_preview):
                return {"Error": {"error": "acceptance Right Hand fixture is unavailable"}}, "unsupported"
            if request == {"GetSlotItem": "RightHand"}:
                return {"SlotItem": self._state()["right_hand"]}, "slot_item"
            return {"SlotItem": self._item()}, "slot_item"
        if request == "GetRollingAudio":
            return {"RollingAudio": {
                "path": str(self.rolling_audio_path), "duration_secs": 1.0,
                "sample_rate": 48_000, "channels": 1,
            }}, "rolling_audio"
        if request == "GetAutoActions":
            return {"AutoActions": {}}, "auto_actions"
        return {"Error": {"error": f"unsupported acceptance request: {request!r}"}}, "unsupported"

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
                    request = json.loads(connection.makefile("rb").readline(1_048_577))
                    response, kind = self._response(request)
                    self.events.append({"kind": kind, "request": request})
                    connection.sendall((json.dumps(response, separators=(",", ":")) + "\n").encode())
                except (OSError, ValueError, TypeError) as exc:
                    self.events.append({"kind": "protocol_error", "error": str(exc)})

    def close(self) -> dict[str, Any]:
        self.stop_event.set()
        self.listener.close()
        self.thread.join(timeout=1)
        self.log_path.write_text(json.dumps(self.events, indent=2) + "\n", encoding="utf-8")
        slot_item_events = [
            event for event in self.events
            if event["kind"] == "slot_item" and event["request"] == {"GetSlotItem": "LeftHand"}
        ]
        item_requested = bool(slot_item_events)
        result = {
            "log": str(self.log_path),
            "rolling_audio_fixture": str(self.rolling_audio_path),
            "slot_item_requested": item_requested,
            "pass": item_requested,
            "events": len(self.events),
        }
        if self.midi_export:
            result["midi_export"] = self.midi_export_result
            result["pass"] = (
                sum(event["kind"] in ("midi_export", "midi_export_rejected") for event in self.events) == 1
                and any(event["kind"] == "midi_export" for event in self.events)
                and self.midi_export_result is not None
                and self.midi_export_result["inside_private_runtime"]
            )
        if self.pdf_preview:
            result.update({
                "pdf_fixture": str(self.pdf_path),
                "initial_slot_item_hydrations": len(slot_item_events),
                "additional_slot_item_hydrations_after_initial": max(0, len(slot_item_events) - 1),
            })
        return result

    def pdf_slot_item_hydration_count(self) -> int:
        return sum(
            event["kind"] == "slot_item"
            and event["request"] == {"GetSlotItem": "LeftHand"}
            for event in self.events
        )

    def wait_for_pdf_slot_item_hydrations(self, count: int, timeout: float = 5.0) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.pdf_slot_item_hydration_count() >= count:
                return True
            time.sleep(0.05)
        return self.pdf_slot_item_hydration_count() >= count

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
    expected_size: tuple[int, int] | None = None,
) -> None:
    expected = list(expected_size or (
        math.floor(logical_size[0] * 0.92),
        math.floor(logical_size[1] * 0.92),
    ))
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
        "first_observed_window_size_matches_expected": first_observation["window_size"] == expected,
        "final_window_size_matches_expected": final_observation["window_size"] == expected,
        "first_and_final_window_size_stable": first_observation["window_size"] is not None
        and first_observation["window_size"] == final_observation["window_size"],
        "initial_wayland_min_max_hints_match_expected_size": hints["min_size"] == expected and hints["max_size"] == expected,
        "initial_window_opened_event_size_matches_expected": initial_observation.get("window_size") == expected,
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
        "size_source": "fixed fallback" if expected_size is not None else "92 percent of focused output",
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
    repo_scripts = Path(__file__).resolve().parent.parent / "scripts"
    (scratch / "scripts").symlink_to(repo_scripts, target_is_directory=True)
    config = f'[workbench]\nappearance = "{args.appearance}"\n'
    (scratch / "config.toml").write_text(config, encoding="utf-8")
    user_config = config_home / "little-screen-clip" / "config.toml"
    user_config.parent.mkdir(mode=0o700)
    user_config.write_text(config, encoding="utf-8")
    if args.fake_node_machine:
        recipes_dir = user_config.parent / "recipes"
        recipes_dir.mkdir(mode=0o700)
        (recipes_dir / "pad_and_caption.yaml").write_text(
            """name: Pad and Caption
description: Deterministic Node Machine script-property acceptance fixture
input_type: any
tags: [acceptance]
graph:
  nodes:
    - id: 00000000-0000-4000-8000-000000000001
      kind: PythonScript
      title: Python Transform
      pos: [20.0, 20.0]
      props:
        script_file: transform.py
      inputs:
        - name: In
          port_type: Text
      outputs:
        - name: Out
          port_type: Text
  connections: []
  pan: [0.0, 0.0]
  zoom: 1.0
""",
            encoding="utf-8",
        )
        (recipes_dir / "transform.py").write_text(
            "def transform(value):\n    return value\n", encoding="utf-8"
        )
    niri_config = output / "niri.kdl"
    niri_config.write_text(
        "".join((
            "hotkey-overlay { skip-at-startup; }\n",
            "" if args.allow_server_decorations else "prefer-no-csd\n",
            f'output "winit" {{ scale {args.scale}; }}\n',
            'window-rule { match app-id="little-clip-workbench"; open-floating true; }\n',
        )),
        encoding="utf-8",
    )
    clean_env = os.environ.copy()
    for key in ("DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "NIRI_SOCKET", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"):
        clean_env.pop(key, None)
    clean_env.update({"LIBGL_ALWAYS_SOFTWARE": "1", "XDG_RUNTIME_DIR": str(runtime), "XDG_CONFIG_HOME": str(config_home)})
    niri_bin, weston_bin = shutil.which("niri"), shutil.which("weston")
    if not niri_bin or not weston_bin:
        raise RuntimeError("both niri and weston must be installed")
    startup_query_failure_marker: Path | None = None
    if args.fail_startup_output_query:
        private_bin = output / "private-bin"
        private_bin.mkdir(mode=0o700)
        startup_query_failure_marker = output / "startup-output-query-failure.txt"
        niri_shim = private_bin / "niri"
        niri_shim.write_text(
            "#!/usr/bin/env python3\n"
            "import os, sys\n"
            f"real_niri = {str(Path(niri_bin).resolve())!r}\n"
            f"failure_marker = {str(startup_query_failure_marker)!r}\n"
            "if sys.argv[1:] == ['msg', '--json', 'focused-output']:\n"
            "    with open(failure_marker, 'a', encoding='utf-8') as marker:\n"
            "        marker.write('focused-output query failed as requested\\n')\n"
            "    print('injected focused-output query failure', file=sys.stderr)\n"
            "    raise SystemExit(1)\n"
            "os.execv(real_niri, [real_niri, *sys.argv[1:]])\n",
            encoding="utf-8",
        )
        niri_shim.chmod(0o700)
        clean_env["PATH"] = f"{private_bin}{os.pathsep}{clean_env.get('PATH', '')}"
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
        "startup_query_failure_marker": startup_query_failure_marker,
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
    if args.binary_diff_view:
        command.append("--binary")
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
    fake_text_daemon: FakeTextPreviewDaemon | None = None,
) -> None:
    niri_bin, scratch = config["niri_bin"], config["scratch"]
    logical_size = output_logical_size(outputs)
    if logical_size is None:
        raise RuntimeError("Niri outputs response did not include logical width and height")
    report["outputs"] = outputs
    report["actual_logical_size"] = list(logical_size)
    app_env = niri_env | {"WAYLAND_DEBUG": "client"}
    pointer_helper: Path | None = None
    if (args.click is not None or args.click_after_key_action is not None
            or args.move is not None or args.drag is not None or args.scroll is not None):
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
    if args.fake_pdf_preview and fake_text_daemon is not None:
        if not fake_text_daemon.wait_for_pdf_slot_item_hydrations(1):
            raise RuntimeError("fake PDF slot item was not initially hydrated by the workbench")
        time.sleep(0.3)
    if args.fake_node_machine:
        run_checked(["wtype", "-k", "F1"], env=niri_env)
        report["node_machine_route"] = "F1 sent through private Wayland keyboard input"
        time.sleep(0.5)
    report["first_observed_windows"] = first_windows
    report["screenshots"]["first_observed"] = screenshot(
        niri_bin, niri_env, output / "first-observed.png"
    )
    if args.fake_pdf_preview:
        report["screenshots"]["before_pdf_open"] = screenshot(
            niri_bin, niri_env, output / "before-pdf-open.png"
        )
    if args.image_tool:
        run_checked(["wtype", "-k", IMAGE_TOOL_KEYS[args.image_tool]], env=niri_env)
        if args.image_action:
            time.sleep(0.1)
            run_checked(["wtype", "-k", IMAGE_ACTION_KEYS[args.image_action]], env=niri_env)
        if args.drag is not None:
            time.sleep(0.25)
    if args.post_tool_delay_ms:
        time.sleep(args.post_tool_delay_ms / 1000)
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
            time.sleep(args.click_delay_ms / 1000)
            if args.scroll_after_click == click_index:
                send_private_scroll()
        if args.scroll is not None and args.scroll_after_click is None:
            send_private_scroll()
    elif args.scroll is not None:
        send_private_scroll()
    if args.drag is not None and pointer_helper is not None:
        report["pointer_input"]["drags"] = []
        for start_x, start_y, end_x, end_y in args.drag:
            drag_result = run_checked(
                [str(pointer_helper), "drag", str(logical_size[0]), str(logical_size[1]),
                 str(start_x), str(start_y), str(end_x), str(end_y)],
                env=niri_env,
            )
            report["pointer_input"]["drags"].append({
                "start": [start_x, start_y], "end": [end_x, end_y], "button": "left",
                "result": drag_result.strip(),
            })
            time.sleep(0.5)
    if args.move is not None and pointer_helper is not None:
        x, y = args.move
        hover_log = output / "private-pointer-hover.log"
        hover_process = children.start(
            "pointer_hover",
            [str(pointer_helper), "move", str(logical_size[0]), str(logical_size[1]), str(x), str(y)],
            env=niri_env, cwd=scratch, log_path=hover_log,
        )
        ready_line = f"moved private Niri pointer to {x},{y}"
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline:
            if hover_log.is_file() and ready_line in hover_log.read_text(encoding="utf-8", errors="replace"):
                break
            if hover_process.poll() is not None:
                raise RuntimeError(f"private hover pointer exited before reaching ({x}, {y})")
            time.sleep(0.05)
        else:
            raise RuntimeError(f"timed out waiting for private hover pointer at ({x}, {y})")
        report["pointer_input"]["move"] = {"x": x, "y": y, "button": "none", "held_until_cleanup": True}
    if args.verify_viewer_return and args.type_text:
        run_checked(["wtype", args.type_text], env=niri_env)
        report.setdefault("pointer_input", {})["typed_text"] = args.type_text
    if args.key_action:
        report.setdefault("pointer_input", {})
        report["pointer_input"]["key_actions"] = []
        for action in args.key_action:
            run_checked(["wtype", *KEY_ACTIONS[action]], env=niri_env)
            report["pointer_input"]["key_actions"].append(action)
            time.sleep(0.25)
    if args.type_text and not args.verify_viewer_return:
        run_checked(["wtype", args.type_text], env=niri_env)
        report.setdefault("pointer_input", {})["typed_text"] = args.type_text
    if args.click_after_key_action or args.capture_after_key_actions:
        if args.post_key_delay_ms:
            time.sleep(args.post_key_delay_ms / 1000)
        if args.capture_after_key_actions:
            report["screenshots"]["after_key_actions"] = screenshot(
                niri_bin, niri_env, output / "after-key-actions.png"
            )
        if args.click_after_key_action:
            assert pointer_helper is not None
            report.setdefault("pointer_input", {})["clicks_after_key_actions"] = []
            for x, y in args.click_after_key_action:
                run_checked(
                    [str(pointer_helper), "click", str(logical_size[0]), str(logical_size[1]), str(x), str(y)],
                    env=niri_env,
                )
                report["pointer_input"]["clicks_after_key_actions"].append(
                    {"x": x, "y": y, "button": "left"}
                )
                time.sleep(args.click_delay_ms / 1000)
    if args.capture_after_actions:
        report["screenshots"]["after_actions"] = screenshot(
            niri_bin, niri_env, output / "after-actions.png"
        )
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
    expected_size = FALLBACK_WINDOW_SIZE if args.fail_startup_output_query else None
    inspect_geometry(
        first_windows, final_windows, layout_events, logical_size, hints, report,
        expected_size=expected_size,
    )
    if args.fail_startup_output_query:
        marker = config["startup_query_failure_marker"]
        marker_seen = marker.is_file() and "focused-output query failed as requested" in marker.read_text(
            encoding="utf-8", errors="replace"
        )
        observed_size = layout_size(first_windows[0]) if first_windows else None
        fallback_pass = marker_seen and observed_size == FALLBACK_WINDOW_SIZE
        report["startup_fallback"] = {
            "forced_focused_output_query_failure": marker_seen,
            "expected_fallback_size": list(FALLBACK_WINDOW_SIZE),
            "observed_window_size": list(observed_size) if observed_size else None,
            "fixed_size_hints": hints,
            "visible_workbench_capture": report["screenshots"]["first_observed"]["path"],
            "pass": fallback_pass,
        }
        if not fallback_pass:
            report["failures"].append(
                "startup output-query failure did not open the workbench at the fixed fallback size"
            )

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
    if args.verify_viewer_return:
        run_checked(["wtype", "-k", "Escape"], env=niri_env)
        time.sleep(max(args.settle_ms / 1000, 0.25))
        returned_windows = own_windows(niri_bin, niri_env, {app.pid})
        app_alive = app.poll() is None
        focused = (
            len(returned_windows) == 1
            and returned_windows[0].get("pid") == app.pid
            and returned_windows[0].get("is_focused") is True
        )
        report["screenshots"]["after-viewer-close"] = screenshot(
            niri_bin, niri_env, output / "after-viewer-close.png"
        )
        report["viewer_return_check"] = {
            "escape_sent": True,
            "workbench_remained_open": app_alive,
            "remaining_workbench_windows": returned_windows,
            "existing_popup_remained_focused": focused,
            "pass": app_alive and focused,
        }
        if not report["viewer_return_check"]["pass"]:
            report["failures"].append(
                "Escape did not return from the viewer to the existing focused workbench popup"
            )
        if args.fake_pdf_preview and fake_text_daemon is not None:
            time.sleep(1.5)
            hydration_count = fake_text_daemon.pdf_slot_item_hydration_count()
            report["pdf_dirty_close_mirror_refresh"] = {
                "initial_slot_item_hydrations": min(hydration_count, 1),
                "additional_daemon_slot_item_hydrations_after_viewer_close": max(0, hydration_count - 1),
                "local_mirror_redraw_is_verified_by": "closing_dirty_pdf_requests_and_consumes_dirty_mirror in clip-workbench tests",
                "daemon_rehydration_expected": False,
                "pass": report["viewer_return_check"]["pass"],
            }
            if report["pdf_dirty_close_mirror_refresh"]["pass"]:
                report["screenshots"]["after_dirty_pdf_close"] = screenshot(
                    niri_bin, niri_env, output / "after-dirty-pdf-close.png"
                )
            if hydration_count == 0:
                report["failures"].append(
                    "fake PDF clipboard item was never hydrated before opening the editor"
                )


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path, help="new directory for report, logs, and PNG captures")
    parser.add_argument("--width", type=int, default=1280, help="Weston logical width (default: 1280)")
    parser.add_argument("--height", type=int, default=800, help="Weston logical height (default: 800)")
    parser.add_argument("--scale", type=float, default=1.0, help="nested Niri output scale (default: 1)")
    parser.add_argument("--appearance", choices=("light", "dark"), required=True)
    parser.add_argument(
        "--allow-server-decorations", action="store_true",
        help="omit Niri's prefer-no-csd setting to characterize decorated popup geometry",
    )
    parser.add_argument(
        "--fail-startup-output-query", action="store_true",
        help="privately fail the workbench's focused-output query to verify its fixed-size startup fallback",
    )
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
    parser.add_argument(
        "--post-tool-delay-ms", type=int, default=0,
        help="wait after selecting an image tool and before pointer actions (default: 0)",
    )
    parser.add_argument(
        "--key-action", choices=tuple(KEY_ACTIONS), action="append",
        help="send an ordered keyboard shortcut after pointer actions; repeat to send multiple shortcuts",
    )
    parser.add_argument(
        "--type-text",
        help="type literal text into the focused control through private Wayland",
    )
    parser.add_argument("--image", type=Path, help="optional existing image fixture")
    parser.add_argument(
        "--click", nargs=2, type=int, metavar=("X", "Y"), action="append",
        help="send a left click at logical screen coordinates through private Niri; repeat to perform ordered UI interactions",
    )
    parser.add_argument(
        "--click-delay-ms", type=int, default=500,
        help="wait after each private pointer click before continuing (default: 500)",
    )
    parser.add_argument(
        "--capture-after-actions", action="store_true",
        help="capture immediately after injected actions and before the settling delay",
    )
    parser.add_argument(
        "--capture-after-key-actions", action="store_true",
        help="capture after keyboard actions and before any --click-after-key-action clicks",
    )
    parser.add_argument(
        "--click-after-key-action", nargs=2, type=int, metavar=("X", "Y"), action="append",
        help="send a private left click after keyboard actions; repeat for ordered clicks",
    )
    parser.add_argument(
        "--post-key-delay-ms", type=int, default=0,
        help="wait after keyboard actions before capture/clicks (default: 0)",
    )
    parser.add_argument(
        "--move", nargs=2, type=int, metavar=("X", "Y"),
        help="move and hold a virtual pointer at logical screen coordinates in private Niri without clicking",
    )
    parser.add_argument(
        "--drag", nargs=4, type=int,
        metavar=("START_X", "START_Y", "END_X", "END_Y"), action="append",
        help="send one left-button drag through private Niri; repeat for ordered gestures",
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
        "--verify-midi-copy-pointer", action="store_true",
        help="accept one private pointer click on Copy MIDI through a fake Left Hand IPC receiver",
    )
    parser.add_argument(
        "--fake-audio-transition",
        action="store_true",
        help="serve a private pending-to-ready audio slot over IPC while the workbench stays open",
    )
    parser.add_argument(
        "--fake-text-preview", action="store_true",
        help="serve a private Markdown clipboard item through IPC for isolated viewer acceptance",
    )
    parser.add_argument(
        "--fake-pdf-preview", action="store_true",
        help="serve a private PDF path item and deterministic PDF fixture through IPC",
    )
    parser.add_argument(
        "--fake-csv-preview", action="store_true",
        help="serve private CSV clipboard text through IPC for spreadsheet viewer acceptance",
    )
    parser.add_argument(
        "--fake-multisheet-xlsx", action="store_true",
        help="serve a private text item pointing to a generated two-sheet XLSX for acceptance",
    )
    parser.add_argument(
        "--fake-model3d", choices=("valid", "error", "loading"),
        help="serve a private STL fixture for 3D viewer acceptance",
    )
    parser.add_argument(
        "--fake-node-machine", action="store_true",
        help="load a deterministic script-node recipe from this run's private config and open Node Machine",
    )
    parser.add_argument("--fake-binary-diff", action="store_true",
                        help="serve private byte fixtures for binary-diff viewer acceptance")
    parser.add_argument("--fake-diff-preview", action="store_true",
                        help="serve two private text items for regular Diff route acceptance")
    parser.add_argument("--fake-image-diff-preview", action="store_true",
                        help="serve two deterministic private PNG items for image Diff route acceptance")
    parser.add_argument("--fake-reload-error", action="store_true",
                        help="return a deterministic error to ReloadConfig over private IPC")
    parser.add_argument("--binary-diff-view", action="store_true",
                        help="open binary diff directly (requires --fake-binary-diff)")
    parser.add_argument("--verify-return", action="store_true", help="send Escape and verify the popup closes while private Niri remains responsive")
    parser.add_argument("--verify-viewer-return", action="store_true",
                        help="send Escape and verify a directly opened viewer returns to the existing popup")
    parser.add_argument("--binary", type=Path, help="workbench binary (default: target/debug/clip-workbench)")
    parser.add_argument("--settle-ms", type=int, default=500, help="settling delay before second capture (default: 500)")
    return parser


def validate_cli(args: argparse.Namespace, repo: Path) -> Path:
    if (
        min(args.width, args.height, args.scale) <= 0
        or not math.isfinite(args.scale)
        or args.settle_ms < 0
        or args.post_tool_delay_ms < 0
        or args.click_delay_ms < 0
        or args.post_key_delay_ms < 0
    ):
        raise SystemExit("width, height, and scale must be positive; delay values cannot be negative")
    for fixture in (args.image, args.audio, args.pdf, args.midi):
        if fixture is not None and not fixture.is_file():
            raise SystemExit(f"fixture does not exist: {fixture}")
    if args.verify_midi_copy_pointer and (
        args.midi is None or not args.click or len(args.click) != 1
        or args.key_action or args.click_after_key_action or args.type_text
        or args.verify_return or args.verify_viewer_return
        or args.image_tool or args.image_action or args.drag or args.scroll
        or any((args.fake_audio_transition, args.fake_text_preview, args.fake_pdf_preview,
                args.fake_csv_preview, args.fake_multisheet_xlsx, args.fake_model3d,
                args.fake_binary_diff, args.fake_diff_preview, args.fake_image_diff_preview,
                args.fake_node_machine, args.fake_reload_error))
    ):
        raise SystemExit(
            "--verify-midi-copy-pointer requires --midi and one --click, with no keyboard or return actions"
        )
    if args.fake_audio_transition and any((args.image, args.audio, args.pdf, args.midi)):
        raise SystemExit("--fake-audio-transition cannot be combined with file-entry fixtures")
    if args.fake_text_preview and (
        args.fake_audio_transition or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-text-preview cannot be combined with other fixture routes")
    if args.fake_pdf_preview and (
        args.fake_text_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
        or args.fake_model3d or args.fake_binary_diff or args.fake_diff_preview
        or args.fake_image_diff_preview or args.fake_audio_transition
        or args.fake_node_machine or args.fake_reload_error
        or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-pdf-preview cannot be combined with other fixture routes")
    if args.fake_csv_preview and (
        args.fake_text_preview or args.fake_audio_transition
        or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-csv-preview cannot be combined with other fixture routes")
    if args.fake_multisheet_xlsx and (
        args.fake_text_preview or args.fake_csv_preview or args.fake_audio_transition
        or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-multisheet-xlsx cannot be combined with other fixture routes")
    if args.fake_model3d and (
        args.fake_text_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
        or args.fake_audio_transition or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-model3d cannot be combined with other fixture routes")
    if args.fake_binary_diff and (
        args.fake_text_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
        or args.fake_model3d or args.fake_audio_transition
        or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-binary-diff cannot be combined with other fixture routes")
    if args.fake_diff_preview and (
        args.fake_text_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
        or args.fake_model3d or args.fake_binary_diff or args.fake_audio_transition
        or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-diff-preview cannot be combined with other fixture routes")
    if args.fake_image_diff_preview and (
        args.fake_text_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
        or args.fake_model3d or args.fake_binary_diff or args.fake_diff_preview
        or args.fake_audio_transition or args.fake_node_machine
        or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-image-diff-preview cannot be combined with other fixture routes")
    fake_fixture_flags = (
        args.fake_audio_transition, args.fake_text_preview, args.fake_pdf_preview, args.fake_csv_preview,
        args.fake_multisheet_xlsx, bool(args.fake_model3d), args.fake_binary_diff,
        args.fake_diff_preview, args.fake_image_diff_preview, args.fake_node_machine,
    )
    if args.fake_reload_error and (
        any(fake_fixture_flags) or any((args.image, args.audio, args.pdf, args.midi))
        or not args.key_action or any(action != "f6" for action in args.key_action)
    ):
        raise SystemExit("--fake-reload-error requires only --key-action f6 and no other fixture route")
    if args.fake_node_machine and (
        args.fake_text_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
        or args.fake_model3d or args.fake_audio_transition or args.fake_binary_diff
        or args.fake_diff_preview or args.fake_image_diff_preview
        or any((args.image, args.audio, args.pdf, args.midi))
    ):
        raise SystemExit("--fake-node-machine cannot be combined with other fixture routes")
    if args.binary_diff_view and not args.fake_binary_diff:
        raise SystemExit("--binary-diff-view requires --fake-binary-diff")
    direct_fixture_count = sum(
        fixture is not None for fixture in (args.image, args.audio, args.pdf, args.midi)
    )
    direct_viewer_route = direct_fixture_count == 1 and not any(fake_fixture_flags)
    palette_diff_route = (
        (args.fake_diff_preview or args.fake_image_diff_preview) and args.type_text == "diff"
        and args.key_action and "enter" in args.key_action
    )
    if args.verify_viewer_return and not (
        direct_viewer_route or palette_diff_route or args.fake_pdf_preview
    ):
        raise SystemExit(
            "--verify-viewer-return requires one direct file fixture, fake Diff route, or fake PDF route"
        )
    if args.verify_viewer_return and args.verify_return:
        raise SystemExit("--verify-viewer-return cannot be combined with --verify-return")
    if args.fake_pdf_preview and args.verify_viewer_return and (
        not args.click or "r" not in (args.key_action or [])
    ):
        raise SystemExit(
            "--fake-pdf-preview with --verify-viewer-return requires a Palette click and --key-action r"
        )
    if args.image_tool and not args.image:
        raise SystemExit("--image-tool requires --image")
    if args.click is not None and any(
        not (0 <= x < args.width) or not (0 <= y < args.height) for x, y in args.click
    ):
        raise SystemExit("--click coordinates must be inside the requested logical output")
    if args.click_after_key_action is not None and any(
        not (0 <= x < args.width) or not (0 <= y < args.height)
        for x, y in args.click_after_key_action
    ):
        raise SystemExit("--click-after-key-action coordinates must be inside the requested logical output")
    if args.click_after_key_action and not args.key_action:
        raise SystemExit("--click-after-key-action requires --key-action")
    if args.capture_after_key_actions and not args.key_action:
        raise SystemExit("--capture-after-key-actions requires --key-action")
    if args.move is not None and (
        not (0 <= args.move[0] < args.width) or not (0 <= args.move[1] < args.height)
    ):
        raise SystemExit("--move coordinates must be inside the requested logical output")
    if args.drag is not None and any(
        not (0 <= x < args.width) or not (0 <= y < args.height)
        for start_x, start_y, end_x, end_y in args.drag
        for x, y in ((start_x, start_y), (end_x, end_y))
    ):
        raise SystemExit("--drag coordinates must be inside the requested logical output")
    if args.scroll is not None and (
        not (0 <= args.scroll[0] < args.width) or not (0 <= args.scroll[1] < args.height)
        or args.scroll[2] == 0 or abs(args.scroll[2]) > 1000
    ):
        raise SystemExit("--scroll coordinates must be inside the requested output and DELTA must be 1..1000 in magnitude")
    if args.scroll is not None and args.click is None and not args.fake_node_machine:
        raise SystemExit("--scroll requires --click so the pointer action remains an explicit post-selection interaction")
    if args.scroll_after_click is not None and args.scroll is None:
        raise SystemExit("--scroll-after-click requires --scroll")
    if args.scroll_after_click is not None and (
        args.click is None or not (1 <= args.scroll_after_click <= len(args.click))
    ):
        raise SystemExit("--scroll-after-click must identify an existing one-based --click")
    if args.image_action and not args.image_tool:
        raise SystemExit("--image-action requires --image-tool")
    if args.key_action and not args.image and any(action != "f3" for action in args.key_action):
        if not (
            (all(action in {"f3", "f6"} for action in args.key_action))
            or (args.fake_text_preview and all(action in {"f3", "ctrl-f"} for action in args.key_action))
            or (args.fake_pdf_preview and all(action == "r" for action in args.key_action))
            or ((args.fake_diff_preview or args.fake_image_diff_preview)
                and all(action == "enter" for action in args.key_action))
        ):
            raise SystemExit(
                "only --key-action f3 may be used without an image fixture, "
                "except in a fake text preview run"
            )
    if args.type_text and not (
        args.fake_text_preview or args.fake_pdf_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
        or args.fake_diff_preview or args.fake_image_diff_preview or args.fake_model3d
    ):
        raise SystemExit("--type-text requires a fake text preview route")
    if (
        args.verify_return or args.verify_viewer_return or args.image_tool or args.key_action or args.type_text
        or args.fake_node_machine
    ) and shutil.which("wtype") is None:
        raise SystemExit("keyboard input requires wtype")
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
            "allow_server_decorations": args.allow_server_decorations,
            "fail_startup_output_query": args.fail_startup_output_query,
            "image_tool": args.image_tool,
            "image_action": args.image_action,
            "post_tool_delay_ms": args.post_tool_delay_ms,
            "key_action": args.key_action,
            "image_fixture": str(args.image.resolve()) if args.image else None,
            "click": args.click,
            "click_delay_ms": args.click_delay_ms,
            "capture_after_actions": args.capture_after_actions,
            "capture_after_key_actions": args.capture_after_key_actions,
            "click_after_key_action": args.click_after_key_action,
            "post_key_delay_ms": args.post_key_delay_ms,
            "move": args.move,
            "drag": args.drag,
            "scroll": args.scroll,
            "scroll_after_click": args.scroll_after_click,
            "audio_fixture": str(args.audio.resolve()) if args.audio else None,
            "pdf_fixture": str(args.pdf.resolve()) if args.pdf else None,
            "midi_fixture": str(args.midi.resolve()) if args.midi else None,
            "verify_midi_copy_pointer": args.verify_midi_copy_pointer,
            "fake_audio_transition": args.fake_audio_transition,
            "fake_text_preview": args.fake_text_preview,
            "fake_pdf_preview": args.fake_pdf_preview,
            "fake_csv_preview": args.fake_csv_preview,
            "fake_multisheet_xlsx": args.fake_multisheet_xlsx,
            "fake_model3d": args.fake_model3d,
            "fake_binary_diff": args.fake_binary_diff,
            "fake_diff_preview": args.fake_diff_preview,
            "fake_image_diff_preview": args.fake_image_diff_preview,
            "fake_reload_error": args.fake_reload_error,
            "fake_node_machine": args.fake_node_machine,
            "binary_diff_view": args.binary_diff_view,
            "verify_return": args.verify_return,
            "verify_viewer_return": args.verify_viewer_return,
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
    fake_text_daemon: FakeTextPreviewDaemon | None = None
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
        if (
            args.fake_text_preview or args.fake_pdf_preview or args.fake_csv_preview or args.fake_multisheet_xlsx
            or args.fake_model3d or args.fake_binary_diff or args.fake_diff_preview
            or args.fake_image_diff_preview or args.fake_reload_error
            or args.verify_midi_copy_pointer
        ):
            fake_text_daemon = FakeTextPreviewDaemon(
                config["runtime"], output / "fake-text-daemon-events.json",
                csv_preview=args.fake_csv_preview,
                xlsx_preview=args.fake_multisheet_xlsx,
                model3d_variant=args.fake_model3d,
                binary_diff_preview=args.fake_binary_diff,
                diff_preview=args.fake_diff_preview,
                image_diff_preview=args.fake_image_diff_preview,
                pdf_preview=args.fake_pdf_preview,
                reload_config_error=args.fake_reload_error,
                midi_export=args.verify_midi_copy_pointer,
            )
        capture_workbench(
            args, binary, output, config, niri_env, outputs, children, report,
            fake_text_daemon=fake_text_daemon,
        )
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
        if fake_text_daemon is not None:
            text_route = fake_text_daemon.close()
            if args.fake_binary_diff:
                report["fake_binary_diff"] = text_route
            elif args.verify_midi_copy_pointer:
                report["midi_copy_pointer"] = text_route
                if not text_route["pass"]:
                    report["failures"].append(
                        "private pointer click did not send one valid MIDI PushFile to Left Hand"
                    )
            elif args.fake_pdf_preview:
                report["fake_pdf_preview"] = text_route
            elif args.fake_diff_preview:
                report["fake_diff_preview"] = text_route
            elif args.fake_image_diff_preview:
                report["fake_image_diff_preview"] = text_route
            elif args.fake_reload_error:
                report["fake_reload_error"] = text_route
                if not any(event.get("kind") == "reload_config_error" for event in fake_text_daemon.events):
                    report["failures"].append("workbench did not send ReloadConfig to fake error daemon")
            elif args.fake_model3d:
                report["fake_model3d"] = text_route
            elif args.fake_multisheet_xlsx:
                report["fake_multisheet_xlsx"] = text_route
            elif args.fake_csv_preview:
                report["fake_csv_preview"] = text_route
            else:
                report["fake_text_preview"] = text_route
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
