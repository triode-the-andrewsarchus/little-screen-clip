#!/usr/bin/env python3
"""SCSI/ATA metadata on private Weston/Niri without storage device I/O."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    host=next(iter(sorted(Path('/sys/class/scsi_host').iterdir())))
    port=next(iter(sorted(Path('/sys/class/ata_port').iterdir())))
    link=next(iter(sorted(Path('/sys/class/ata_link').iterdir())))
    (root/'a-host').symlink_to(host,target_is_directory=True)
    (root/'b-port').symlink_to(port,target_is_directory=True)
    (root/'c-link').symlink_to(link,target_is_directory=True)
    (root/'notes.txt').write_text('Ordinary text beside read-only storage controller metadata.\n')


def exercise(session,root):
    session.observe('01-storage-bus-strips',['a-host','b-port','SCSI','ATA'])
    session.click(848,201)
    session.observe('02-mixed-folder',['a-host','b-port','c-link','notes.txt'])
    x,y=session.word('02-mixed-folder','a-host')
    session.click(x+32,y+55)
    session.observe('03-host-inspector',['SCSI host metadata','SCSI host driver label','SCSI host kernel state','Host queue capacity'])
    session.key('Escape')
    session.observe('04-browser-return',['a-host','b-port','c-link'])
    x,y=session.word('04-browser-return','b-port')
    session.click(x+32,y+55)
    session.observe('05-port-inspector',['ATA port metadata','Host-local ATA port number','Registered port-multiplier link count'])
    session.key('Escape')
    session.observe('06-folder-return',['a-host','b-port','c-link'])
    x,y=session.word('06-folder-return','c-link')
    session.click(x+32,y+55)
    session.observe('07-link-inspector',['ATA link metadata','Reported SATA link speed','Libata SATA speed limit'])
    session.key('Escape')
    session.observe('08-final-folder',['a-host','b-port','notes.txt'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'SCSI host and ATA port/link metadata')


if __name__=='__main__':
    raise SystemExit(main())
