"""Translate the pinned native ABI's compute failures into protocol categories."""
import re


class BackendFailure(ValueError):
    error_kind = 'backend'


def embedding_error(message):
    # These are fixed templates from the pinned C shim, never model output.
    # llama_process documents -1 as invalid input and <-1 as a fatal compute
    # error. Encoder failure occurs after media parsing/tokenization succeeded.
    status = re.fullmatch(r'llama_process failed \((-?\d+)\)', message)
    if (status and int(status[1]) < -1) or message.startswith('the encoder failed on '):
        raise BackendFailure(message)
    raise ValueError(message)


def generation_status(status):
    # answer_shim.c: 2 is native decode failure; context/output limits 1/4 and
    # invalid grammar 3 retain their existing request/abstention behavior.
    if status == 2:
        raise BackendFailure('Native answer decoding failed')
    return status


def error_reply(request_id, error):
    return {'id':request_id, 'ok':False, 'error':str(error)[:500],
            'kind':getattr(error, 'error_kind', 'request')}
