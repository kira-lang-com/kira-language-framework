//! The VS Code language configuration (`KLF-EDITOR.md` section 19.3 item 3).

use crate::Language;
use crate::regex::translate;
use klf_facts::pattern::Engine;
use klf_facts::spec::{AutoCloseContext, EnterAction};
use serde_json::{Value, json};

pub(crate) fn configuration(language: &Language) -> Result<Value, String> {
    let spec = &language.spec;
    let js = |member: &str, pattern: &str| translate(member, pattern, Engine::JavaScript);
    let mut comments = json!({});
    if let Some(line) = spec.line_comments.first() {
        comments["lineComment"] = json!(line);
    }
    if let Some(block) = spec.block_comments.first() {
        comments["blockComment"] = json!([block.open, block.close]);
    }
    let brackets: Vec<Value> = spec
        .brackets
        .iter()
        .map(|pair| json!([pair.open, pair.close]))
        .collect();
    let auto_closing: Vec<Value> = spec
        .auto_close
        .iter()
        .map(|pair| {
            let mut entry = json!({ "open": pair.open, "close": pair.close });
            if !pair.not_in.is_empty() {
                entry["notIn"] = pair
                    .not_in
                    .iter()
                    .map(|context| match context {
                        AutoCloseContext::String => json!("string"),
                        AutoCloseContext::Comment => json!("comment"),
                    })
                    .collect();
            }
            entry
        })
        .collect();
    let surrounding: Vec<Value> = spec
        .auto_close
        .iter()
        .map(|pair| json!([pair.open, pair.close]))
        .collect();
    let mut configuration = json!({
        "comments": comments,
        "brackets": brackets,
        "autoClosingPairs": auto_closing,
        "surroundingPairs": surrounding,
        "wordPattern": js("identifier", &spec.identifier)?,
    });
    if let (Some(increase), Some(decrease)) = (&spec.indent.increase, &spec.indent.decrease) {
        configuration["indentationRules"] = json!({
            "increaseIndentPattern": js("indent.increase", increase)?,
            "decreaseIndentPattern": js("indent.decrease", decrease)?,
        });
    }
    if !spec.on_enter.is_empty() {
        let mut rules = Vec::new();
        for (index, rule) in spec.on_enter.iter().enumerate() {
            let mut entry = json!({
                "beforeText": js(&format!("onEnter[{index}].before"), &rule.before)?,
                "action": {
                    "indent": match rule.action {
                        EnterAction::None => "none",
                        EnterAction::Indent => "indent",
                        EnterAction::IndentOutdent => "indentOutdent",
                        EnterAction::Outdent => "outdent",
                    },
                },
            });
            if let Some(after) = &rule.after {
                entry["afterText"] = json!(js(&format!("onEnter[{index}].after"), after)?);
            }
            if !rule.append.is_empty() {
                entry["action"]["appendText"] = json!(rule.append);
            }
            rules.push(entry);
        }
        configuration["onEnterRules"] = json!(rules);
    }
    if let Some((start, end)) = &spec.region_markers {
        configuration["folding"] = json!({
            "markers": { "start": js("regionMarkers.start", start)?, "end": js("regionMarkers.end", end)? },
        });
    }
    Ok(configuration)
}
