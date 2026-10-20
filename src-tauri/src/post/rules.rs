//! Dicionário e snippets compilados numa regex única (spec §1.6 e V14).

use std::collections::HashMap;

use regex::{Regex, RegexBuilder};

use crate::storage::DictionaryEntry;

/// Uma substituição pronta para aplicar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub phrase: String,
    pub replacement: String,
    pub html: Option<String>,
    pub is_snippet: bool,
}

/// Chave de comparação: espaços colapsados e minúsculas.
pub fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Regras compiladas. Vazio é válido (nada a substituir).
#[derive(Default)]
pub struct Rules {
    regex: Option<Regex>,
    by_key: HashMap<String, Rule>,
}

impl Rules {
    /// Entradas do dicionário viva e com substituição; vocabulário puro (sem
    /// substituição) serve só ao STT e fica de fora. Frase repetida: vale a primeira.
    pub fn from_entries(entries: &[DictionaryEntry]) -> Self {
        Self::compile(
            entries
                .iter()
                .filter(|e| !e.is_deleted)
                .filter_map(|e| {
                    Some(Rule {
                        phrase: e.phrase.clone(),
                        replacement: e.replacement.clone()?,
                        html: e.replacement_html.clone(),
                        is_snippet: e.is_snippet,
                    })
                })
                .collect(),
        )
    }

    pub fn compile(rules: Vec<Rule>) -> Self {
        let mut by_key: HashMap<String, Rule> = HashMap::new();
        for rule in rules {
            let key = normalize(&rule.phrase);
            if !key.is_empty() {
                by_key.entry(key).or_insert(rule);
            }
        }
        if by_key.is_empty() {
            return Self::default();
        }
        // Mais longa primeiro: "meu e-mail pessoal" ganha de "meu e-mail".
        let mut keys: Vec<&String> = by_key.keys().collect();
        keys.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
        let alternatives: Vec<String> = keys.iter().map(|k| pattern_for(k)).collect();
        let regex = RegexBuilder::new(&alternatives.join("|"))
            .case_insensitive(true)
            .build()
            .ok();
        Self { regex, by_key }
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Se a fala inteira (sem pontuação nas pontas) é o gatilho de um snippet.
    pub fn whole_snippet(&self, utterance: &str) -> Option<&Rule> {
        let trimmed =
            utterance.trim_matches(|c: char| c.is_whitespace() || ".,;:!?¿¡\"'".contains(c));
        self.by_key
            .get(&normalize(trimmed))
            .filter(|r| r.is_snippet)
    }

    /// Troca cada ocorrência (sem distinção de caixa, em fronteira de palavra) pelo texto.
    pub fn apply(&self, text: &str) -> String {
        let Some(regex) = &self.regex else {
            return text.to_string();
        };
        regex
            .replace_all(text, |caps: &regex::Captures<'_>| {
                match self.by_key.get(&normalize(&caps[0])) {
                    Some(rule) => rule.replacement.clone(),
                    None => caps[0].to_string(),
                }
            })
            .into_owned()
    }
}

/// Padrão de uma frase: espaços viram `\s+`; `\b` só nas pontas que são letra/dígito
/// (uma frase como "c++" não tem fronteira de palavra no fim).
fn pattern_for(key: &str) -> String {
    let body = key
        .split(' ')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(r"\s+");
    let start = if key.chars().next().is_some_and(is_word) {
        r"\b"
    } else {
        ""
    };
    let end = if key.chars().last().is_some_and(is_word) {
        r"\b"
    } else {
        ""
    };
    format!("{start}{body}{end}")
}
