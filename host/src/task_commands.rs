//! User-authored task recipes. Page data is never interpolated into a prompt.
use std::collections::HashSet;

use brauser_protocol::TaskCommand;
use serde::Deserialize;

pub(crate) const REGISTRY_ISSUE: &str = "Saved task commands in config.toml are invalid. Use unique lowercase names, descriptions and literal prompts, with output = \"sidebar\". Templates, include and other outputs are not supported yet. Fix [[commands]] and reload the panel; ordinary tasks still work.";
pub(crate) const UNKNOWN_COMMAND: &str =
    "Unknown task command. Choose a saved command, or use // for a literal leading slash.";
pub(crate) const STALE_COMMAND: &str =
    "Saved commands changed. Review the refreshed list and press Run again.";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Command {
    pub name: String,
    pub description: String,
    pub prompt: String,
    #[serde(default)]
    output: Option<String>,
}

impl Command {
    pub fn definition(&self) -> brauser_protocol::TaskCommandDefinition {
        brauser_protocol::TaskCommandDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            prompt: self.prompt.clone(),
        }
    }

    pub fn descriptor(&self) -> TaskCommand {
        TaskCommand {
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}

pub(crate) fn parse(raw: Option<&toml::Value>) -> Result<Vec<Command>, &'static str> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let commands: Vec<Command> = raw.clone().try_into().map_err(|_| REGISTRY_ISSUE)?;
    if commands.len() > 32 {
        return Err(REGISTRY_ISSUE);
    }
    let mut names = HashSet::new();
    for command in &commands {
        let mut chars = command.name.bytes();
        if !chars.next().is_some_and(|c| c.is_ascii_lowercase())
            || command.name.len() > 32
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c))
            || ["summarize", "note", "save", "related", "log"].contains(&command.name.as_str())
            || !names.insert(&command.name)
            || command.description.trim().is_empty()
            || command.description.chars().count() > 256
            || command.description.chars().any(char::is_control)
            || command.prompt.trim().is_empty()
            || command.prompt.chars().count() > 4096
            || command
                .prompt
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            || command
                .output
                .as_deref()
                .is_some_and(|output| output != "sidebar")
            || [
                "{page.title}",
                "{page.url}",
                "{page.content}",
                "{selection}",
                "{input}",
                "{date}",
            ]
            .iter()
            .any(|token| command.prompt.contains(token))
        {
            return Err(REGISTRY_ISSUE);
        }
    }
    Ok(commands)
}

pub(crate) enum Input<'a> {
    Literal(&'a str),
    Command { name: &'a str, extra: &'a str },
}

/// The first slash escapes a literal slash; all other leading slashes are
/// explicit command invocations and must never fall through to an agent.
pub(crate) fn input(text: &str) -> Input<'_> {
    if text.starts_with("//") {
        return Input::Literal(&text[1..]);
    }
    if let Some(command) = text.strip_prefix('/') {
        let end = command.find(char::is_whitespace).unwrap_or(command.len());
        return Input::Command {
            name: &command[..end],
            extra: command[end..].trim(),
        };
    }
    Input::Literal(text)
}

pub(crate) fn resolve(
    commands: &[Command],
    name: &str,
    extra: &str,
) -> Result<String, &'static str> {
    let command = commands
        .iter()
        .find(|command| command.name == name)
        .ok_or(UNKNOWN_COMMAND)?;
    let mut instruction = format!("Saved command /{}:\n\n{}", command.name, command.prompt);
    if !extra.is_empty() {
        instruction.push_str("\n\nAdditional request from the user:\n");
        instruction.push_str(extra);
    }
    Ok(instruction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(value: serde_json::Value) -> toml::Value {
        toml::Value::try_from(&value).unwrap()
    }
    fn recipe() -> serde_json::Value {
        json!({"name":"actions", "description":"Extract action items", "prompt":"Return a checklist.", "output":"sidebar"})
    }

    #[test]
    fn literal_prompts_compose_only_the_users_extra_instruction() {
        let commands = parse(Some(&raw(json!([recipe()])))).unwrap();
        assert_eq!(
            resolve(&commands, "actions", "").unwrap(),
            "Saved command /actions:\n\nReturn a checklist."
        );
        assert_eq!(
            resolve(&commands, "actions", "Focus on {input} /actions").unwrap(),
            "Saved command /actions:\n\nReturn a checklist.\n\nAdditional request from the user:\nFocus on {input} /actions"
        );
        assert!(resolve(&commands, "missing", "").is_err());
        assert_eq!(commands[0].descriptor().name, "actions");
    }

    #[test]
    fn a_registry_refuses_duplicates_unsupported_fields_outputs_templates_and_bad_names() {
        for (key, value) in [
            ("name", json!("summarize")),
            ("name", json!("Actions")),
            ("name", json!("é")),
            ("name", json!("a/b")),
            ("name", json!("")),
            ("name", json!("a".repeat(33))),
            ("description", json!("x\ny")),
            ("description", json!("")),
            ("prompt", json!(" ")),
            ("prompt", json!("x\u{0000}")),
            ("prompt", json!("x".repeat(4097))),
            ("output", json!("new_note")),
            ("output", json!("clipboard")),
            ("output", json!("page_note")),
            ("include", json!(["page.content"])),
            ("cwd", json!("/tmp")),
        ] {
            let mut item = recipe();
            item[key] = value;
            assert!(parse(Some(&raw(json!([item])))).is_err(), "{key}");
        }
        for token in [
            "{page.title}",
            "{page.url}",
            "{page.content}",
            "{selection}",
            "{input}",
            "{date}",
        ] {
            let mut item = recipe();
            item["prompt"] = json!(token);
            assert!(parse(Some(&raw(json!([item])))).is_err(), "{token}");
        }
        assert!(parse(Some(&raw(json!([recipe(), recipe()])))).is_err());
        assert!(
            parse(Some(&raw(json!(
                (0..33)
                    .map(|n| {
                        let mut r = recipe();
                        r["name"] = json!(format!("c{n}"));
                        r
                    })
                    .collect::<Vec<_>>()
            ))))
            .is_err()
        );
        assert!(parse(None).unwrap().is_empty());
        let mut item = recipe();
        item.as_object_mut().unwrap().remove("output");
        assert!(parse(Some(&raw(json!([item])))).is_ok());
    }

    #[test]
    fn descriptions_are_bounded_by_unicode_characters() {
        for (description, valid) in [("🔎".repeat(256), true), ("🔎".repeat(257), false)] {
            let mut item = recipe();
            item["description"] = json!(description);
            assert_eq!(parse(Some(&raw(json!([item])))).is_ok(), valid);
        }
    }

    #[test]
    fn slash_input_is_explicit_and_literals_remove_only_one_slash() {
        assert!(matches!(
            input("/actions \n details"),
            Input::Command {
                name: "actions",
                extra: "details"
            }
        ));
        assert!(matches!(input("//actions"), Input::Literal("/actions")));
        assert!(matches!(input("///tmp/file"), Input::Literal("//tmp/file")));
        assert!(matches!(
            input("Do /actions"),
            Input::Literal("Do /actions")
        ));
        assert!(matches!(
            input("/"),
            Input::Command {
                name: "",
                extra: ""
            }
        ));
    }
}
