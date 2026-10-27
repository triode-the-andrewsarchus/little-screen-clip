"""Semantic candidate review contracts, independent of downloaded weights."""
import json
import unittest
from test_runtime import load
from test_plans import Library

answers = load('answers')


class Reviews(unittest.TestCase):
    def request(self, **changes):
        return dict({'question':'What is the publication date?', 'type':'date',
                     'sources':[{'id':0, 'text':'date: 2021-11-18\ndraft: false\n'}],
                     'complete':False, 'may_reuse':True,
                     'candidate':{'path':['date'], 'value':'2021-11-18'}}, **changes)

    def test_review_sees_actual_candidate_and_evidence_but_not_document_coverage(self):
        result = {'scope':'literal', 'reason':'The candidate matches the date field.', 'decision':'reuse'}
        library = Library(result)
        self.assertEqual(answers.review_reply(library, None, self.request()), {'review':result})
        _, user, _ = library.calls[0]
        self.assertEqual(user['candidate']['value'], '2021-11-18')
        self.assertIn('draft: false', user['sources'][0]['text'])
        self.assertFalse(user['complete'])
        self.assertIn('body not examined', user['extraction_coverage'])
        self.assertLess(list(user).index('candidate'), list(user).index('question'))

    def test_per_file_review_cannot_authorize_reuse_and_unknown_actions_fail(self):
        for decision in ['reuse', 'execute', 'not_applicable']:
            with self.assertRaises(ValueError):
                answers.review_reply(Library({'scope':'literal', 'reason':'Looks fine', 'decision':decision}), None,
                                     self.request(may_reuse=False))
        for decision in ['accept', 'escalate']:
            library = Library({'scope':'literal', 'reason':'A file-specific judgment is needed.', 'decision':decision})
            self.assertEqual(answers.review_reply(library, None, self.request(may_reuse=False))['review']['decision'], decision)
            self.assertNotIn(json.dumps(json.dumps('reuse')), library.calls[0][2])

    def test_review_cannot_open_paths_change_values_or_claim_full_document(self):
        for changes in [{'complete':True}, {'may_reuse':1},
                        {'candidate':{'path':['date'], 'value':'2021-11-18', 'file':'/etc/passwd'}},
                        {'candidate':{'path':['date'], 'value':'x\0y'}}]:
            library = Library({})
            with self.assertRaises(ValueError):
                answers.review_reply(library, None, self.request(**changes))
            self.assertEqual(library.calls, [])
        for result in [{'scope':'literal', 'reason':'Fine', 'decision':'reuse', 'value':'2022-01-01'},
                       {'scope':'literal', 'reason':'', 'decision':'accept'}]:
            with self.assertRaises(ValueError):
                answers.review_reply(Library(result), None, self.request())

    def test_prior_review_feedback_stays_outside_evidence_and_is_bounded(self):
        request = {'question':'What is the age?', 'type':'number', 'complete':True,
                   'sources':[{'id':0,'text':'Current age: 6 years.'}],
                   'review_feedback':'The imported header age looked implausible.'}
        user, texts = answers.prepare(request)
        self.assertNotIn('implausible', texts[0])
        self.assertEqual(json.loads(user)['review_feedback'], request['review_feedback'])
        self.assertLess(list(json.loads(user)).index('review_feedback'), list(json.loads(user)).index('question'))
        for value in [True, 'x' * 1025, 'a\0b']:
            with self.assertRaises(ValueError):
                answers.prepare(dict(request, review_feedback=value))

    def test_generation_limits_escalate_and_never_approve(self):
        for status in [1, 4]:
            result = answers.review_reply(Library({}, status), None, self.request())['review']
            self.assertEqual(result['decision'], 'escalate')


if __name__ == '__main__':
    unittest.main()
