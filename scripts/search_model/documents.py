#!/usr/bin/env python3
"""Bounded, passive text extraction. No rendering, links, macros or formula execution."""
import argparse
import codecs
from html.parser import HTMLParser
import io
import json
import os
import posixpath
import re
import resource
import stat
import xml.etree.ElementTree as ET
import zipfile

SOURCE_LIMIT = 64 * 1024 * 1024
TEXT_LIMIT = 8 * 1024 * 1024
XML_LIMIT = 8 * 1024 * 1024
XML_TOTAL = 32 * 1024 * 1024
PART_LIMIT = 512
PLAIN = {"txt", "text", "md", "markdown", "rst", "org", "csv", "tsv", "log"}
KINDS = PLAIN | {"html", "htm", "xhtml", "docx", "odt", "pptx", "xlsx"}


def decode(data, truncated=False, encoding=None):
    if encoding is None:
        encoding = "utf-8-sig"
        for marker, name in [(codecs.BOM_UTF32_LE, "utf-32"), (codecs.BOM_UTF32_BE, "utf-32"),
                             (codecs.BOM_UTF16_LE, "utf-16"), (codecs.BOM_UTF16_BE, "utf-16")]:
            if data.startswith(marker):
                encoding = name
                break
    return codecs.getincrementaldecoder(encoding)(errors="strict").decode(data, final=not truncated)


def local(tag):
    return tag.rsplit("}", 1)[-1]


def xml(data):
    text = decode(data)
    if re.search(r"<!\s*(?:DOCTYPE|ENTITY)\b", text, re.I):
        raise ValueError("Document type/entity declarations are not admitted")
    depth = count = 0
    parser = ET.iterparse(io.StringIO(text), events=("start", "end"))
    for event, element in parser:
        if event == "start":
            depth += 1
            count += 1
            if depth > 128 or count > 250000:
                raise ValueError("XML depth/element budget exceeded")
        else:
            depth -= 1
    return parser.root


def text_nodes(element):
    # Run boundaries do not imply word boundaries (e.g. bold inside a word).
    return "".join(node.text or "" for node in element.iter() if local(node.tag) == "t")


def paragraphs(root):
    return "\n".join(text_nodes(node) for node in root.iter() if local(node.tag) == "p")


class HtmlText(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.parts = []
        self.suppressed = []
        self.partial = False

    def handle_starttag(self, tag, attrs):
        if tag in {"script", "style", "template", "noscript"}:
            self.suppressed.append(tag)
        if self.suppressed:
            return
        if tag in {"p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "section", "article", "td", "th"}:
            self.parts.append("\n")
        if tag == "img":
            self.parts.append(dict(attrs).get("alt", ""))
            self.partial = True
        if tag in {"iframe", "object", "embed"}:
            self.partial = True

    def handle_endtag(self, tag):
        if self.suppressed:
            if tag == self.suppressed[-1]:
                self.suppressed.pop()
            return
        if tag in {"p", "div", "li", "tr", "section", "article"}:
            self.parts.append("\n")

    def handle_data(self, data):
        if not self.suppressed:
            self.parts.append(data)


class Package:
    def __init__(self, source):
        self.archive = zipfile.ZipFile(source)
        entries = self.archive.infolist()
        self.names = {entry.filename for entry in entries}
        if len(entries) > 4096 or len(self.names) != len(entries):
            raise ValueError("ZIP entry budget exceeded or duplicate member names")
        if any(name.startswith("/") or ".." in name.split("/") or "\\" in name for name in self.names):
            raise ValueError("Invalid package member path")
        self.read_bytes = 0
        self.partial = any("/media/" in name or "/embeddings/" in name or name.endswith("vbaProject.bin") for name in self.names)

    def read(self, name):
        info = self.archive.getinfo(name)
        if info.file_size > XML_LIMIT or self.read_bytes + info.file_size > XML_TOTAL:
            raise ValueError("Document XML expansion budget exceeded")
        with self.archive.open(info) as stream:
            data = stream.read(XML_LIMIT + 1)
        self.read_bytes += len(data)
        if len(data) > XML_LIMIT or self.read_bytes > XML_TOTAL:
            raise ValueError("Document XML expansion budget exceeded")
        return xml(data)

    def relationships(self, name, base):
        result = {}
        for node in self.read(name):
            if node.attrib.get("TargetMode") == "External":
                continue  # External links are data; never fetch them.
            target = node.attrib.get("Target", "")
            path = posixpath.normpath(target.lstrip("/") if target.startswith("/") else posixpath.join(base, target))
            if path.startswith("../") or ":" in path or "\\" in path:
                raise ValueError("Invalid internal relationship")
            result[node.attrib.get("Id")] = path
        return result

    def docx(self):
        names = ["word/document.xml"] + sorted(name for name in self.names if re.fullmatch(r"word/(?:header\d+|footer\d+|footnotes|endnotes|comments)\.xml", name))
        for name in names:
            yield name, paragraphs(self.read(name))

    def pptx(self):
        relationships = self.relationships("ppt/_rels/presentation.xml.rels", "ppt")
        slides = [node for node in self.read("ppt/presentation.xml").iter() if local(node.tag) == "sldId"]
        for number, slide in enumerate(slides, 1):
            relation = next((value for key, value in slide.attrib.items() if key.endswith("}id")), None)
            name = relationships.get(relation)
            if not name or not name.startswith("ppt/slides/"):
                raise ValueError("Missing/invalid slide relationship")
            yield f"slide {number} ({name})", paragraphs(self.read(name))
        for name in sorted(self.names):
            if re.fullmatch(r"ppt/notesSlides/notesSlide\d+\.xml", name):
                yield name, paragraphs(self.read(name))

    def odt(self):
        root = self.read("content.xml")
        if any(local(node.tag) in {"image", "object", "object-ole"} for node in root.iter()):
            self.partial = True
        lines = []
        for node in root.iter():
            if local(node.tag) in {"p", "h"}:
                lines.append(odt_text(node))
        yield "content.xml", "\n".join(lines)

    def xlsx(self):
        strings = []
        if "xl/sharedStrings.xml" in self.names:
            strings = [text_nodes(node) for node in self.read("xl/sharedStrings.xml") if local(node.tag) == "si"]
        relationships = self.relationships("xl/_rels/workbook.xml.rels", "xl")
        workbook = self.read("xl/workbook.xml")
        for sheet in workbook.iter():
            if local(sheet.tag) != "sheet":
                continue
            relation = next((value for key, value in sheet.attrib.items() if key.endswith("}id")), None)
            name = relationships.get(relation)
            if not name or not name.startswith("xl/worksheets/"):
                raise ValueError("Missing/invalid worksheet relationship")
            values = []
            for cell in self.read(name).iter():
                if local(cell.tag) != "c":
                    continue
                value, missing = cell_value(cell, strings)
                self.partial |= missing
                if value:
                    values.append(f"{cell.attrib.get('r', '?')}: {value}")
            yield f"sheet {sheet.attrib.get('name', '?')} ({name})", "\n".join(values)
        # Cached values are searchable; display styles and Excel serial dates
        # require the typed spreadsheet reader before claiming full fidelity.
        self.partial = True


def cell_value(cell, strings):
    kind = cell.attrib.get("t")
    value = next((node.text or "" for node in cell if local(node.tag) == "v"), "")
    if kind == "s":
        index = int(value)
        if not 0 <= index < len(strings):
            raise ValueError("Invalid shared-string index")
        value = strings[index]
    elif kind == "inlineStr":
        value = text_nodes(cell)
    elif kind == "b":
        value = {"0": "false", "1": "true"}.get(value, value)
    missing = any(local(node.tag) == "f" for node in cell) and not value
    return value, missing


def odt_text(node):
    text = node.text or ""
    for child in node:
        tag = local(child.tag)
        if tag == "s":
            count = next((value for key, value in child.attrib.items() if local(key) == "c"), "1")
            count = int(count)
            if not 0 <= count <= 4096:
                raise ValueError("ODT whitespace expansion budget exceeded")
            text += " " * count
        elif tag == "tab":
            text += "\t"
        elif tag == "line-break":
            text += "\n"
        else:
            text += odt_text(child)
        text += child.tail or ""
    return text


def extract(source, kind, cursor=0):
    if kind not in KINDS or cursor < 0:
        raise ValueError("Unsupported document kind/cursor")
    partial = False
    if kind in PLAIN or kind in {"html", "htm", "xhtml"}:
        data = source.read(TEXT_LIMIT + 1)
        partial = len(data) > TEXT_LIMIT
        text = decode(data[:TEXT_LIMIT], truncated=partial)
        if "\0" in text:
            raise ValueError("Binary content is not searchable text")
        if kind not in PLAIN:
            parser = HtmlText()
            parser.feed(text)
            parser.close()
            text = "".join(parser.parts)
            partial |= parser.partial
        parts = [("document text", text)]
    else:
        package = Package(source)
        parts = []
        count = 0
        for location, text in getattr(package, kind)():
            encoded = text.encode("utf-8")
            remaining = TEXT_LIMIT - count
            if len(encoded) > remaining:
                parts.append((location, encoded[:remaining].decode("utf-8", errors="ignore")))
                partial = True
                break
            parts.append((location, text))
            count += len(encoded)
            if len(parts) >= PART_LIMIT:
                partial = True
                break
        partial |= package.partial
    if not parts or cursor >= len(parts):
        raise ValueError("Document cursor is outside extracted coverage")
    location, text = parts[cursor]
    encoded = text.encode("utf-8")
    if len(encoded) > TEXT_LIMIT:
        text = encoded[:TEXT_LIMIT].decode("utf-8", errors="ignore")
        partial = True
    return {"text": text, "modality": "text", "data": None, "location": location,
            "next": cursor + 1 if cursor + 1 < len(parts) else None,
            "coverage": "partial: extraction/representation budget or unsupported visual/style content" if partial else "complete"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=sorted(KINDS), required=True)
    parser.add_argument("--cursor", type=int, default=0)
    parser.add_argument("source")
    args = parser.parse_args()
    memory = 768 * 1024 * 1024
    hard = resource.getrlimit(resource.RLIMIT_AS)[1]
    memory = min(memory, hard) if hard != resource.RLIM_INFINITY else memory
    resource.setrlimit(resource.RLIMIT_AS, (memory, memory))
    resource.setrlimit(resource.RLIMIT_CPU, (20, 25))
    try:
        if not re.fullmatch(r"/proc/self/fd/\d+", args.source):
            raise ValueError("Expected an inherited source descriptor")
        with open(args.source, "rb") as source:
            metadata = os.fstat(source.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > SOURCE_LIMIT:
                raise ValueError("Document exceeds ordinary-source budget")
            result = {"Ok": extract(source, args.kind, args.cursor)}
    except Exception as error:
        result = {"Err": str(error)[:1024]}
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
