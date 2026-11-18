'use strict';
const vscode = require('vscode');
function activate(context) {
    context.subscriptions.push(vscode.debug.registerDebugAdapterDescriptorFactory('basil-sim', {
        createDebugAdapterDescriptor(session) {
            const executable = vscode.workspace.getConfiguration('basilSim', session.workspaceFolder?.uri)
                .get('adapterPath', 'basil-sim-dap');
            return new vscode.DebugAdapterExecutable(executable, []);
        }
    }));
}
module.exports = { activate };
