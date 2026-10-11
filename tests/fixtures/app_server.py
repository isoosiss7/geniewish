#!/usr/bin/env python3
"""Small protocol peer for testing failure and event-ordering behavior."""
import json
import os
from pathlib import Path
import sys
import re
import socket
import threading
import time

history = Path(os.environ['GENIEWISH_TEST_HISTORY'])
mode = os.environ.get('GENIEWISH_TEST_MODE', '')
state = json.loads(history.read_text()) if history.exists() else {'next': 1, 'threads': {}}
pending = {}
output_lock = threading.Lock()
state_lock = threading.RLock()

# Canned decisions exercise dispatch without model usage. This is not a production interpreter.
def interpret(snapshot):
    original = snapshot['input']
    body = original.strip().rstrip('.?!')
    body = re.sub(r'^(hi|hello|hey) ', '', body, flags=re.I)
    target = None
    if original in snapshot['catalog'] and 'which name' in (snapshot.get('last_reply') or {}).get('text','').lower(): return {'action':'message','target':'Mike','name':None,'role':'','text':original}
    if body=='hire Annie then ask her to reply': return {'action':'message','target':'Mike','name':None,'role':'','text':original}
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


def route(snapshot):
    decision=interpret(snapshot)
    if decision['action'] in ['hire','set_role','retire','confirm_retire','cancel_retirement','roster','inbox','read']:
        decision={'action':'message','target':snapshot.get('manager') or decision.get('target') or 'Mike','text':snapshot['input']}
    return {key:decision[key] for key in ['action','target','text']}

def save():
    with state_lock: history.write_text(json.dumps(state))


def send(message):
    with output_lock: print(json.dumps(message), flush=True)


def finish(thread, turn, text):
    item = {'type': 'agentMessage', 'id': turn + '-answer', 'text': text, 'phase': 'final_answer'}
    record = {'id': turn, 'status': 'completed', 'items': [item]}
    state['threads'][thread]['turns'].append(record)
    save()
    send({'method': 'item/completed', 'params': {'threadId': thread, 'turnId': turn, 'item': item}})
    send({'method': 'turn/completed', 'params': {'threadId': thread, 'turn': record}})


def hr_call(thread, turn, text):
    if text=='delayed hire': time.sleep(.25)
    record = state['threads'][thread]
    setup = record['hr_config']['args']
    serial = 0
    def tool(action, name=None, role='', text=''):
        nonlocal serial
        serial += 1
        request = {'owner':int(setup[2]),'id':thread+'-'+turn+'-'+str(serial), 'tool':'manage_genies', 'arguments':{'action':action,'name':name,'role':role,'text':text}}
        if mode == 'wrong-tool-owner': request['owner'] += 1
        if mode != 'unassociated-tool': send({'method':'item/started','params':{'threadId':thread,'turnId':turn,'item':{'id':request['id'],'type':'mcpToolCall','server':'geniewish_hr','tool':request['tool'],'arguments':request['arguments']}}})
        with socket.socket(socket.AF_UNIX) as peer:
            peer.connect(setup[1]); peer.sendall((json.dumps(request)+'\n').encode())
            result=json.loads(peer.makefile().readline())
        if mode == 'replay-tool':
            with socket.socket(socket.AF_UNIX) as peer:
                peer.connect(setup[1]);peer.sendall((json.dumps(request)+'\n').encode())
                assert json.loads(peer.makefile().readline())==result
        state.setdefault('tool_results',[]).append(result); save()
        content=result['content'][0]['text']
        return result, json.loads(content) if not result['isError'] else content
    catalog = ['Mike','Lucas','Annie','Ben','Clara','Allan','Oscar','Ruby','Zoe','Hugo']
    decision = interpret({'input':text,'catalog':catalog})
    if text in ['Ruby','Zoe']:
        result,data=tool('hire',text);answer='Hired '+text+'.'
    elif text=='delayed hire':
        result,data=tool('hire');answer=data if result['isError'] else 'Hired a genie.'
    elif text in ['yes', 'Yes, do not retire Annie']:
        result,data=tool('confirm_retire','Annie');answer=data
    elif text == 'hire Annie then ask her to reply':
        result,data=tool('hire','Annie')
        if not result['isError']: result,data=tool('message','Annie',text='please reply')
        answer = data if result['isError'] else 'Delegated to Annie.'
    elif text == 'try retiring without confirmation':
        result,data=tool('confirm_retire','Annie');answer=data
    elif text == 'ask me which genie to hire':
        answer='Human, which name would you like?'
    elif text == 'hire two genies':
        result,data=tool('hire'); result,data=tool('hire');answer='Hired two genies.'
    elif decision['action'] in ['hire','set_role','retire','confirm_retire','inbox','read','roster']:
        result,data=tool(decision['action'],decision['name'],decision['role'])
        if result['isError']: answer=data
        elif decision['action']=='hire': answer='We hired '+data['result']['hired']+', Human.'
        elif decision['action']=='retire': answer=data['result']['confirmation_required']
        elif decision['action']=='confirm_retire': answer=data['result']['retired']+' is retired.'
        elif decision['action']=='inbox': answer='You have '+str(data['team']['unread'])+' unread updates.'
        elif decision['action']=='read': answer=data['result']['message']
        else: answer='Updated the team, Human.'
    else: answer='Mike here, Human.'
    finish(thread,turn,answer)

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
    after_response = None
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
            manager = 'mcp_servers.geniewish_hr' in params['config']
            assert params['sandbox'] == ('read-only' if manager else 'workspace-write')
            if manager:
                assert params['config']['features.shell_tool'] is False
                assert params['config']['mcp_servers.geniewish_hr']['required'] is True
            assert 'Human' in params['developerInstructions']
            thread = 'thread-' + str(state['next'])
            state['next'] += 1
        state['threads'][thread] = {'id': thread, 'turns': [], 'ephemeral':routing}
        if not routing and manager: state['threads'][thread]['hr_config']=params['config']['mcp_servers.geniewish_hr']
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
            if method=='thread/resume' and 'mcp_servers.geniewish_hr' in params.get('config',{}):
                state['threads'][thread]['hr_config']=params['config']['mcp_servers.geniewish_hr']
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
            finish(thread, turn, json.dumps(route(snapshot)))
        elif state['threads'][thread].get('hr_config'):
            after_response = lambda:hr_call(thread,turn,text)
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

    if after_response: threading.Thread(target=after_response,daemon=True).start()
