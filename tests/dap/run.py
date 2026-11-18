#!/usr/bin/env python3
"""End-to-end DAP sessions with real compiler DWARF for both PIC cores."""
import argparse
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]

class Client:
    def __init__(self, binary):
        self.p = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.seq = 0
        self.messages = queue.Queue()
        self.pending = []
        def reader():
            try:
                while True:
                    line = self.p.stdout.readline()
                    if not line:
                        raise EOFError('adapter closed stdout')
                    n = int(line.decode().split(':', 1)[1])
                    assert self.p.stdout.readline() == b'\r\n'
                    self.messages.put(json.loads(self.p.stdout.read(n)))
            except Exception as e:
                self.messages.put(e)
        threading.Thread(target=reader, daemon=True).start()
    def wait(self, predicate):
        for i, message in enumerate(self.pending):
            if predicate(message):
                return self.pending.pop(i)
        while True:
            m = self.messages.get(timeout=15)
            if isinstance(m, Exception):
                raise m
            if predicate(m):
                return m
            self.pending.append(m)
    def request(self, command, args=None, success=True):
        self.seq += 1
        b = json.dumps(dict(seq=self.seq, type='request', command=command, arguments=args or {})).encode()
        self.p.stdin.write(f'Content-Length: {len(b)}\r\n\r\n'.encode() + b)
        self.p.stdin.flush()
        reply = self.wait(lambda m: m.get('request_seq') == self.seq)
        assert reply['success'] == success, reply
        return reply.get('body', {})
    def event(self, event):
        return self.wait(lambda m: m.get('event') == event)['body']
    def close(self):
        try:
            if self.p.poll() is None:
                self.request('disconnect')
                self.p.wait(timeout=5)
            assert self.p.returncode == 0, self.p.stderr.read().decode()
        finally:
            if self.p.poll() is None:
                self.p.kill()
                self.p.wait()


def run(binary, basil, part):
    with tempfile.TemporaryDirectory(prefix='picsim-dap-') as directory:
        d = Path(directory)
        cfg = 'FEXTOSC = OFF, RSTOSC = HFINTOSC_32MHZ, WDTE = OFF, LVP = ON' if part.startswith('16') else 'FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON'
        text = f'''Program Dap
Device = {part}
Clock = {32 if part.startswith('16') else 64}
Config {cfg}
Dim value As Byte
Sub AddOne()
    value = value + 1
End Sub
value = 1
AddOne()
value = value + 1
'''
        src = d / 'dap.bas'
        src.write_text(text)
        subprocess.run([str(basil), '-g', '-O0', '--keep-vars', str(src), '-o', str(d/'dap.hex')], check=True, capture_output=True)
        c = Client(binary)
        try:
            c.request('threads', success=False)
            caps = c.request('initialize', {'adapterID':'picsim', 'linesStartAt1':True})
            assert caps['supportsConfigurationDoneRequest']
            c.request('launch', {'program':str(d/'dap.hex'), 'device':part})
            c.event('initialized')
            c.request('launch', {'program':str(d/'dap.hex'), 'device':part}, success=False)
            bps = c.request('setBreakpoints', {'source':{'path':str(src)}, 'breakpoints':[{'line':10},{'line':999}]})
            assert bps['breakpoints'][0]['verified'] and not bps['breakpoints'][1]['verified'], bps
            assert c.request('loadedSources')['sources']
            c.request('configurationDone')
            assert c.event('stopped')['reason'] == 'entry'
            c.request('continue', {'threadId':1})
            assert c.event('stopped')['reason'] == 'breakpoint'
            frames = c.request('stackTrace', {'threadId':1})['stackFrames']
            assert frames[0]['line'] == 10, frames
            call_address = frames[0]['instructionPointerReference']
            scopes = c.request('scopes', {'frameId':frames[0]['id']})['scopes']
            variables = c.request('variables', {'variablesReference':scopes[1]['variablesReference']})['variables']
            assert any(v['value'].split()[0]=='1' for v in variables), variables
            assert c.request('evaluate', {'expression':'value'})['result'].split()[0] == '1'
            c.request('evaluate', {'expression':'value + 1'}, success=False)
            c.request('stepIn', {'threadId':1})
            c.event('stopped')
            frames = c.request('stackTrace', {'threadId':1})['stackFrames']
            assert len(frames) >= 2, frames
            c.request('stepOut', {'threadId':1})
            c.event('stopped')
            assert c.request('evaluate', {'expression':'value'})['result'].split()[0]=='2'
            c.request('setBreakpoints', {'source':{'path':str(src)}, 'breakpoints':[]})
            c.request('continue', {'threadId':1})
            try:
                stopped = c.event('stopped')
            except queue.Empty:
                c.request('pause', {'threadId':1})
                raise AssertionError(c.request('stackTrace', {'threadId':1}))
            assert 'Halt' in stopped['description'], stopped
            assert c.request('evaluate', {'expression':'value'})['result'].split()[0]=='3'
            c.request('restart')
            assert c.event('stopped')['reason']=='entry'
            assert c.request('evaluate', {'expression':'cycles'})['result']=='0'
            bps = c.request('setInstructionBreakpoints', {'breakpoints':[{'instructionReference':call_address}]})
            assert bps['breakpoints'][0]['verified'], bps
            c.request('continue', {'threadId':1})
            assert c.event('stopped')['reason']=='breakpoint'
            c.request('next', {'threadId':1})
            assert c.event('stopped')['reason']=='step'
            assert c.request('evaluate', {'expression':'value'})['result'].split()[0]=='2'
            assert c.request('stackTrace', {'threadId':1})['totalFrames']==1
            c.request('next', {'threadId':1, 'granularity':'instruction'})
            c.event('stopped')
            c.request('setInstructionBreakpoints', {'breakpoints':[]})

        finally:
            c.close()
        # A program that never halts must still respond to pause and disconnect.
        text = text[:text.index('Dim value')] + 'While True\nWend\n'
        src.write_text(text)
        subprocess.run([str(basil), str(src), '-o', str(d/'loop.hex')], check=True, capture_output=True)
        c = Client(binary)
        try:
            c.request('initialize', {'adapterID':'picsim'})
            c.request('launch', {'program':str(d/'loop.hex'), 'device':part, 'stopOnEntry':False})
            c.event('initialized')
            c.request('configurationDone')
            c.request('pause', {'threadId':1})
            assert c.event('stopped')['reason']=='pause'
        finally:
            c.close()
        print(f'PASS DAP {part}: source breakpoints, frames, variables, stepping, halt, restart, pause')

if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--adapter', type=Path, default=ROOT/'target/debug/basil-sim-dap')
    ap.add_argument('--basil', type=Path, default=ROOT.parent/'Basic_Compiler/target/release/basil')
    a = ap.parse_args()
    for part in ['18F27Q10','16F17146']:
        run(a.adapter, a.basil, part)
