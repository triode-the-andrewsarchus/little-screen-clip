#!/usr/bin/env python3
"""Exercise folder-owned visibility, recovery and malformed-source preservation."""
from pathlib import Path
from PIL import Image
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_home_acceptance import click_word
from file_manager_session_acceptance import close_shelf


def menu(session,shot,name):
    click_word(session,shot,"Rules:",minimum_y=140,maximum_y=200)
    session.observe(name,["Reveal all files for recovery","Reload visibility rules"])


def colored_pixels(session,shot,color):
    with Image.open(session.output/(shot+".png")).convert("RGB") as picture:
        return sum(pixel==color for pixel in picture.getdata())


def exercise(session,root,binary,appearance):
    session.ocr_regions=[("contrast-line-visibility-error-ocr.png",(96,187,1178,215))]
    original={p:p.read_bytes() for p in root.rglob("*") if p.is_file()}
    text=session.observe("initial",["visible.txt","env","child","Rules:"])
    close_shelf(session,"initial",text)
    text=session.observe("selective-dotfile",["visible.txt","env","Shown"])
    assert "hidden.tmp" not in text and "secret" not in text
    shown=(27,53,49) if appearance=="dark" else (219,239,230)
    hidden=(67,43,34) if appearance=="dark" else (248,228,212)
    assert colored_pixels(session,"selective-dotfile",shown)>1000,"shown opinions need a painted solid background"
    menu(session,"selective-dotfile","recovery-menu")
    click_word(session,"recovery-menu","Reveal",minimum_y=180)
    text=session.observe("recovery",["secret","hide","Rules: off"])
    session.scroll_to_visible("recovered-hidden-file",["hidden.tmp","Hidden"])
    assert colored_pixels(session,"recovered-hidden-file",hidden)>1000,"ignored hide opinions remain visibly labeled and painted"
    session.close();session.restart(binary,root)
    text=session.observe("restarted",["visible.txt","env","Rules: on"])
    close_shelf(session,"restarted",text)
    text=session.observe("restarted-policy",["visible.txt","env"])
    assert "hidden.tmp" not in text and "secret" not in text
    future=b"version=99\npatterns=[]\n"
    (root/".show").write_bytes(future)
    session.key("F5")
    session.observe("bad-rule",["Visibility rules:","Unsupported visibility version","hidden.tmp"])
    assert (root/".show").read_bytes()==future
    (root/".show").write_bytes(original[root/".show"])
    session.key("F5")
    text=session.observe("repaired",["visible.txt","env","Shown"])
    assert "hidden.tmp" not in text and "Unsupported visibility" not in text
    click_word(session,"repaired","child",minimum_y=230)
    session.key("Return")
    text=session.observe("local-overrides",["other.txt","env","Shown"])
    assert "local.txt" not in text
    for path,data in original.items(): assert path.read_bytes()==data


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    (root/".hide").write_text("version=1\npatterns=['*.tmp']\n")
    (root/".show").write_text("version=1\npatterns=['.env']\n")
    for name in ["visible.txt","hidden.tmp",".env",".secret"]: (root/name).write_text(name+" original\n")
    child=root/"child";child.mkdir()
    (child/".hide").write_text("version=1\npatterns=['local.txt']\n")
    for name in ["local.txt","other.txt",".env"]: (child/name).write_text(name+" original\n")
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary,args.appearance),
        "Selective dotfiles, solid labeled rows, reveal-all recovery, restart, malformed-source preservation and inherited/local overrides"))
