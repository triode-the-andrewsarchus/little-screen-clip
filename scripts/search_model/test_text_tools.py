"""Question-only search planning and per-file sufficiency-review contracts."""
import unittest
from test_plans import Library
from test_runtime import load

answers = load('answers')


class PlanningLibrary(Library):
    def clip_answer_generate(self, model, system, user, grammar, output, length):
        import json
        response = self.response
        if set(json.loads(user)) == {'question'}:
            self.response = {'subject':None}
        try:
            return super().clip_answer_generate(model,system,user,grammar,output,length)
        finally:
            self.response = response


class TextTools(unittest.TestCase):
    def setUp(self):
        answers.SEARCH_PLANS.clear()
        answers._applicability['SUBJECTS'].clear()

    def request(self, **changes):
        return dict({'question':'What is the redemption code?', 'type':'text'}, **changes)

    def query(self):
        return {'mode':'regex','pattern':r'(?i)(?:redemption|coupon)\s+code'}

    def review_request(self, **changes):
        return dict(self.request(), sources=[{'id':0,'text':'Redemption code: ALPHA-2049.'}],
                    complete=False, value='ALPHA-2049', search={'queries':[self.query()]}, **changes)

    def test_plans_receive_no_file_content_and_preserve_regex_escapes(self):
        library = PlanningLibrary({'queries':[self.query()]})
        request = self.request(sources=[{'id':0,'text':'private source payload'}])
        result = answers.search_plan_reply(library,None,request,'qwen3.5-9b')['search_plan']
        self.assertEqual(result,{'queries':[self.query()],'before':256,'after':512})
        self.assertEqual(set(library.calls[-1][1]), {'question','type'})
        self.assertEqual(set(library.calls[0][1]), {'question'})
        self.assertNotIn('private source payload',str(library.calls))

    def test_question_plans_are_bounded_cached_and_typed_but_file_verdicts_are_not(self):
        library = PlanningLibrary({'queries':None})
        for index in range(40):
            answers.search_plan_reply(library,None,self.request(question=f'Question {index}'),'qwen3.5-9b')
        self.assertEqual(len(answers.SEARCH_PLANS),32)
        answers.search_plan_reply(library,None,self.request(question='Question 39'),'qwen3.5-9b')
        self.assertEqual(len(library.calls),80)
        answers.search_plan_reply(library,None,self.request(question='Question 39',type='number'),'qwen3.5-9b')
        self.assertEqual(len(library.calls),81)
        reviewer = Library({'sufficient':False,'reason':'The question requires omitted context.'})
        for _ in range(2):
            answers.search_review_reply(reviewer,None,self.review_request())
        self.assertEqual(len(reviewer.calls),2)

    def test_extra_operations_and_unbounded_patterns_are_rejected(self):
        for response in [
            {'queries':[]}, {'queries':[self.query()]*4}, {'queries':[], 'command':'bash'},
            {'queries':[{'mode':'shell','pattern':'echo text'}]},
            {'queries':[dict(self.query(),path='/etc/passwd')]},
            {'queries':[{'mode':'literal','pattern':'x'*513}]},
            {'queries':[{'mode':'literal','pattern':'\0'}]},
        ]:
            with self.assertRaises(ValueError):
                answers.search_plan_reply(PlanningLibrary(response),None,self.request(),'qwen3.5-9b')

    def test_application_field_seed_adds_literal_coverage_without_model_format_assumptions(self):
        library = PlanningLibrary({'queries':[self.query()]})
        request = self.request(literal_field='redemption code')
        plan = answers.search_plan_reply(library,None,request,'qwen3.5-9b')['search_plan']
        self.assertEqual(plan['queries'][0],{'mode':'literal','pattern':'redemption code'})
        self.assertEqual(len(plan['queries']),2)
        with self.assertRaises(ValueError):
            answers.search_plan_reply(library,None,self.request(literal_field='/etc/passwd'),'qwen3.5-9b')

    def test_restricted_subject_declines_partial_search_without_reading_any_file(self):
        library = Library({'subject':'bank statement'})
        result = answers.search_plan_reply(library,None,self.request(question='What is the closing balance on this bank statement?'),'qwen3.5-9b')
        self.assertIsNone(result['search_plan'])
        self.assertEqual(len(library.calls),1)

    def test_refusal_budget_and_unsupported_provider_decline_search(self):
        for status in (1,4):
            library = PlanningLibrary({},status)
            self.assertIsNone(answers.search_plan_reply(library,None,self.request(),'qwen3.5-9b')['search_plan'])
            self.assertEqual(len(answers.SEARCH_PLANS),0)
        self.assertIsNone(answers.search_plan_reply(Library({}),None,self.request(),'nuextract3')['search_plan'])

    def test_sufficiency_review_declares_partial_coverage_and_checks_each_candidate(self):
        library = Library({'sufficient':True,'reason':'The code is directly stated.'})
        result = answers.search_review_reply(library,None,self.review_request())
        self.assertTrue(result['search_review']['sufficient'])
        system, data, _ = library.calls[0]
        self.assertIn('NOT supplied in full',system)
        self.assertFalse(data['complete'])
        self.assertEqual(data['candidate_value'],'ALPHA-2049')
        self.assertEqual(data['search_queries'],[self.query()])
        request = self.review_request()
        request['complete'] = True
        with self.assertRaises(ValueError):
            answers.search_review_reply(Library({}),None,request)
        request['complete'] = False
        request.pop('search')
        with self.assertRaises(ValueError):
            answers.search_review_reply(Library({}),None,request)

    def test_invalid_or_budget_exhausted_review_cannot_authorize_answering(self):
        for response in [{'sufficient':1,'reason':'No'}, {'sufficient':True,'reason':''},
                         {'sufficient':True,'reason':'OK','command':'true'}]:
            with self.assertRaises(ValueError):
                answers.search_review_reply(Library(response),None,self.review_request())
        result = answers.search_review_reply(Library({},4),None,self.review_request())
        self.assertFalse(result['search_review']['sufficient'])


if __name__ == '__main__':
    unittest.main()
