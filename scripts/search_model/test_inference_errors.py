"""Pinned native failure categories; no model, network or GPU allocation."""
import unittest
from test_runtime import load

errors, worker = load('inference_errors'), load('worker')


class NativeFailureProtocol(unittest.TestCase):
    def test_embedding_native_compute_failures_are_distinct_from_invalid_inputs(self):
        for message, expected in [('llama_process failed (-2)','backend'),
                                  ('llama_process failed (-3)','backend'),
                                  ('the encoder failed on /owned/input.png','backend'),
                                  ('llama_process failed (-1)','request'),
                                  ('text exceeds context (4096 tokens, limit 2048)','request'),
                                  ('out of memory is user text','request')]:
            with self.subTest(message=message):
                with self.assertRaises(ValueError) as failure:
                    errors.embedding_error(message)
                reply = errors.error_reply(17, failure.exception)
                self.assertEqual(reply['id'],17)
                self.assertEqual(reply['kind'],expected)
                self.assertFalse(reply['ok'])

    def test_answer_context_grammar_and_output_statuses_do_not_request_backend_replay(self):
        for status in [0,1,3,4]:
            self.assertEqual(errors.generation_status(status),status)
        with self.assertRaises(ValueError) as failure:
            errors.generation_status(2)
        self.assertEqual(errors.error_reply(1,failure.exception)['kind'],'backend')

    def test_actual_embedding_encoder_routes_native_result_through_protocol_category(self):
        class Library:
            message = b'llama_process failed (-2)'
            def clip_search_token_count(self, model, text): return 8
            def clip_search_embed_texts(self, model, pointers, count, output, error, size):
                error.value = self.message
                return 1
        library = Library()
        for message, expected in [(b'llama_process failed (-2)','backend'),(b'batch full','request')]:
            library.message = message
            with self.assertRaises(ValueError) as failure:
                worker.encode(library,None,{'op':'text','inputs':['owned']})
            self.assertEqual(errors.error_reply(4,failure.exception)['kind'],expected)


if __name__=='__main__':
    unittest.main()
