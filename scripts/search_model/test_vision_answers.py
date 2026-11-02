"""Visual evidence contract checks without model weights or image inference."""
import base64
import json
import tempfile
from pathlib import Path
import unittest
from test_runtime import load
from test_plans import Library

answers, visual = load('answers'), load('vision_answers')


class VisualAnswers(unittest.TestCase):
    def setUp(self):
        visual.SCOPES.clear()

    def test_source_planning_uses_only_question_and_bounds_its_cache(self):
        library = Library({'scope':'text'})
        question = 'What rating is in the extracted text?'
        self.assertEqual(visual.question_scope(library, None, question), 'text')
        self.assertEqual(visual.question_scope(library, None, question), 'text')
        self.assertEqual(len(library.calls), 1)
        self.assertEqual(library.calls[0][1], {'question':question})
        for index in range(33):
            visual.question_scope(library, None, str(index))
        self.assertEqual(len(visual.SCOPES), 32)
        self.assertNotIn(question, visual.SCOPES)

    def test_uncertain_source_plans_keep_both_channels_and_are_not_cached(self):
        for status in (1, 4):
            library = Library({}, status)
            self.assertEqual(visual.question_scope(library, None, 'Question?'), 'all')
            self.assertEqual(visual.SCOPES, {})
        for response in ({'scope':'shell'}, {'scope':'text','extra':True}, []):
            with self.subTest(response=response), self.assertRaises(ValueError):
                visual.question_scope(Library(response), None, 'Question?')
        with self.assertRaises(ValueError):
            visual.question_scope(Library({}, 2), None, 'Question?')

    def test_explicit_text_scope_needs_no_image_decoder_or_projector(self):
        question = 'What rating is in the extracted text?'
        visual.SCOPES[question] = 'text'
        reply = dict(self.reply(), value='14', unit='points',
                     evidence=[{'source':1,'quote':'Rating: 14 points.'}])
        library = Library(reply)
        request = {'question':question,'type':'number',
                   'sources':[{'id':1,'text':'Rating: 14 points.'}]}
        def no_media(*args):
            self.fail('Text-only question must not load image media')
        result = visual.generate(library, None, request, None, {}, answers.prepare,
                                answers.grammar, answers.validate, no_media, answers.SYSTEM)
        self.assertEqual(result['value'], '14')
        self.assertEqual(result['evidence'], reply['evidence'])
        self.assertIn('evidence ::= text-evidence', library.calls[0][2])
        library.response['evidence'][0]['quote'] = 'Rating: 18 points.'
        with self.assertRaises(ValueError):
            visual.generate(library, None, request, None, {}, answers.prepare,
                            answers.grammar, answers.validate, no_media, answers.SYSTEM)

    def test_mixed_prompt_validates_sources_before_loading_media_or_models(self):
        request = {'question':'What rating?', 'type':'number'}
        user, texts = visual.prepare_question(request, answers.prepare)
        self.assertEqual(texts, {})
        self.assertNotIn('extracted_text', json.loads(user))
        user, texts = visual.prepare_question(dict(request,sources=[{'id':1,'text':'Rating: 14','ignored':'not prompt data'}]), answers.prepare)
        self.assertEqual(texts, {1:'Rating: 14'})
        self.assertEqual(json.loads(user)['extracted_text'], [{'id':1,'text':'Rating: 14'}])
        for sources in [None, [{'id':0,'text':'Rating: 14'}], [{'id':65,'text':'Rating: 14'}],
                        [{'id':True,'text':'Rating: 14'}], [{'id':1,'text':'x'*32769}]]:
            with self.subTest(sources=sources), self.assertRaises(ValueError):
                visual.prepare_question(dict(request,sources=sources), answers.prepare)
        with self.assertRaises(ValueError):
            visual.prepare_question(dict(request,op='visual_evidence',sources=[{'id':1,'text':'Rating: 14'}]), answers.prepare)

    def reply(self):
        return {'reason':'A green rectangle is visible.', 'applicability':'applicable', 'state':'value',
                'value':'false', 'unit':None,
                'evidence':[{'source':0,'region':[10,20,900,800],'observation':'A green rectangle.'}]}

    def test_false_and_distinct_visual_provenance_are_preserved(self):
        result = visual.validate(self.reply(), 'boolean', answers.validate)
        self.assertEqual(result['value'], 'false')
        self.assertIn('observation', result['evidence'][0])
        self.assertNotIn('quote', result['evidence'][0])

    def test_mixed_text_quotes_keep_exact_sources_and_visual_provenance(self):
        reply = dict(self.reply(), state='ambiguous', value=None,
                     evidence=self.reply()['evidence']+[{'source':1,'quote':'Rating: 14'}])
        texts = {1:'Rating: 14 points.'}
        self.assertEqual(visual.validate(reply, 'number', answers.validate, texts)['evidence'], reply['evidence'])
        for item in [{'source':2,'quote':'Rating: 14'}, {'source':True,'quote':'Rating: 14'},
                     {'source':1,'quote':'Rating: 18'}, {'source':0,'quote':'Rating: 14'},
                     {'source':1,'quote':'Rating: 14','region':[0,0,1000,1000]}]:
            with self.subTest(item=item), self.assertRaises(ValueError):
                visual.validate(dict(reply,evidence=[item]), 'number', answers.validate, texts)
        syntax = visual.grammar(answers.grammar('number'), has_text=True)
        self.assertIn('evidence ::= visual-evidence | text-evidence', syntax)
        with self.assertRaises(ValueError):
            visual.grammar(answers.grammar('number'), selection=True, has_text=True)

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
