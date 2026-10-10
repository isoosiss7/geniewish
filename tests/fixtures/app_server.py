#!/usr/bin/env python3
"""Small protocol peer for testing failure and event-ordering behavior."""
import json
import os
from pathlib import Path
import sys

history = Path(os.environ['GENIEWISH_TEST_HISTORY'])
mode = os.environ.get('GENIEWISH_TEST_MODE', '')
state = json.loads(history.read_text()) if history.exists() else {'next': 1, 'threads': {}}
pending = {}


def save():
    history.write_text(json.dumps(state))


def send(message):
    print(json.dumps(message), flush=True)


def finish(thread, turn, text):
    item = {'type': 'agentMessage', 'id': turn + '-answer', 'text': text, 'phase': 'final_answer'}
    record = {'id': turn, 'status': 'completed', 'items': [item]}
    state['threads'][thread]['turns'].append(record)
    save()
    send({'method': 'item/completed', 'params': {'threadId': thread, 'turnId': turn, 'item': item}})
    send({'method': 'turn/completed', 'params': {'threadId': thread, 'turn': record}})


for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    params = request.get('params', {})
    identity = request.get('id')
    if method is None:
        thread, turn = pending.pop(identity)
        decision = request.get('result', {}).get('decision')
        text = 'approved' if decision == 'accept' else 'denied'
        if state['threads'][thread].get('steered'):
            text += ': ' + state['threads'][thread]['steered']
        finish(thread, turn, text)
        continue
    if identity is None:
        continue
    error = None
    result = {}
    if method == 'initialize':
        send({'id': 999999, 'result': {}})
        result = {'userAgent': 'fixture'}
    elif method == 'thread/start':
        assert params['sandbox'] == 'workspace-write'
        assert 'Human' in params['developerInstructions']
        thread = 'thread-' + str(state['next'])
        state['next'] += 1
        state['threads'][thread] = {'id': thread, 'turns': []}
        result = {'thread': state['threads'][thread]}
        save()
    elif method in ['thread/resume', 'thread/read']:
        thread = params['threadId']
        if thread not in state['threads'] or (mode == 'fail-resume' and method == 'thread/resume'):
            error = {'code': -32600, 'message': 'thread not found'}
        else:
            result = {'thread': state['threads'][thread]}
    elif method == 'thread/delete':
        if mode == 'fail-delete':
            error = {'code': -32000, 'message': 'deletion refused'}
        else:
            del state['threads'][params['threadId']]
            save()
    elif method == 'turn/steer':
        thread = params['threadId']
        turn = params['expectedTurnId']
        assert (thread, turn) in pending.values(), 'Steering must target the existing active turn.'
        state['threads'][thread]['steered'] = params['input'][0]['text']
        state.setdefault('steers', []).append(params)
        save()
        result = {'turnId': turn}
    elif method == 'turn/start':
        thread = params['threadId']
        turn = 'turn-' + str(len(state['threads'][thread]['turns']) + 1)
        text = params['input'][0]['text']
        result = {'turn': {'id': turn, 'items': [], 'status': 'inProgress'}}
        send({'method': 'turn/started', 'params': {'threadId': thread, 'turn': result['turn']}})
        if text == 'needs approval':
            approval = 'approval-' + thread
            pending[approval] = (thread, turn)
            send({'id': approval, 'method': 'item/commandExecution/requestApproval', 'params': {'threadId': thread, 'turnId': turn, 'itemId': 'command', 'command': 'echo approved'}})
        else:
            # Notifications intentionally precede the RPC response.
            send({'method': 'item/completed', 'params': {'threadId': thread, 'turnId': turn, 'item': {'type': 'agentMessage', 'id': turn + '-progress', 'text': 'intermediate progress', 'phase': 'commentary'}}})
            finish(thread, turn, 'fixture reply')
    else:
        error = {'code': -32601, 'message': 'unsupported fixture method'}
    send({'id': identity, 'error': error} if error else {'id': identity, 'result': result})
