#!/usr/bin/env python3
"""Exercise reference retargeting after native rename/move and metadata failure."""
import fcntl
import json
import os
from pathlib import Path
import uuid
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word, enter_home
from file_manager_session_acceptance import close_shelf, line_count


def load(path):return json.loads(path.read_text())


def pin_path(state,identifier):
    return next(pin["path"] for pin in load(state/"home.json")["pins"] if pin["id"]==identifier)


def rename(session,shot,source,name,label):
    click_word(session,shot,source,minimum_y=230)
    session.key("F2")
    session.observe(label+"-dialog",["Rename","Name:"])
    session.key("a","ctrl")
    private.run_checked(["wtype",name],env=session.env)
    session.key("Return")


def exercise(session,root,binary,appearance):
    text=session.observe("initial",["project-old","Home"])
    close_shelf(session,"initial",text)
    session.observe("initial-ready",["source.txt","Home"])
    session.close()
    state=Path(session.env["XDG_STATE_HOME"])/"little-screen-clip";state.mkdir(parents=True,exist_ok=True)
    source=root/"source.txt";project=root/"project-old";project_id=load(project/".fmproject")["id"]
    file_pin=str(uuid.uuid4());folder_pin=str(uuid.uuid4());group=str(uuid.UUID(int=1))
    metadata=os.stat(source);identity=[metadata.st_dev,metadata.st_ino,metadata.st_size,metadata.st_mtime_ns//10**9,metadata.st_mtime_ns%10**9,metadata.st_ctime_ns//10**9,metadata.st_ctime_ns%10**9,metadata.st_mode]
    (state/"home.json").write_text(json.dumps({"version":1,"pins":[{"id":file_pin,"path":str(source),"folder":False},{"id":folder_pin,"path":str(project),"folder":True}]}))
    (state/"projects.json").write_text(json.dumps({"version":1,"projects":[{"id":project_id,"title":"Project","folder":str(project)}]}))
    (state/"organization.json").write_text(json.dumps({"version":2,"session":{"enabled":True,"definitions":{"name":"Review","meanings":["Red","Yellow","Green"]},"marks":[{"target":{"path":str(source),"identity":identity},"color":"green"}],"previous":[],"recovery_revision":0},"sets":{"groups":[{"id":group,"parent":None,"name":"A","color":[91,151,201]}],"members":[{"path":str(source),"group":group}]}}))
    session.restart(binary,root)
    text=session.observe("seeded",["project-old","Home"])
    close_shelf(session,"seeded",text)
    session.observe("seeded-ready",["source.txt","Home"])
    enter_home(session,"seeded-ready")
    session.observe("pinned-home",["source.txt","project-old","Back to folder"])
    wait_for(lambda:load(state/"home.json")["version"]==2,"legacy pins migrated")
    click_word(session,"pinned-home","Back",minimum_y=85,maximum_y=145)
    session.observe("before-rename",["source.txt","project-old"])
    lock=open(state/".home.lock","a+b");fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    rename(session,"before-rename","source.txt","renamed.txt","file-rename")
    renamed=root/"renamed.txt"
    wait_for(lambda:renamed.exists() and not source.exists(),"actual file renamed")
    session.observe("reference-error",["File operation finished","Home references could not be updated","Retry Home reference updates"])
    assert pin_path(state,file_pin)==str(source)
    session.key("q","ctrl")
    session.observe("close-refused",["Retry Home reference updates"])
    assert session.process.poll() is None
    fcntl.flock(lock,fcntl.LOCK_UN);lock.close()
    click_word(session,"close-refused","Retry",minimum_y=90,maximum_y=300)
    wait_for(lambda:pin_path(state,file_pin)==str(renamed),"Home references retried")
    wait_for(lambda:load(state/"organization.json")["sets"]["members"][0]["path"]==str(renamed),"set reference renamed")
    session.observe("renamed",["renamed.txt","Session:"])
    green=(89,194,135) if appearance=="dark" else (23,121,73)
    assert line_count(session,"renamed",green)>0,"rename should retain the review mark"
    click_word(session,"renamed","renamed.txt",minimum_y=230)
    session.key("c","ctrl");session.address(root/"copies")
    session.observe("copy-target",["copies","Name"])
    session.key("v","ctrl")
    wait_for(lambda:(root/"copies/renamed.txt").exists(),"file copied")
    session.observe("copied",["renamed.txt","1 completed"])
    assert pin_path(state,file_pin)==str(renamed)
    assert line_count(session,"copied",green)==0,"copies must not inherit the source's mark"
    session.address(root);session.observe("move-source",["renamed.txt","project-old"])
    click_word(session,"move-source","renamed.txt",minimum_y=230)
    session.key("x","ctrl");session.address(root/"moved")
    session.observe("move-target",["moved","Name"]);session.key("v","ctrl")
    moved=root/"moved/renamed.txt"
    wait_for(lambda:moved.exists() and not renamed.exists(),"file moved")
    wait_for(lambda:pin_path(state,file_pin)==str(moved),"Home move acknowledged")
    wait_for(lambda:load(state/"organization.json")["session"]["marks"][0]["target"]["path"]==str(moved),"review mark moved")
    session.observe("moved",["renamed.txt","Session:"])
    assert line_count(session,"moved",green)>0
    session.address(root);session.observe("project-source",["project-old"])
    rename(session,"project-source","project-old","project-new","project-rename")
    new_project=root/"project-new"
    wait_for(lambda:new_project.is_dir() and not project.exists(),"project folder renamed")
    wait_for(lambda:load(state/"projects.json")["projects"][0]["folder"]==str(new_project),"registry retargeted")
    wait_for(lambda:pin_path(state,folder_pin)==str(new_project),"project shortcut retargeted")
    assert load(new_project/".fmproject")["id"]==project_id
    session.observe("project-renamed",["project-new","Home"])
    enter_home(session,"project-renamed")
    session.observe("updated-home",["renamed.txt","project-new","Back to folder"])
    click_word(session,"updated-home","renamed.txt",minimum_y=145)
    session.observe("pinned-file-open",["Back to folder","reviewed source"])
    session.key("Escape");session.observe("file-parent",["renamed.txt","Home"])
    session.key("Left","alt");session.observe("home-return",["project-new","Back to folder"])
    click_word(session,"home-return","project-new",minimum_y=145)
    session.observe("project-from-home",["TODO.md","README.md"])
    session.close();session.restart(binary,root)
    session.observe("restarted",["project-new","Home"])
    enter_home(session,"restarted");session.observe("persisted-home",["renamed.txt","project-new"])
    assert load(state/"projects.json")["projects"][0]["id"]==project_id
    assert moved.read_text()=="reviewed source\n" and (root/"copies/renamed.txt").read_bytes()==moved.read_bytes()
    assert (new_project/"TODO.md").read_text()=="- [ ] Review project\n"


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    (root/"source.txt").write_text("reviewed source\n")
    for name in ["copies","moved","project-old"]:(root/name).mkdir()
    project=root/"project-old"
    (project/".fmproject").write_text(json.dumps({"version":1,"id":str(uuid.uuid4()),"title":"Project","readme":"README.md","todo":"TODO.md"}))
    (project/"README.md").write_text("# Project\n")
    (project/"TODO.md").write_text("- [ ] Review project\n")
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary,args.appearance),
        "Renames/moves retarget references, copies do not, metadata failure blocks close and retries safely, project identity and shortcuts survive restart"))
