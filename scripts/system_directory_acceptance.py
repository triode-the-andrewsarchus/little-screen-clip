#!/usr/bin/env python3
"""Identity-scoped sysfs summaries on the private Weston/Niri runner."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    for name, target in [('a-buses','/sys/bus'),('b-classes','/sys/class'),('c-power','/sys/power'),('d-mappings','/sys/dev/char'),('e-children','/sys/devices/platform')]:
        (root/name).symlink_to(target,target_is_directory=True)
    (root/'notes.txt').write_text('Ordinary text alongside bounded sysfs summaries.\n')


def exercise(session,root):
    session.observe('01-directory-strips',['a-buses','b-classes','Bus registration inventory'])
    session.click(848,201)
    session.observe('02-mixed-folder',['a-buses','b-classes','c-power'])
    for name,label,expected in [('a-buses','buses',['Bus registration inventory','usb','Registered device names','Driver directories']),('b-classes','classes',['Functional device classes','drm','net','Class member names']),('c-power','power',['System sleep configuration','Supported system sleep states','Configured suspend mode','Configured hibernation method'])]:
        x,y=session.word('02-mixed-folder' if label=='buses' else f'return-{previous}',name)
        session.click(x+32,y+55)
        session.observe(f'inspector-{label}',expected)
        session.key('Escape')
        session.observe(f'return-{label}',['a-buses','b-classes','c-power'])
        previous=label
    session.scroll_to_visible('03-child-row', ['e-children'], step=150, attempts=5)
    x,y=session.word('03-child-row','e-children')
    session.click(x+32,y+55)
    session.scroll_to_visible('04-child-inventory',['Child device inventory','directly registered children','non-atomic','registration snapshot'],step=150,attempts=12)
    session.key('Escape')
    session.observe('05-child-return',['Folders','e-children'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'Bus/class registration summaries and passive sleep configuration')


if __name__=='__main__':
    raise SystemExit(main())
