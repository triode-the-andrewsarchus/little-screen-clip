#!/usr/bin/env python3
"""Capture mixed prepared cards through the existing private Weston/Niri runner.

Accepts ui_acceptance_capture.py options. Set LITTLE_CLIP_NATIVE_MIRROR=1 for
native mode, or leave it unset for the normal raster baseline. The fixture is
isolated from the user's daemon, clipboard, compositor and audio devices.
"""
import sys
import json
import subprocess
from PIL import Image
import uuid
import ui_acceptance_capture as private


class MixedMirrorDaemon(private.FakeTextPreviewDaemon):
    SLOTS = {
        "Undo": "undo", "LeftHand": "left_hand", "RightHand": "right_hand",
        "LeftPrior1": "left_prior1", "LeftPrior2": "left_prior2",
        "LeftPrior3": "left_prior3", "RightPrior2": "right_prior2",
        "RightPrior1": "right_prior1",
    }

    def _state(self):
        def item(index, payload, title, subtitle):
            return {"id": self.ITEM_ID if index == 1 else str(uuid.UUID(int=index)),
                    "created_at": 1, "payload": payload, "title": title,
                    "subtitle": subtitle, "details": [], "handler_tag": "PREPARED"}
        state = {field: None for field in self.SLOTS.values()}
        state.update(cursor="LeftHand", selected=["LeftHand"])
        state["left_hand"] = item(1, {"Text": "fn main() {\n    let café = 42;\n    // 東京\n}"}, "sample.rs", "Prepared source text")
        state["left_prior1"] = item(2, {"Markdown": "# Reference\n**Rich text**\n- Retains formatting"}, "Notes", "Raster fallback")
        colors = [channel for y in range(48) for x in range(64)
                  for channel in ([40, 160, 210, 255] if (x // 8 + y // 8) % 2 else [220, 130, 60, 255])]
        state["left_prior2"] = item(3, {"Image": {"width": 64, "height": 48,
            "mime": "image/png", "thumbnail": {"width": 64, "height": 48,
            "premultiplied_rgba": colors}}}, "Colors.png", "Prepared thumbnail")
        state["left_prior3"] = item(4, {"FileBatch": ["/unavailable/reference.txt"]}, "Reference.txt", "Path reference")
        state["right_prior1"] = item(5, {"Color": {"hex": "#42B6A4", "r": 66, "g": 182, "b": 164}}, "Sea green", "RGB 66, 182, 164")
        state["right_hand"] = item(6, {"AudioRef": {"path": "/unavailable/audio.wav",
            "duration_ms": 12000, "sample_rate": 48000, "channels": 2, "format": "WAV",
            "waveform": {"channels": 2, "sample_rate": 48000, "duration_ms": 12000,
                "ch1_peaks": [0.1, 0.7, 0.2, 0.9, 0.4, 0.6],
                "ch2_peaks": [0.8, 0.2, 0.6, 0.3, 0.9, 0.1],
                "spectral_colors": [[40, 160, 210], [90, 180, 130], [220, 170, 60], [180, 90, 160], [110, 120, 210], [50, 180, 160]],
                "is_snippet": False}}}, "Stereo.wav", "00:12 · 48 kHz · Stereo")
        return state

    def _response(self, request):
        state = self._state()
        if request == "GetStateSummary":
            cards = [{"slot": slot, "item_id": state[field]["id"],
                      "payload_kind": next(iter(state[field]["payload"])), "preview_phase": "Ready"}
                     for slot, field in self.SLOTS.items() if state[field]]
            return {"StateSummary": {"cards": cards, "cursor": "LeftHand", "selected": ["LeftHand"]}}, "summary"
        if isinstance(request, dict) and "GetSlotItem" in request:
            return {"SlotItem": state[self.SLOTS[request["GetSlotItem"]]]}, "slot_item"
        return super()._response(request)


if __name__ == "__main__":
    sys.argv.append("--fake-text-preview")
    args = private.create_parser().parse_args()
    result = private.main(fake_text_factory=MixedMirrorDaemon)
    if result == 0:
        output = args.output_dir.expanduser().resolve()
        screenshot = output / 'settled.png'
        with Image.open(screenshot) as image:
            image.resize((image.width * 2, image.height * 2)).save(output / 'content-ocr.png')
        text = subprocess.check_output(['tesseract', str(output / 'content-ocr.png'), 'stdout', '-l', 'eng'], stderr=subprocess.DEVNULL, text=True, timeout=30)
        (output / 'content.txt').write_text(text)
        normalized = ''.join(text.split())
        missing = [value for value in ['sample.rs', 'Stereo.wav'] if value not in normalized]
        report_path = output / 'report.json'
        report = json.loads(report_path.read_text())
        report['mixed_mirror_content'] = {'missing': missing}
        if missing:
            report['status'] = 'failed'
            report['failures'].append(f'Mirror content not displayed: {missing}')
            result = 1
        report_path.write_text(json.dumps(report, indent=2) + '\n')
    raise SystemExit(result)
