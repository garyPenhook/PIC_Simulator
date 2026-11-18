#!/usr/bin/env python3
"""Package the dependency-free local VS Code debugger extension as a VSIX."""
import json
from pathlib import Path
import zipfile
from xml.sax.saxutils import escape
root = Path(__file__).resolve().parents[1]
source = root / 'tools/vscode-picsim'
pkg = json.loads((source/'package.json').read_text())
output = root / 'target' / f"{pkg['name']}-{pkg['version']}.vsix"
output.parent.mkdir(exist_ok=True)
manifest = f'''<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
<Metadata><Identity Language="en-US" Id="{escape(pkg['name'])}" Version="{escape(pkg['version'])}" Publisher="{escape(pkg['publisher'])}"/>
<DisplayName>{escape(pkg['displayName'])}</DisplayName><Description xml:space="preserve">{escape(pkg['description'])}</Description>
<Tags>debuggers,pic,basil</Tags><Categories>Debuggers</Categories><GalleryFlags>Public</GalleryFlags>
<Properties><Property Id="Microsoft.VisualStudio.Code.Engine" Value="{escape(pkg['engines']['vscode'])}"/><Property Id="Microsoft.VisualStudio.Code.ExtensionDependencies" Value=""/><Property Id="Microsoft.VisualStudio.Code.ExtensionPack" Value=""/></Properties></Metadata>
<Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation><Dependencies/>
<Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets>
</PackageManifest>'''
with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as z:
    z.writestr('extension.vsixmanifest', manifest)
    z.writestr('[Content_Types].xml', '<?xml version="1.0" encoding="utf-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="json" ContentType="application/json"/><Default Extension="js" ContentType="application/javascript"/><Default Extension="md" ContentType="text/markdown"/><Default Extension="vsixmanifest" ContentType="text/xml"/></Types>')
    for p in sorted(source.iterdir()):
        if p.is_file():
            z.write(p, f'extension/{p.name}')
print(output)
