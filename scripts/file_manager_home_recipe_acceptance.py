#!/usr/bin/env python3
"""Verify explicit Home recipe activation against an isolated fake daemon."""
import json
from pathlib import Path
from file_manager_acceptance import application_arguments,media_fixture_directory,run_application_acceptance,wait_for
from file_manager_home_acceptance import click_word,enter_home,click_cropped_word
from file_manager_home_regions_acceptance import trace
import ui_acceptance_capture as private


class RecipeDaemon(private.FakeTextPreviewDaemon):
    def __init__(self,runtime,log_path):
        self.executions=[]
        super().__init__(runtime,log_path)

    def _response(self,request):
        if request=="ListRecipes":
            return {"RecipeList":["Acceptance recipe"]},"recipe_list"
        if isinstance(request,dict) and "ExecuteRecipeWithPrompt" in request:
            self.executions.append(request["ExecuteRecipeWithPrompt"])
            if len(self.executions)==2:
                return {"Error":{"error":"Acceptance recipe refused this run"}},"recipe_error"
            return {"PipelineExecuted":{"node_previews":{},"output_title":None,"duration_ms":1}},"recipe_run"
        return super()._response(request)


def exercise(session,root,binary,daemon):
    state=Path(session.env["XDG_STATE_HOME"])/"little-screen-clip/home.json"
    session.observe("folder",["Home"])
    enter_home(session,"folder")
    session.observe("home",["Make this place yours"])
    trace(session,[(700,370),(900,370),(900,530),(700,530),(700,370)])
    session.observe("draft",["Run saved recipe"])
    click_word(session,"draft","Run",minimum_y=210,maximum_y=255,control_bounds=(265,215,403,254))
    session.observe("recipe-mode",["Choose a saved recipe","Optional prompt"])
    assert not daemon.executions
    click_word(session,"recipe-mode","Choose",minimum_y=200,maximum_y=420)
    session.observe("recipes",["Acceptance recipe"])
    click_word(session,"recipes","Acceptance",minimum_y=220,maximum_y=470)
    session.observe("selected",["Acceptance recipe","Optional prompt"])
    x,y=session.word("selected","Name",tree=True)
    session.click(x+150,y);session.key("a","ctrl")
    private.run_checked(["wtype","Run tidy"],env=session.env)
    x,y=session.word("selected","Optional",tree=True)
    session.click(x+180,y)
    private.run_checked(["wtype","fixture prompt"],env=session.env)
    session.observe("configured",["Run tidy","fixture prompt"])
    assert not daemon.executions
    x,y=session.word("configured","Optional",tree=True)
    if not click_cropped_word(session,"configured","Save",(100,y+48,212,y+92),240,500):
        session.click(x+45,y+68)
    wait_for(lambda:state.exists() and len(json.loads(state.read_text())["board"].get("regions",[]))==1,"recipe region saved")
    session.observe("saved",["Pinned references: 0"])
    assert not daemon.executions,"saving a recipe region must never execute it"
    saved=json.loads(state.read_text())
    assert saved["board"]["regions"][0]["action"]=={"kind":"recipe","name":"Acceptance recipe","prompt":"fixture prompt"}
    assert not saved["pins"]
    hover=session.children.start("recipe-hover",[str(session.pointer),"move",*map(str,session.logical_size),"800","450"],env=session.env,
        cwd=session.output/"scratch",log_path=session.output/"hover.log")
    session.observe("hover",["Run tidy"],preserve_pointer=True)
    assert not daemon.executions,"hover must not execute a recipe"
    hover.terminate();hover.wait(timeout=3)
    session.click(800,450)
    wait_for(lambda:len(daemon.executions)==1,"explicit recipe dispatched once")
    session.observe("executed",["Pinned references: 0"])
    assert daemon.executions==[{"recipe_name":"Acceptance recipe","prompt_text":"fixture prompt"}]
    session.close();session.restart(binary)
    session.observe("restarted",["Pinned references: 0"])
    assert len(daemon.executions)==1,"loading persisted actions must not execute them"
    session.click(800,450)
    session.observe("failure-visible",["Acceptance recipe refused this run"])
    assert len(daemon.executions)==2,"execution failures must not cause an automatic retry"


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    servers=[]
    def prepare(env,children):
        servers.append(RecipeDaemon(Path(env["XDG_RUNTIME_DIR"]),output/"recipe-ipc.json"))
    try:
        result=run_application_acceptance(args,output,root,
            lambda session,root:exercise(session,root,args.binary,servers[0]),
            "Recipe picker, optional prompt, no execution on save/hover/load, exact explicit dispatch and failure without retry",prepare_environment=prepare)
    finally:
        for server in servers:server.close()
    raise SystemExit(result)
