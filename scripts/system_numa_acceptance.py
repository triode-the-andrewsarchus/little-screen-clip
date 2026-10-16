#!/usr/bin/env python3
"""NUMA membership, memory and distance previews on private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    nodes=Path('/sys/devices/system/node')
    node=next(nodes.glob('node[0-9]*'))
    (root/'a-node').symlink_to(node,target_is_directory=True)
    (root/'b-topology').symlink_to(nodes,target_is_directory=True)
    (root/'c-distance').symlink_to(node/'distance')
    (root/'notes.txt').write_text('Ordinary text beside read-only NUMA topology.\n')


def exercise(session,root):
    session.observe('01-numa-strips',['a-node','b-topology','NUMA'])
    session.click(848,201)
    session.observe('02-mixed-folder',['a-node','b-topology','c-distance','notes.txt'])
    x,y=session.word('02-mixed-folder','a-node')
    session.click(x+32,y+55)
    session.observe('03-node-inspector',['NUMA distance matrix','CPUs','Total','Free','Relative distances'])
    session.key('Escape')
    session.observe('04-browser-return',['a-node','b-topology'])
    x,y=session.word('04-browser-return','b-topology')
    session.click(x+32,y+55)
    session.observe('05-topology-inspector',['NUMA distance matrix','represented nodes','online distance columns'])
    session.key('Escape')
    session.observe('06-folder-return',['a-node','b-topology','c-distance'])
    x,y=session.word('06-folder-return','c-distance')
    session.click(x+16,y,delay=0.05)
    session.click(x+16,y)
    session.observe('07-distance-inspector',['NUMA distance matrix','Relative distances','distance'])
    session.key('Escape')
    session.observe('08-final-folder',['a-node','b-topology','c-distance'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'NUMA topology and relative distances')


if __name__=='__main__':
    raise SystemExit(main())
