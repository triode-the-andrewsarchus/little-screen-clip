#!/usr/bin/env python3
"""Verify folder drops ask for intent and reuse project/preview placement."""
import json
from pathlib import Path
from file_manager_acceptance import Session,application_arguments,finish_run,wait_for
from file_manager_desktop_acceptance import drag,position,start_two_clients
from file_manager_home_acceptance import click_word,click_cropped_word
import ui_acceptance_capture as private

if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"));output=args.output_dir.resolve();output.mkdir(mode=0o700)
    children=private.ChildProcesses();report={"status":"running","appearance":args.appearance}
    try:
        source,target=output/"SOURCE",output/"DESTINATION";source.mkdir();target.mkdir()
        folder=source/"DropRoom";folder.mkdir();(folder/"one.txt").write_text("Keep this source")
        config,env,apps=start_two_clients(output,args.appearance,args.binary,[source,target],children)
        session=Session(output,env,config["niri_bin"],apps[1],children)
        session.observe("clients",["DropRoom"])
        if not click_cropped_word(session,"clients","Home",(1058,78,1123,110),65,120):session.click(1090,94)
        session.observe("home",["Make this place yours"])
        drag(session,position(session,"home","DropRoom",True),(970,450))
        session.observe("intent",["Add dropped folder","Folder preview","Project"])
        state=Path(env["XDG_STATE_HOME"])/"little-screen-clip/home.json"
        read=lambda:json.loads(state.read_text())["board"]
        assert not (folder/".fmproject").exists()
        click_word(session,"intent","preview",minimum_y=140)
        wait_for(lambda:state.exists() and any(c["content"]["kind"]=="folder" for c in read()["cards"]),"folder preview placed")
        session.observe("preview",["DropRoom","Filter"])
        drag(session,position(session,"preview","DropRoom",True),(970,450))
        session.observe("project-intent",["Add dropped folder","Project"])
        click_word(session,"project-intent","Project",minimum_y=140,control_bounds=(792,214,865,253))
        session.observe("project-form",["Create a project here","DropRoom"])
        assert not (folder/".fmproject").exists(),"choosing intent alone must not create project files"
        click_word(session,"project-form","Create",minimum_y=250,maximum_y=310,control_bounds=(1014,260,1137,295))
        wait_for(lambda:(folder/".fmproject").exists(),"explicit project creation")
        wait_for(lambda:any(c["content"]["kind"]=="project" for c in read()["cards"]),"dropped project receives an independent task panel")
        session.observe("project-placed",["DropRoom","Reload TODO"])
        assert (folder/"one.txt").read_text()=="Keep this source" and not list(target.iterdir())
        apps[0].terminate();apps[0].wait(timeout=5)
        session.close();session.restart(args.binary)
        session.observe("restart",["DropRoom"])
        assert {c["content"]["kind"] for c in read()["cards"]}=={"folder","project"}
        report.update(status="passed",observations=session.observations,private_runtime=str(config["runtime"]))
    except Exception as error:
        if "session" in locals() and session.process.poll() is None:session.observe("failure-state",[])
        report.update(status="failed",error=str(error),observations=session.observations if "session" in locals() else [])
    finally:result=finish_run(output,children,report)
    raise SystemExit(result)
