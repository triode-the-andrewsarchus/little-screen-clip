#!/usr/bin/env python3
"""Generate an owned EPUB with navigation order distinct from archive/manifest order."""
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED, ZIP_STORED
from io import BytesIO
from PIL import Image

def make(path, version=3):
    image = BytesIO()
    Image.new("RGB", (320, 160), (20, 90, 230)).save(image, "PNG")
    namespace = 'xmlns="http://www.w3.org/1999/xhtml"'
    with ZipFile(path, "w", compression=ZIP_DEFLATED) as z:
        z.writestr("mimetype", "application/epub+zip", compress_type=ZIP_STORED)
        z.writestr("META-INF/container.xml", '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="Books/main.opf" media-type="application/oebps-package+xml"/></rootfiles></container>')
        package = f'''<package xmlns="http://www.idpf.org/2007/opf" version="{version}.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>A Small Journey</dc:title><dc:creator>Example Author</dc:creator><dc:language>en</dc:language><meta name="cover" content="cover"/></metadata><manifest><item id="second" href="text/second.xhtml" media-type="application/xhtml+xml"/><item id="first" href="text/first.xhtml" media-type="application/xhtml+xml"/><item id="cover" href="images/cover.png" media-type="image/png" properties="cover-image"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="first"/><itemref idref="second"/></spine></package>'''
        if version == 2:
            package = package.replace('<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>', '')
        z.writestr("Books/main.opf", package)
        z.writestr("Books/nav.xhtml", f'<html {namespace}><body><nav><ol><li><a href="text/first.xhtml#start">Departure</a></li><li><a href="text/second.xhtml">Arrival</a></li></ol></nav></body></html>')
        z.writestr("Books/toc.ncx", '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>Departure</text></navLabel><content src="text/first.xhtml#start"/></navPoint><navPoint><navLabel><text>Arrival</text></navLabel><content src="text/second.xhtml"/></navPoint></navMap></ncx>')
        z.writestr("Books/text/second.xhtml", f'<html {namespace}><body><h1>Arrival</h1><p>The harbor welcomes our small boat.</p></body></html>')
        z.writestr("Books/text/first.xhtml", f'<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd"><html {namespace}><head><title>Hidden title</title><style>Hidden CSS</style></head><body><h1 id="start">Departure</h1><p>We begin our journey beside the blue sea. A &amp; B.</p><img src="../images/cover.png" alt="Blue sea illustration"/><script>HIDDEN SCRIPT</script><img src="https://example.invalid/remote.png"/><p>Evening follows the shore.</p></body></html>')
        z.writestr("Books/images/cover.png", image.getvalue())

if __name__ == '__main__':
    root = Path(__file__).resolve().parents[1] / 'crates/little-clip-core/tests/fixtures/ebooks'
    make(root / 'journey.epub')
    make(root / 'journey-v2.epub', 2)
