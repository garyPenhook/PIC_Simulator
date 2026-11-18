# VS Code host smoke test

Build the release workspace and the sibling `Basic_Compiler` release compiler.
With VS Code installed and a graphical display available, run from the simulator
repository (use a new temporary directory for each run):

```sh
smoke_dir=$(mktemp -d /tmp/picsim-vscode.XXXXXX)
mkdir -p "$smoke_dir/workspace"
code --new-window --disable-extensions --disable-workspace-trust \
  --skip-welcome --skip-release-notes \
  --user-data-dir "$smoke_dir/profile" \
  --extensions-dir "$smoke_dir/extensions" \
  --extensionDevelopmentPath="$PWD/tools/vscode-picsim" \
  --extensionTestsPath="$PWD/tests/vscode/smoke.js" \
  "$smoke_dir/workspace"
```

The editor closes when the test completes. Check `workspace/smoke-results.log`
under the temporary directory for `PASS 18F27Q10` and `PASS 16F17146`.
Failures appear in the isolated profile's `logs/` directory. The test compiles
real DWARF firmware and checks automatic extension activation, editor-managed
launch and source breakpoints, stack frames, variables, source stepping,
restart and disconnect. It uses VS Code APIs in a real desktop extension host;
it does not inspect the rendered panels or simulate mouse/keyboard input.
