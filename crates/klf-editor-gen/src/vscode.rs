//! The VS Code extension, written as a `.vsix` directly (`KLF-EDITOR.md`
//! section 19.5): an OPC zip with `extension.vsixmanifest` and
//! `[Content_Types].xml`, so no Node.js or `vsce` is needed.

use crate::{File, Language, zip};
use serde_json::{Map, Value, json};

const ENGINE: &str = "^1.82.0";

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The extension's `package.json`.
fn manifest(language: &Language, client: bool) -> Value {
    let spec = &language.spec;
    let id = language.identifier();
    let mut properties = Map::new();
    for (name, default, about) in language.settings() {
        let kind = match &default {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "integer",
            Value::Array(_) => "array",
            _ => "string",
        };
        properties.insert(
            name,
            json!({ "type": kind, "default": default, "description": about }),
        );
    }
    let (_, modifiers) = klf_lsp::legend_names();
    let custom: Vec<Value> = modifiers
        .iter()
        .skip_while(|modifier| *modifier != "defaultLibrary")
        .skip(1)
        .map(|modifier| json!({ "id": modifier, "description": format!("KLF modifier `{modifier}`") }))
        .collect();
    let mut entry = json!({
        "id": language.name,
        "aliases": [spec.display_name, language.name],
        "extensions": language.extensions.iter().map(|extension| format!(".{extension}")).collect::<Vec<_>>(),
        "configuration": "./language-configuration.json",
    });
    if !spec.file_names.is_empty() {
        entry["filenames"] = json!(spec.file_names);
    }
    if let Some(first_line) = &spec.first_line {
        entry["firstLine"] = json!(first_line);
    }
    let mut package = json!({
        "name": id.replace('_', "-"),
        "displayName": spec.display_name,
        "description": format!("{} language support, generated from its KLF definition", spec.display_name),
        "version": language.version,
        "publisher": "klf",
        "engines": { "vscode": ENGINE },
        "categories": ["Programming Languages"],
        "contributes": {
            "languages": [entry],
            "grammars": [{
                "language": language.name,
                "scopeName": spec.scope,
                "path": format!("./syntaxes/{id}.tmLanguage.json"),
            }],
            "configuration": { "title": spec.display_name, "properties": properties },
            "semanticTokenTypes": [
                { "id": "escapeSequence", "superType": "string", "description": "An escape sequence in a string" },
                { "id": "label", "superType": "variable", "description": "A label" },
            ],
            "semanticTokenModifiers": custom,
            "semanticTokenScopes": [{
                "language": language.name,
                "scopes": {
                    "escapeSequence": ["constant.character.escape"],
                    "label": ["entity.name.label"],
                },
            }],
        },
    });
    if client {
        package["main"] = json!("./extension.js");
        package["activationEvents"] = json!([format!("onLanguage:{}", language.name)]);
        package["contributes"]["commands"] = json!([
            { "command": "klf.restartServer", "title": format!("{}: Restart Language Server", spec.display_name) },
            { "command": "klf.reloadProjects", "title": format!("{}: Reload Projects", spec.display_name) },
            { "command": "klf.run", "title": format!("{}: Run", spec.display_name) },
            { "command": "klf.showOutput", "title": format!("{}: Show Shader Output", spec.display_name) },
            { "command": "klf.showKir", "title": format!("{}: Show KIR", spec.display_name) },
            { "command": "klf.showExpansion", "title": format!("{}: Show Macro Expansion", spec.display_name) },
        ]);
        // Both take the arguments of the lens that runs them.
        package["contributes"]["menus"] = json!({ "commandPalette": [
            { "command": "klf.run", "when": "false" },
            { "command": "klf.showOutput", "when": "false" },
        ] });
    }
    package
}

fn vsix_manifest(language: &Language) -> String {
    let spec = &language.spec;
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="{id}" Version="{version}" Publisher="klf" />
    <DisplayName>{display}</DisplayName>
    <Description xml:space="preserve">{display} language support, generated from its KLF definition</Description>
    <Tags>{name}</Tags>
    <Categories>Programming Languages</Categories>
    <GalleryFlags>Public</GalleryFlags>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="{engine}" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionDependencies" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionPack" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionKind" Value="workspace" />
      <Property Id="Microsoft.VisualStudio.Code.LocalizedLanguages" Value="" />
    </Properties>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code" />
  </Installation>
  <Dependencies />
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
  </Assets>
</PackageManifest>
"#,
        id = xml(&language.identifier().replace('_', "-")),
        version = xml(&language.version),
        display = xml(&spec.display_name),
        name = xml(&language.name),
        engine = ENGINE,
    )
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json" /><Default Extension=".vsixmanifest" ContentType="text/xml" /><Default Extension=".js" ContentType="application/javascript" /></Types>
"#;

pub(crate) fn package(
    language: &Language,
    grammar: &Value,
    configuration: &Value,
    client: Option<&[u8]>,
) -> Result<File, String> {
    let id = language.identifier();
    let pretty =
        |value: &Value| serde_json::to_vec_pretty(value).map_err(|error| error.to_string());
    let mut entries = vec![
        (
            "[Content_Types].xml".to_owned(),
            CONTENT_TYPES.as_bytes().to_vec(),
        ),
        (
            "extension.vsixmanifest".to_owned(),
            vsix_manifest(language).into_bytes(),
        ),
        (
            "extension/package.json".to_owned(),
            pretty(&manifest(language, client.is_some()))?,
        ),
        (
            "extension/language-configuration.json".to_owned(),
            pretty(configuration)?,
        ),
        (
            format!("extension/syntaxes/{id}.tmLanguage.json"),
            pretty(grammar)?,
        ),
        (
            "extension/manifest.json".to_owned(),
            pretty(&crate::manifest(language))?,
        ),
    ];
    if let Some(client) = client {
        entries.push(("extension/extension.js".to_owned(), client.to_vec()));
    }
    Ok(File {
        path: format!("vscode/{}-{}.vsix", id.replace('_', "-"), language.version).into(),
        contents: zip::archive(&entries),
    })
}
