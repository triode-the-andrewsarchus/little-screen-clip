#!/usr/bin/env python3
"""Metadata-only kernel object/control filesystems in private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    for name,target in [('a-config','/sys/kernel/config'),('b-bpf','/sys/fs/bpf'),('c-pstore','/sys/fs/pstore')]:
        (root/name).symlink_to(target,target_is_directory=True)
    (root/'notes.txt').write_text('Ordinary text next to protected kernel filesystems.\n')


def exercise(session,root):
    session.observe('01-filesystem-strips',['a-config','b-bpf','Kernel configuration objects'])
    session.click(848,201)
    session.observe('02-mixed-folder',['a-config','b-bpf','c-pstore'])
    previous='02-mixed-folder'
    for name,label,expected in [('a-config','config',['Kernel configuration objects','creates no objects']),('b-bpf','bpf',['Pinned BPF objects','does not acquire object handles']),('c-pstore','pstore',['Persistent kernel records','reads no record payloads'])]:
        x,y=session.word(previous,name)
        session.click(x+32,y+55)
        session.observe(f'inspector-{label}',expected)
        session.key('Escape')
        previous=f'return-{label}'
        session.observe(previous,['a-config','b-bpf','c-pstore'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'Metadata-only configfs, BPF and persistent kernel record previews')


if __name__=='__main__':
    raise SystemExit(main())
