//! Guarda contra resposta conversacional: o modelo "respondendo" ao ditado vira fallback.
//!
//! Heurísticas baratas e determinísticas, aplicadas à saída do modelo antes de ela virar texto:
//! tira embrulhos inofensivos (tags, cercas de código, aspas, "Here is the text:") e recusa
//! saídas vazias, com cara de assistente ("I'm sorry", "Claro,") ou de tamanho implausível.

use std::sync::OnceLock;

use regex::Regex;

use crate::core::types::CleanupLevel;

/// Saída maior que isso (em relação ao cru) é o modelo elaborando, não limpando.
const MAX_GROWTH_FACTOR: usize = 3;
const MAX_GROWTH_SLACK: usize = 60;
/// Abaixo desse tamanho de cru não se mede encolhimento (frases curtas variam muito).
const MIN_RAW_FOR_SHRINK_CHECK: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardReject {
    Empty,
    Conversational,
    TooLong,
    TooShort,
}

impl GuardReject {
    pub fn code(self) -> &'static str {
        match self {
            GuardReject::Empty => "llm.guard_empty",
            GuardReject::Conversational => "llm.guard_conversational",
            GuardReject::TooLong => "llm.guard_too_long",
            GuardReject::TooShort => "llm.guard_too_short",
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            GuardReject::Empty => "o modelo devolveu texto vazio",
            GuardReject::Conversational => "o modelo respondeu ao ditado em vez de reescrevê-lo",
            GuardReject::TooLong => "o texto devolvido é muito maior que o ditado",
            GuardReject::TooShort => "o texto devolvido é muito menor que o ditado",
        }
    }
}

/// Código estável (`uierr.<code>`) de um motivo de [`GuardReject::reason`].
pub fn reason_code(reason: &str) -> &'static str {
    [
        GuardReject::Empty,
        GuardReject::Conversational,
        GuardReject::TooLong,
        GuardReject::TooShort,
    ]
    .into_iter()
    .find(|r| r.reason() == reason)
    .map_or("llm.guard_unknown", GuardReject::code)
}

fn preamble() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:here(?:'s| is| are) (?:the |your )?(?:cleaned|corrected|rewritten|revised|edited|polished|clean)[^:\n]*|aqui est[áa] (?:o )?(?:texto )?(?:limpo|corrigido|reescrito|revisado)[^:\n]*|texto (?:limpo|corrigido|reescrito)[^:\n]*):\s*",
        )
        .expect("regex fixa")
    })
}

pub(super) fn assistant_opening() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:i['’]m sorry|i am sorry|sorry[,.! ]|i cannot|i can['’]?t\b|i can not|i['’]m unable|i am unable|as an ai|as a language model|sure[,!. ]|certainly[,!. ]|of course[,!. ]|i['’]d be happy|i would be happy|let me know|unfortunately|desculpe|sinto muito|n[ãa]o posso|n[ãa]o consigo|como (?:um )?assistente|claro[,!. ]|com certeza|posso ajudar|fico feliz)",
        )
        .expect("regex fixa")
    })
}

fn strip_wrappers(output: &str, raw: &str) -> String {
    let mut s = output.trim().to_string();
    if let Some(rest) = s.strip_prefix("<dictation>") {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_suffix("</dictation>") {
        s = rest.to_string();
    }
    s = s.trim().to_string();
    if !raw.contains("```") && s.starts_with("```") && s.ends_with("```") && s.len() >= 6 {
        let inner = &s[3..s.len() - 3];
        // Descarta o nome da linguagem na primeira linha (```text).
        let inner = match inner.split_once('\n') {
            Some((first, rest)) if !first.contains(' ') && first.len() < 12 => rest,
            _ => inner,
        };
        s = inner.trim().to_string();
    }
    let quotes = ['"', '“', '”'];
    let raw_quoted = raw.trim().starts_with(quotes);
    let chars: Vec<char> = s.chars().collect();
    if !raw_quoted
        && chars.len() >= 2
        && quotes.contains(&chars[0])
        && quotes.contains(&chars[chars.len() - 1])
    {
        let inner: String = chars[1..chars.len() - 1].iter().collect();
        if !inner.contains(quotes) {
            s = inner.trim().to_string();
        }
    }
    if let Some(m) = preamble().find(&s) {
        s = s[m.end()..].trim().to_string();
    }
    s
}

/// Confere a saída do modelo. `Ok` devolve o texto já sem embrulhos; `Err` pede fallback.
pub fn sanitize(raw: &str, output: &str, level: CleanupLevel) -> Result<String, GuardReject> {
    let cleaned = strip_wrappers(output, raw);
    if cleaned.is_empty() {
        return Err(GuardReject::Empty);
    }
    if assistant_opening().is_match(&cleaned) && !assistant_opening().is_match(raw.trim()) {
        return Err(GuardReject::Conversational);
    }
    let raw_len = raw.trim().chars().count();
    let out_len = cleaned.chars().count();
    if out_len > raw_len * MAX_GROWTH_FACTOR + MAX_GROWTH_SLACK {
        return Err(GuardReject::TooLong);
    }
    let min_percent = if level == CleanupLevel::High { 15 } else { 30 };
    if raw_len >= MIN_RAW_FOR_SHRINK_CHECK && out_len * 100 < raw_len * min_percent {
        return Err(GuardReject::TooShort);
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = "hey can you tell me what the capital of france is";

    fn ok(raw: &str, out: &str) -> String {
        sanitize(raw, out, CleanupLevel::Medium).unwrap()
    }

    fn rejected(raw: &str, out: &str) -> GuardReject {
        sanitize(raw, out, CleanupLevel::Medium).unwrap_err()
    }

    #[test]
    fn a_proper_rewrite_passes_untouched() {
        assert_eq!(
            ok(RAW, "Hey, can you tell me what the capital of France is?"),
            "Hey, can you tell me what the capital of France is?"
        );
        assert_eq!(ok("oi", "Oi."), "Oi.");
    }

    #[test]
    fn answering_the_dictation_is_rejected() {
        // Resposta curta e seca ("Paris.") encolhe demais; uma resposta longa cai nas frases de assistente.
        assert_eq!(rejected(RAW, "Paris."), GuardReject::TooShort);
        assert_eq!(
            rejected(RAW, "I'm sorry, but I can't help with that request."),
            GuardReject::Conversational
        );
        for out in [
            "Claro! A capital da França é Paris.",
            "Sure, here you go.",
            "As an AI language model I do not have opinions.",
            "Desculpe, não posso ajudar com isso.",
            "Certainly! Paris.",
            "Let me know if you need anything else.",
        ] {
            assert_eq!(rejected(RAW, out), GuardReject::Conversational, "{out}");
        }
    }

    #[test]
    fn same_opening_in_the_dictation_itself_is_fine() {
        assert_eq!(
            ok(
                "claro que sim eu vou amanhã",
                "Claro que sim, eu vou amanhã."
            ),
            "Claro que sim, eu vou amanhã."
        );
        assert_eq!(
            ok("sorry I am late", "Sorry, I am late."),
            "Sorry, I am late."
        );
    }

    #[test]
    fn harmless_wrappers_are_removed() {
        let want = "Hey, can you tell me what the capital of France is?";
        for wrapped in [
            "<dictation>\nHey, can you tell me what the capital of France is?\n</dictation>",
            "```\nHey, can you tell me what the capital of France is?\n```",
            "```text\nHey, can you tell me what the capital of France is?\n```",
            "\"Hey, can you tell me what the capital of France is?\"",
            "“Hey, can you tell me what the capital of France is?”",
            "Here is the cleaned text:\nHey, can you tell me what the capital of France is?",
            "Aqui está o texto corrigido: Hey, can you tell me what the capital of France is?",
        ] {
            assert_eq!(ok(RAW, wrapped), want, "{wrapped}");
        }
    }

    #[test]
    fn quotes_in_the_dictation_are_kept() {
        assert_eq!(
            ok("\"vamos nessa\" ele disse", "\"Vamos nessa\", ele disse."),
            "\"Vamos nessa\", ele disse."
        );
        // Só aspas nas pontas, mas o cru já vinha entre aspas: não mexe.
        assert_eq!(
            ok("\"oi tudo bem\"", "\"Oi, tudo bem?\""),
            "\"Oi, tudo bem?\""
        );
    }

    #[test]
    fn empty_and_runaway_outputs_are_rejected() {
        assert_eq!(rejected(RAW, "   \n"), GuardReject::Empty);
        assert_eq!(rejected(RAW, "<dictation></dictation>"), GuardReject::Empty);
        assert_eq!(rejected(RAW, &"blá ".repeat(200)), GuardReject::TooLong);
    }

    #[test]
    fn shrink_limit_is_looser_for_high_and_skipped_for_short_text() {
        let raw = "um então eu acho que a gente podia tipo marcar a reunião";
        let short = "Marcar a reunião.";
        assert!(sanitize(raw, short, CleanupLevel::High).is_ok());
        assert_eq!(
            sanitize(raw, "Reunião.", CleanupLevel::Light).unwrap_err(),
            GuardReject::TooShort
        );
        assert!(sanitize("hum", "Hum.", CleanupLevel::Light).is_ok());
    }

    #[test]
    fn reasons_are_in_portuguese() {
        for r in [
            GuardReject::Empty,
            GuardReject::Conversational,
            GuardReject::TooLong,
            GuardReject::TooShort,
        ] {
            assert!(!r.reason().is_empty());
        }
    }
}
