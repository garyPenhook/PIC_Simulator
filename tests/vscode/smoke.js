'use strict';
// Run inside VS Code with --extensionTestsPath pointing to this file.
const vscode = require('vscode');
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

exports.run = async function () {
    const root = path.resolve(__dirname, '../..');
    const folder = vscode.workspace.workspaceFolders[0];
    const directory = folder.uri.fsPath;
    await vscode.workspace.getConfiguration('basilSim', folder.uri).update(
        'adapterPath', path.join(root, 'target/release/basil-sim-dap'),
        vscode.ConfigurationTarget.Workspace);
    const events = [];
    const tracker = vscode.debug.registerDebugAdapterTrackerFactory('basil-sim', {
        createDebugAdapterTracker() {
            return { onDidSendMessage(m) { if (m.type === 'event') events.push(m); } };
        }
    });
    async function stopped() {
        const deadline = Date.now() + 15000;
        while (Date.now() < deadline) {
            const index = events.findIndex(m => m.event === 'stopped');
            if (index >= 0) return events.splice(index, 1)[0].body;
            await new Promise(resolve => setTimeout(resolve, 25));
        }
        throw new Error('Timed out waiting for stopped event');
    }
    try {
        for (const part of ['18F27Q10', '16F17146']) {
            events.length = 0;
            const source = path.join(directory, `${part}.bas`);
            const program = path.join(directory, `${part}.hex`);
            fs.writeFileSync(source, `Program Dap
Device = ${part}
Clock = ${part.startsWith('16') ? 32 : 64}
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_${part.startsWith('16') ? 32 : 64}MHZ, WDTE = OFF, LVP = ON
Dim value As Byte
Sub AddOne()
    value = value + 1
End Sub
value = 1
AddOne()
value = value + 1
`);
            execFileSync(path.resolve(root, '../Basic_Compiler/target/release/basil'),
                ['-g', '-O0', '--keep-vars', source, '-o', program]);
            const breakpoint = new vscode.SourceBreakpoint(new vscode.Location(
                vscode.Uri.file(source), new vscode.Position(9, 0)));
            vscode.debug.addBreakpoints([breakpoint]);
            assert(await vscode.debug.startDebugging(folder, {
                type: 'basil-sim', request: 'launch', name: `Smoke ${part}`,
                program, device: part, stopOnEntry: true
            }));
            assert.strictEqual((await stopped()).reason, 'entry');
            const session = vscode.debug.activeDebugSession;
            assert(session);
            await session.customRequest('continue', { threadId: 1 });
            assert.strictEqual((await stopped()).reason, 'breakpoint');
            let frames = (await session.customRequest('stackTrace', { threadId: 1 })).stackFrames;
            assert.strictEqual(frames[0].line, 10);
            const scopes = (await session.customRequest('scopes', { frameId: frames[0].id })).scopes;
            const variables = await session.customRequest('variables', {
                variablesReference: scopes[1].variablesReference
            });
            assert(variables.variables.some(v => v.value.split(' ')[0] === '1'));
            await session.customRequest('stepIn', { threadId: 1 });
            await stopped();
            frames = (await session.customRequest('stackTrace', { threadId: 1 })).stackFrames;
            assert(frames.length >= 2);
            await session.customRequest('stepOut', { threadId: 1 });
            await stopped();
            assert.strictEqual((await session.customRequest('evaluate', { expression: 'value' })).result.split(' ')[0], '2');
            await session.customRequest('restart');
            assert.strictEqual((await stopped()).reason, 'entry');
            await session.customRequest('next', { threadId: 1 });
            assert.strictEqual((await stopped()).reason, 'step');
            const ended = new Promise(resolve => {
                const subscription = vscode.debug.onDidTerminateDebugSession(s => {
                    if (s.id === session.id) { subscription.dispose(); resolve(); }
                });
            });
            await vscode.debug.stopDebugging(session);
            await Promise.race([ended, new Promise((_, reject) => setTimeout(() => reject(new Error('Disconnect timed out')), 15000))]);
            vscode.debug.removeBreakpoints([breakpoint]);
            console.log(`PASS VS Code host ${part}: launch, breakpoint, stack, variables, stepping, restart, disconnect`);
            fs.appendFileSync(path.join(directory, 'smoke-results.log'), `PASS ${part}\n`);
        }
    } finally {
        tracker.dispose();
        await vscode.debug.stopDebugging();
    }
};
