#!/usr/bin/env python3
"""Exercise folder-owned visibility, recovery and malformed-source preservation."""
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_home_acceptance import click_word
from file_manager_session_acceptance import close_shelf


def menu(session,shot,name):
    click_word(session,shot,"Rules:",minimum_y=140,maximum_y=200)
    session.observe(name,["Reveal all files for recovery","Reload visibility rules"])


def exercise(session,root,binary):
    session.ocr_regions=[("contrast-line-visibility-error-ocr.png",(96,187,1178,215))]
    original={p:p.read_bytes() for p in root.rglob("*") if p.is_file()}
    text=session.observe("initial",["visible.txt",".env","child","Rules:"])
    close_shelf(session,"initial",text)
    text=session.observe("selective-dotfile",["visible.txt",".env","Shown"])
    assert "hidden.tmp" not in text and ".secret" not in text
    menu(session,"selective-dotfile","recovery-menu")
    click_word(session,"recovery-menu","Reveal",minimum_y=180)
    text=session.observe("recovery",[".secret",".hide","Rules: off"])
    session.scroll_to_visible("recovered-hidden-file",["hidden.tmp","Hidden"])
    session.close();session.restart(binary,root)
    text=session.observe("restarted",["visible.txt",".env","Rules: on"])
    close_shelf(session,"restarted",text)
    text=session.observe("restarted-policy",["visible.txt",".env"])
    assert "hidden.tmp" not in text and ".secret" not in text
    future=b"version=99\npatterns=[]\n"
    (root/".show").write_bytes(future)
    session.key("F5")
    session.observe("bad-rule",["Visibility rules:","Unsupported visibility version","hidden.tmp"])
    assert (root/".show").read_bytes()==future
    (root/".show").write_bytes(original[root/".show"])
    session.key("F5")
    text=session.observe("repaired",["visible.txt",".env","Shown"])
    assert "hidden.tmp" not in text and "Unsupported visibility" not in text
    click_word(session,"repaired","child",minimum_y=230)
    session.key("Return")
    text=session.observe("local-overrides",["other.txt",".env","Shown"])
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
        lambda session,root:exercise(session,root,args.binary),
        "Selective dotfiles, solid labeled rows, reveal-all recovery, restart, malformed-source preservation and inherited/local overrides"))
