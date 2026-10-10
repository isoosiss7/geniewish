#!/usr/bin/env python3
"""Small protocol peer for testing failure and event-ordering behavior."""
import json
import os
from pathlib import Path
import sys
import re

history = Path(os.environ['GENIEWISH_TEST_HISTORY'])
mode = os.environ.get('GENIEWISH_TEST_MODE', '')
state = json.loads(history.read_text()) if history.exists() else {'next': 1, 'threads': {}}
pending = {}

# Canned decisions exercise dispatch without model usage. This is not a production interpreter.
def interpret(snapshot):
    original = snapshot['input']
    body = original.strip().rstrip('.?!')
    body = re.sub(r'^(hi|hello|hey) ', '', body, flags=re.I)
    target = None
    for prefix in ['i want to speak to ', 'i want to talk to ', 'talk to ', 'speak to ', 'switch to ', 'back to ']:
        if body.lower().startswith(prefix):
            body = body[len(prefix):]
            break
    for name in snapshot['catalog']:
        if body.lower() == name.lower():
            return {'action':'switch','target':name,'name':None,'role':'','text':''}
        if body.lower().startswith(name.lower()+','):
            target = name
            body = body[len(name)+1:].strip()
            break
    body = re.sub(r'^(please|pls) ', '', body, flags=re.I)
    lower = body.lower()
    action, chosen, role, text = 'message', None, '', body
    if lower.startswith('hire '):
        action, text = 'hire', ''
        parts = body[5:].split(' as ', 1)
        role = parts[1].removeprefix('my ').strip() if len(parts)>1 else ''
        candidate = parts[0].removesuffix(' for me').strip()
        chosen = None if candidate.lower() in ['a genie','a new genie','new genie'] else candidate
    elif lower.startswith(('set ', 'change ')) and "'s role to " in lower:
        action, text = 'set_role', ''
        candidate = body.split(' ',1)[1]
        chosen, role = candidate.split("'s role to ",1)
        role = role.removeprefix('my ')
    elif lower.startswith('clear ') and lower.endswith("'s role"):
        action, chosen, text = 'set_role', body[6:-7], ''
    elif lower.startswith('yes, retire '): action, chosen, text = 'confirm_retire', body[12:], ''
    elif lower.startswith('retire '): action, chosen, text = 'retire', body[7:], ''
    elif lower in ['any messages','do i have any messages','check messages']: action, text = 'inbox', ''
    elif lower.startswith('read ') and lower.endswith("'s message"): action, chosen, text = 'read', body[5:-10], ''
    elif lower == 'approve once': action, text = 'approve', ''
    elif lower == 'deny': action, text = 'deny', ''
    elif lower.startswith('unrouteable'): action, target, text = 'switch', 'InventedName', ''
    return {'action':action,'target':target,'name':chosen,'role':role,'text':text}


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
    elif method == 'config/read':
        result = {'config': {'mcp_servers':{'sample-server':{'command':'fixture'}},'plugins':{'sample-plugin@registry':{'enabled':True}}}}
    elif method == 'thread/start':
        routing = params.get('ephemeral', False)
        if routing:
            assert params['sandbox'] == 'read-only'
            assert params['config']['features.shell_tool'] is False
            assert params['config']['mcp_servers.sample-server.enabled'] is False
            assert params['config']['plugins.sample-plugin@registry.enabled'] is False
            state['routing_next'] = state.get('routing_next', 0) + 1
            thread = 'router-' + str(state['routing_next'])
        else:
            assert params['sandbox'] == 'workspace-write'
            assert 'Human' in params['developerInstructions']
            thread = 'thread-' + str(state['next'])
            state['next'] += 1
        state['threads'][thread] = {'id': thread, 'turns': [], 'ephemeral':routing}
        result = {'thread': state['threads'][thread]}
        save()
    elif method == 'thread/unsubscribe':
        del state['threads'][params['threadId']]
        save()
        result = {'status':'unsubscribed'}
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
        state['threads'][thread]['steered'] = params['input'][-1]['text']
        state.setdefault('steers', []).append(params)
        save()
        result = {'turnId': turn}
    elif method == 'turn/start':
        thread = params['threadId']
        turn = 'turn-' + str(len(state['threads'][thread]['turns']) + 1)
        text = params['input'][-1]['text']
        routing = state['threads'][thread].get('ephemeral',False)
        if not routing: state.setdefault('inputs', []).append(params)
        save()
        result = {'turn': {'id': turn, 'items': [], 'status': 'inProgress'}}
        send({'method': 'turn/started', 'params': {'threadId': thread, 'turn': result['turn']}})
        if routing:
            assert params['outputSchema']['additionalProperties'] is False
            snapshot = json.loads(text)
            state.setdefault('routing_inputs',[]).append(snapshot)
            finish(thread, turn, json.dumps(interpret(snapshot)))
        elif text == 'needs approval':
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
