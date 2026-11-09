"""Extraction contracts without model downloads or external resources."""
import codecs
import io
import unittest
from unittest.mock import patch
import zipfile

import documents


def package(parts):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, text in parts.items():
            archive.writestr(name, text)
    stream.seek(0)
    return stream


class DocumentsTests(unittest.TestCase):
    def test_utf8_and_bom_encodings_preserve_unicode(self):
        text = "Résumé 🦇\nLine two"
        for encoding in ["utf-8", "utf-8-sig", "utf-16", "utf-32"]:
            with self.subTest(encoding=encoding):
                result = documents.extract(io.BytesIO(text.encode(encoding)), "txt")
                self.assertEqual(result["text"], text)
                self.assertEqual(result["coverage"], "complete")
        with self.assertRaises(ValueError):
            documents.extract(io.BytesIO(b"binary\0text"), "txt")
        with self.assertRaises(UnicodeError):
            documents.extract(io.BytesIO(b"invalid \xff"), "txt")

    def test_html_preserves_words_and_excludes_executable_content(self):
        source = b'<html><style>hidden-css-token</style><script>fetch("https://invalid.test/secret")</script><p>finan<b>cial</b> &amp; taxes</p><p>Second paragraph</p></html>'
        result = documents.extract(io.BytesIO(source), "html")
        self.assertIn("financial & taxes\n", result["text"])
        self.assertNotIn("hidden-css-token", result["text"])
        self.assertNotIn("fetch", result["text"])
        self.assertEqual(result["coverage"], "complete")
        visual = documents.extract(io.BytesIO(b'<p>Text</p><img src="https://invalid.test/secret" alt="diagram">'), "html")
        self.assertIn("diagram", visual["text"])
        self.assertTrue(visual["coverage"].startswith("partial:"))

    def test_docx_preserves_run_boundaries_and_separates_headers(self):
        parts = {
            "word/document.xml": '<w:document xmlns:w="word"><w:body><w:p><w:r><w:t>finan</w:t></w:r><w:r><w:t>cial</w:t></w:r></w:p><w:p><w:r><w:t>Second line</w:t></w:r></w:p></w:body></w:document>',
            "word/header1.xml": '<w:hdr xmlns:w="word"><w:p><w:r><w:t>Report header</w:t></w:r></w:p></w:hdr>',
        }
        first = documents.extract(package(parts), "docx")
        second = documents.extract(package(parts), "docx", first["next"])
        self.assertEqual(first["text"], "financial\nSecond line")
        self.assertEqual(second["text"], "Report header")
        self.assertEqual(second["location"], "word/header1.xml")
        self.assertIsNone(second["next"])
        self.assertEqual(first["coverage"], "complete")

    def test_odt_spaces_tabs_and_newlines_are_not_lost(self):
        source = package({"content.xml": '<document xmlns:t="text"><t:p>foo<t:s t:c="3"/>bar<t:tab/>next<t:line-break/>line</t:p></document>'})
        self.assertEqual(documents.extract(source, "odt")["text"], "foo   bar\tnext\nline")

    def test_pptx_uses_presentation_order_instead_of_zip_order(self):
        source = package({
            "ppt/presentation.xml": '<p:presentation xmlns:p="ppt" xmlns:r="relations"><p:sldIdLst><p:sldId id="4" r:id="second"/><p:sldId id="5" r:id="first"/></p:sldIdLst></p:presentation>',
            "ppt/_rels/presentation.xml.rels": '<Relationships><Relationship Id="first" Target="slides/slide1.xml"/><Relationship Id="second" Target="slides/slide2.xml"/></Relationships>',
            "ppt/slides/slide1.xml": '<slide><p><t>Zip first</t></p></slide>',
            "ppt/slides/slide2.xml": '<slide><p><t>Presentation first</t></p></slide>',
        })
        result = documents.extract(source, "pptx")
        self.assertEqual(result["text"], "Presentation first")
        self.assertTrue(result["location"].startswith("slide 1"))

    def test_xlsx_reads_cached_values_and_discloses_style_limits(self):
        source = package({
            "xl/workbook.xml": '<workbook xmlns:r="relations"><sheets><sheet name="Data" r:id="data"/></sheets></workbook>',
            "xl/_rels/workbook.xml.rels": '<Relationships><Relationship Id="data" Target="worksheets/data.xml"/></Relationships>',
            "xl/sharedStrings.xml": '<sst><si><r><t>Balance</t></r></si></sst>',
            "xl/worksheets/data.xml": '<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1"><f>WEBSERVICE("https://invalid.test/secret")</f><v>42.5</v></c><c r="C1" t="b"><v>0</v></c><c r="D1" t="inlineStr"><is><t>Inline</t></is></c></row></sheetData></worksheet>',
        })
        result = documents.extract(source, "xlsx")
        self.assertEqual(result["text"], "A1: Balance\nB1: 42.5\nC1: false\nD1: Inline")
        self.assertNotIn("WEBSERVICE", result["text"])
        self.assertIn("Data", result["location"])
        self.assertTrue(result["coverage"].startswith("partial:"))

    def test_entities_escaping_paths_and_expansion_budgets_are_rejected(self):
        for data in [b'<!DOCTYPE x [<!ENTITY payload SYSTEM "file:///etc/passwd">]><x>&payload;</x>',
                     b'<x>' * 129 + b'</x>' * 129]:
            with self.assertRaises(ValueError):
                documents.xml(data)
        with self.assertRaises(ValueError):
            documents.extract(package({"../word/document.xml": "<x/>"}), "docx")
        with patch.object(documents, "XML_LIMIT", 32):
            with self.assertRaises(ValueError):
                documents.extract(package({"word/document.xml": "<x>" + "A" * 100 + "</x>"}), "docx")

    def test_output_truncation_marks_partial_and_preserves_utf8(self):
        with patch.object(documents, "TEXT_LIMIT", 5):
            result = documents.extract(io.BytesIO("ééé".encode()), "txt")
            self.assertEqual(result["text"], "éé")
            self.assertTrue(result["coverage"].startswith("partial:"))


if __name__ == "__main__":
    unittest.main()
