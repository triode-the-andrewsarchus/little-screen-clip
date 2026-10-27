"""Visual evidence contract checks without model weights or image inference."""
import base64
import tempfile
from pathlib import Path
import unittest
from test_runtime import load

answers, visual = load('answers'), load('vision_answers')


class VisualAnswers(unittest.TestCase):
    def reply(self):
        return {'reason':'A green rectangle is visible.', 'applicability':'applicable', 'state':'value',
                'value':'false', 'unit':None,
                'evidence':[{'source':0,'region':[10,20,900,800],'observation':'A green rectangle.'}]}

    def test_false_and_distinct_visual_provenance_are_preserved(self):
        result = visual.validate(self.reply(), 'boolean', answers.validate)
        self.assertEqual(result['value'], 'false')
        self.assertIn('observation', result['evidence'][0])
        self.assertNotIn('quote', result['evidence'][0])

    def test_unknown_source_invalid_regions_and_untyped_values_are_rejected(self):
        for changed in [{'source':1}, {'source':True}, {'region':[900,0,800,1000]},
                        {'region':[0,0,1001,1000]}, {'region':[0,False,1000,1000]},
                        {'observation':''}, {'quote':'This is not a verified quotation'}]:
            reply = self.reply()
            reply['evidence'][0].update(changed)
            with self.assertRaises(ValueError):
                visual.validate(reply, 'boolean', answers.validate)
        for value in ('yes', '0', 'False'):
            reply = self.reply()
            reply['value'] = value
            with self.assertRaises(ValueError):
                visual.validate(reply, 'boolean', answers.validate)

    def test_nonapplicability_requires_visible_support_and_null_value(self):
        reply = self.reply()
        reply.update(applicability='not_applicable',state='not_applicable',value=None)
        self.assertEqual(visual.validate(reply, 'date', answers.validate)['state'], 'not_applicable')
        reply['evidence'] = []
        with self.assertRaises(ValueError):
            visual.validate(reply, 'date', answers.validate)

    def test_native_limits_abstain_and_decoder_failures_remain_errors(self):
        worker = load('worker')
        # Native generation is mocked: this header exercises the shared prepared
        # input bounds without requiring a decoder, projector or downloaded model.
        png = b'\x89PNG\r\n\x1a\n' + (13).to_bytes(4,'big') + b'IHDR' + (32).to_bytes(4,'big') * 2
        request = {'question':'Is it blue?', 'type':'boolean', 'data':base64.b64encode(png).decode()}
        class Library:
            status = 1
            def clip_answer_load_vision(self, model, path):
                return 0
            def clip_answer_generate_image(self, *args):
                return self.status
        library = Library()
        with tempfile.TemporaryDirectory() as directory:
            models = Path(directory)
            (models / 'projection.gguf').write_bytes(b'fixture')
            def generate():
                return visual.generate(library, None, request, models,
                    {'vision_file':['projection.gguf']}, answers.prepare,
                    answers.grammar, answers.validate, worker.validate_media)
            for status in (1, 4):
                library.status = status
                result = generate()
                self.assertEqual(result['state'], 'unknown')
                self.assertIsNone(result['value'])
            library.status = 2
            with self.assertRaises(ValueError):
                generate()

    def test_page_selection_validates_regions_and_marks_saturation(self):
        item = self.reply()['evidence'][0]
        reply = {'evidence':[item], 'limited':False}
        self.assertFalse(visual.validate_selection(reply)['limited'])
        self.assertTrue(visual.validate_selection(dict(reply, evidence=[item]*4))['limited'])
        for changed in [{'limited':None}, {'evidence':'invalid'}, {'evidence':[item]*5}, {'extra':1}]:
            with self.assertRaises(ValueError):
                visual.validate_selection(dict(reply, **changed))
        grammar = visual.grammar(answers.grammar('number'), True)
        self.assertEqual(sum(line.startswith('root ::=') for line in grammar.splitlines()), 1)
        self.assertIn('limited', grammar)
        self.assertIn('observation', grammar)

    def test_grammar_retains_typed_values_but_requires_regions_and_observations(self):
        grammar = visual.grammar(answers.grammar('number'))
        evidence = next(line for line in grammar.splitlines() if line.startswith('evidence ::='))
        self.assertIn('region', evidence)
        self.assertIn('observation', evidence)
        self.assertNotIn('quote', evidence)
        self.assertIn('typed-value ::=', grammar)


if __name__ == '__main__':
    unittest.main()
