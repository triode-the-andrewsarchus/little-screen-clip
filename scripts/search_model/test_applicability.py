"""Applicability preflight coverage, evidence and composition contracts."""
import unittest
from test_plans import Library
from test_runtime import load

answers = load('answers')


class Replies(Library):
    def __init__(self, responses):
        super().__init__(None)
        self.responses = list(responses)

    def clip_answer_generate(self, *args):
        self.response = self.responses.pop(0)
        return super().clip_answer_generate(*args)


class Applicability(unittest.TestCase):
    def setUp(self):
        answers._applicability['SUBJECTS'].clear()

    def request(self, **changes):
        return dict({'question':'Does this rental agreement permit pets?', 'type':'boolean',
            'complete':True, 'sources':[{'id':0, 'text':'Document: Recipe\nPets permitted: true\n'}]}, **changes)

    def assessment(self, applicability='not_applicable', subject='recipe'):
        return {'source_subject':subject, 'applicability':applicability,
            'reason':'The source subject determines whether the question applies.',
            'evidence':[{'source':0,'quote':'Document: Recipe'}]}

    def test_nonapplicable_and_unknown_stop_before_fact_extraction(self):
        for applicability, subject in [('not_applicable','recipe'), ('undetermined',None)]:
            answers._applicability['SUBJECTS'].clear()
            library = Replies([{'subject':'rental agreement'},self.assessment(applicability, subject)])
            result = answers.generate_reply(library, None, self.request(), 'qwen3.5-9b')['answer']
            self.assertEqual(result['applicability'], applicability)
            self.assertIsNone(result['value'])
            self.assertEqual(len(library.calls), 2)
            self.assertEqual(set(library.calls[0][1]), {'question'})
            self.assertNotIn('question',library.calls[1][1])
            self.assertNotIn('Pets permitted',str(library.calls[0][1]))

    def test_applicable_then_extracts_a_typed_false(self):
        value = {'reason':'The field explicitly denies permission.', 'applicability':'applicable',
            'state':'value', 'value':'false', 'unit':None,
            'evidence':[{'source':0,'quote':'Pets permitted: false'}]}
        request = self.request(sources=[{'id':0,'text':'Document: Rental agreement\nPets permitted: false\n'}])
        assessment = self.assessment('applicable','rental agreement')
        assessment['evidence'] = [{'source':0,'quote':'Document: Rental agreement'}]
        library = Replies([{'subject':'rental agreement'},assessment,value])
        result = answers.generate_reply(library,None,request,'qwen3.5-9b')['answer']
        self.assertEqual(result['value'],'false')
        self.assertEqual(len(library.calls),3)
        self.assertIn('required_subject',library.calls[1][1])

    def test_partial_or_selected_evidence_cannot_run_subject_assessment(self):
        for changes in [{'complete':False}, {'context':'evidence'}]:
            library = Library({})
            with self.assertRaises(ValueError):
                answers.applicability_reply(library,None,self.request(**changes),'qwen3.5-9b')
            self.assertEqual(library.calls,[])

    def test_only_question_subject_is_cached_and_every_file_gets_its_own_verdict(self):
        matching = self.assessment('applicable','rental agreement')
        matching['evidence'] = [{'source':0,'quote':'Document: Rental agreement'}]
        library = Replies([{'subject':'rental agreement'},matching,self.assessment()])
        first = answers.applicability_reply(library,None,self.request(sources=[{'id':0,'text':'Document: Rental agreement'}]),'qwen3.5-9b')
        second = answers.applicability_reply(library,None,self.request(),'qwen3.5-9b')
        self.assertEqual(first['applicability_result']['applicability'],'applicable')
        self.assertEqual(second['applicability_result']['applicability'],'not_applicable')
        self.assertEqual(len(library.calls),3)
        self.assertNotEqual(library.calls[1][1]['sources'],library.calls[2][1]['sources'])

    def test_question_only_cache_is_bounded(self):
        library = Library({'subject':None})
        for index in range(40):
            ready, subject = answers._applicability['question_subject'](library,None,f'Question {index}',answers.grammar)
            self.assertTrue(ready)
            self.assertIsNone(subject)
        self.assertEqual(len(answers._applicability['SUBJECTS']),32)

    def test_invented_evidence_unknown_subject_and_extra_actions_fail_closed(self):
        invalid = [dict(self.assessment(),source_subject=None),
            dict(self.assessment(),evidence=[{'source':0,'quote':'Document: Invoice'}]),
            dict(self.assessment(),command='run something')]
        for response in invalid:
            answers._applicability['SUBJECTS'].clear()
            with self.assertRaises(ValueError):
                answers.applicability_reply(Replies([{'subject':'rental agreement'},response]),None,self.request(),'qwen3.5-9b')

    def test_inference_budget_returns_unknown(self):
        for status in (1,4):
            result = answers.applicability_reply(Library({},status),None,self.request(),'qwen3.5-9b')['applicability_result']
            self.assertEqual(result['applicability'],'undetermined')
            self.assertEqual(result['state'],'unknown')


if __name__ == '__main__':
    unittest.main()
