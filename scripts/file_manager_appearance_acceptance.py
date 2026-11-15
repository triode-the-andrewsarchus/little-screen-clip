#!/usr/bin/env python3
"""Exercise the Files View appearance menu on a private native compositor."""
from pathlib import Path
import tomllib
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word, enter_home
import ui_acceptance_capture as private


class AppearanceDaemon(private.FakeTextPreviewDaemon):
    def __init__(self,runtime,log):
        self.reloads=0
        super().__init__(runtime,log)

    def _response(self,request):
        if request=="ReloadConfig":
            self.reloads+=1
            if self.reloads==2:
                return {"Error":{"error":"Acceptance reload failed"}},"appearance_error"
            return {"Success":{"message":"Applied"}},"appearance_reload"
        return super()._response(request)


def canvas_brightness(session, shot):
    with Image.open(session.output / f"{shot}.png").convert("RGB") as picture:
        pixel = picture.getpixel((1000, 600))
        return sum(pixel) / 3


def choose(session, shot, mode):
    click_word(session, shot, "View", minimum_y=30, maximum_y=85)
    session.observe("view-menu-" + mode, ["Appearance"])
    click_word(session, "view-menu-" + mode, "Appearance", minimum_y=60, maximum_y=300)
    session.observe("appearance-menu-" + mode, [mode.capitalize()])
    click_word(session, "appearance-menu-" + mode, mode.capitalize(), minimum_y=60, maximum_y=400)
    session.observe("selected-" + mode, ["Pinned references: 0"])
    brightness = canvas_brightness(session, "selected-" + mode)
    assert brightness > 180 if mode == "light" else brightness < 80, (mode, brightness)


def exercise(session, root, binary, initial, daemon):
    session.observe("folder", ["Home"])
    enter_home(session, "folder")
    session.observe("home", ["Make this place yours"])
    config = session.output / "scratch/config.toml"
    config.write_text(config.read_text() + "\n[file_manager]\nsmall_container_items = 17\n")
    opposite = "light" if initial == "dark" else "dark"
    choose(session, "home", opposite)
    wait_for(lambda: tomllib.loads(config.read_text())["workbench"]["appearance"] == opposite,
             "shared appearance persisted")
    assert tomllib.loads(config.read_text())["file_manager"]["small_container_items"] == 17
    wait_for(lambda:daemon.reloads==1,"saved appearance applied to daemon")
    session.close()
    session.restart(binary)
    session.observe("restarted", ["Pinned references: 0"])
    brightness = canvas_brightness(session, "restarted")
    assert brightness > 180 if opposite == "light" else brightness < 80
    choose(session, "restarted", initial)
    assert tomllib.loads(config.read_text())["workbench"]["appearance"] == initial
    assert tomllib.loads(config.read_text())["file_manager"]["small_container_items"] == 17
    session.observe("reload-failure",["Could not update little screen", "Acceptance reload failed"])
    assert daemon.reloads==2,"daemon reload failures must not trigger an automatic resend"
    click_word(session,"reload-failure","Dismiss",minimum_y=75,maximum_y=165)
    session.observe("failure-dismissed",["Pinned references: 0"])


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    servers=[]
    def prepare(env,children):
        servers.append(AppearanceDaemon(Path(env["XDG_RUNTIME_DIR"]),output/"appearance-ipc.json"))
    try:
        result=run_application_acceptance(args,output,root,
            lambda session,root:exercise(session,root,args.binary,args.appearance,servers[0]),
            "Home View menu, both theme transitions, shared preference, daemon reload and inline failure feedback",prepare_environment=prepare)
    finally:
        for server in servers:server.close()
    raise SystemExit(result)
