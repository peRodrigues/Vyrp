//! Comandos de voz simples (V26): "nova linha", "novo parágrafo", "apagar isso".
//!
//! A lista é configurável (vive em `Settings.voice_commands`) e traz PT e EN por padrão.
//! Comandos de texto valem em qualquer ponto da fala; "apagar isso" só vale como
//! fala inteira, para não apagar nada por engano no meio de uma frase.

use std::collections::{HashMap, HashSet};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use super::rules::normalize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum VoiceAction {
    /// Troca a frase falada por este texto (ex.: `"\n"`).
    Insert(String),
    /// Desfaz o último paste do Vyrp.
    UndoLast,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceCommand {
    /// Frases que disparam o comando, em qualquer idioma.
    pub phrases: Vec<String>,
    pub action: VoiceAction,
}

fn command(phrases: &[&str], action: VoiceAction) -> VoiceCommand {
    VoiceCommand {
        phrases: phrases.iter().map(|p| (*p).to_string()).collect(),
        action,
    }
}

pub fn default_voice_commands() -> Vec<VoiceCommand> {
    vec![
        command(
            &["nova linha", "próxima linha", "new line"],
            VoiceAction::Insert("\n".into()),
        ),
        command(
            &["novo parágrafo", "novo paragrafo", "new paragraph"],
            VoiceAction::Insert("\n\n".into()),
        ),
        command(
            &["apagar isso", "apague isso", "delete that", "scratch that"],
            VoiceAction::UndoLast,
        ),
    ]
}

/// Lista de comandos já compilada.
#[derive(Default)]
pub struct CompiledCommands {
    inline: Option<Regex>,
    inserts: HashMap<String, String>,
    undo: HashSet<String>,
}

impl CompiledCommands {
    pub fn compile(commands: &[VoiceCommand]) -> Self {
        let mut inserts = HashMap::new();
        let mut undo = HashSet::new();
        for cmd in commands {
            for phrase in &cmd.phrases {
                let key = normalize(phrase);
                if key.is_empty() {
                    continue;
                }
                match &cmd.action {
                    VoiceAction::Insert(text) => {
                        inserts.entry(key).or_insert_with(|| text.clone());
                    }
                    VoiceAction::UndoLast => {
                        undo.insert(key);
                    }
                }
            }
        }
        let mut keys: Vec<&String> = inserts.keys().collect();
        keys.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
        let inline = if keys.is_empty() {
            None
        } else {
            let phrases: Vec<String> = keys
                .iter()
                .map(|k| {
                    k.split(' ')
                        .map(regex::escape)
                        .collect::<Vec<_>>()
                        .join(r"\s+")
                })
                .collect();
            // Come a vírgula/ponto que o STT põe em volta do comando, mas deixa o
            // ponto final da frase anterior (que fica antes do espaço).
            RegexBuilder::new(&format!(
                r"[ \t]*[,;]?[ \t]*\b(?:{})\b[.,;!?]*[ \t]*",
                phrases.join("|")
            ))
            .case_insensitive(true)
            .build()
            .ok()
        };
        Self {
            inline,
            inserts,
            undo,
        }
    }

    /// A fala inteira é um comando de desfazer?
    pub fn is_undo(&self, utterance: &str) -> bool {
        let trimmed =
            utterance.trim_matches(|c: char| c.is_whitespace() || ".,;:!?¿¡\"'".contains(c));
        self.undo.contains(&normalize(trimmed))
    }

    /// Troca os comandos de texto pelo que eles inserem.
    pub fn apply_inserts(&self, text: &str) -> String {
        let Some(regex) = &self.inline else {
            return text.to_string();
        };
        regex
            .replace_all(text, |caps: &regex::Captures<'_>| {
                let said = normalize(
                    caps[0].trim_matches(|c: char| c.is_whitespace() || ",;.!?".contains(c)),
                );
                self.inserts.get(&said).cloned().unwrap_or_default()
            })
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> CompiledCommands {
        CompiledCommands::compile(&default_voice_commands())
    }

    #[test]
    fn inserts_in_portuguese_and_english() {
        let c = defaults();
        let cases = [
            ("oi nova linha tudo bem", "oi\ntudo bem"),
            ("Oi. Nova linha. Tudo bem.", "Oi.\nTudo bem."),
            ("oi, nova linha, tudo bem", "oi\ntudo bem"),
            ("primeiro novo parágrafo segundo", "primeiro\n\nsegundo"),
            ("hello new line world", "hello\nworld"),
            ("one. New paragraph. Two", "one.\n\nTwo"),
            ("fim nova linha", "fim\n"),
            ("nova linha", "\n"),
            // Parte de outra palavra não conta.
            ("a novalinha existe", "a novalinha existe"),
        ];
        for (said, want) in cases {
            assert_eq!(c.apply_inserts(said), want, "{said:?}");
        }
    }

    #[test]
    fn undo_only_as_whole_utterance() {
        let c = defaults();
        for yes in [
            "apagar isso",
            "Apagar isso.",
            "delete that",
            "Scratch that!",
            "  apague isso ",
        ] {
            assert!(c.is_undo(yes), "{yes:?}");
        }
        for no in ["não vou apagar isso agora", "delete that file", "apagar"] {
            assert!(!c.is_undo(no), "{no:?}");
        }
    }

    #[test]
    fn custom_list_replaces_the_defaults() {
        let c = CompiledCommands::compile(&[
            command(&["quebra"], VoiceAction::Insert("\n".into())),
            command(&["volta"], VoiceAction::UndoLast),
        ]);
        assert_eq!(c.apply_inserts("a quebra b"), "a\nb");
        assert_eq!(c.apply_inserts("a nova linha b"), "a nova linha b");
        assert!(c.is_undo("volta"));
        assert!(!c.is_undo("apagar isso"));
    }

    #[test]
    fn empty_list_does_nothing() {
        let c = CompiledCommands::compile(&[]);
        assert_eq!(c.apply_inserts("nova linha"), "nova linha");
        assert!(!c.is_undo("apagar isso"));
    }

    #[test]
    fn commands_round_trip_through_json() {
        let list = default_voice_commands();
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<VoiceCommand>>(&json).unwrap(),
            list
        );
    }
}
