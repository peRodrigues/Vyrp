//! Pós-processo determinístico: dicionário, snippets, "aperte enter", formatos por app.
//!
//! Card V14 (spec `docs/re/wispr-flow-spec.md` §1.5, §1.6 e §6). Roda depois do
//! STT e antes do LLM (ou sozinho, quando a limpeza por LLM está desligada).
//! Ordem: snippet de fala inteira → comando "aperte enter" → formato → dicionário
//! → espaço inicial.

pub mod commands;
pub mod rules;

use std::sync::RwLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::storage::{self, Db, DictionaryEntry};
pub use commands::{CompiledCommands, VoiceAction, VoiceCommand, default_voice_commands};
pub use rules::{Rule, Rules};

/// Formato do texto por app (spec §1.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFormat {
    /// Só o que o STT disse (com dicionário e espaço inicial).
    Raw,
    /// Tudo em minúsculas (nomes do dicionário mantêm a grafia).
    Lowercase,
    /// Espaços arrumados e maiúscula no começo de frase.
    #[default]
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostOptions {
    pub format: TextFormat,
    /// Texto imediatamente antes do cursor, quando o contexto sabe. Decide o
    /// espaço inicial e a maiúscula.
    pub preceding: Option<String>,
    /// Reconhecer o comando falado "aperte enter" / "press enter" no fim.
    pub press_enter_command: bool,
    /// Reconhecer "nova linha", "novo parágrafo" e "apagar isso" (V26).
    pub voice_commands: bool,
}

impl Default for PostOptions {
    fn default() -> Self {
        Self {
            format: TextFormat::default(),
            preceding: None,
            press_enter_command: true,
            voice_commands: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostResult {
    pub text: String,
    /// Só quando a fala inteira era um snippet com versão HTML.
    pub html: Option<String>,
    /// O inseridor deve apertar Enter depois de colar.
    pub press_enter: bool,
    /// A fala foi "apagar isso": o inseridor desfaz o último paste e não cola nada.
    pub undo_last: bool,
}

/// Dono das regras compiladas. Compartilhe por referência (`app.manage`).
pub struct PostProcessor {
    rules: RwLock<Rules>,
    commands: RwLock<CompiledCommands>,
    vocabulary: RwLock<Vec<String>>,
}

impl Default for PostProcessor {
    fn default() -> Self {
        Self {
            rules: RwLock::default(),
            commands: RwLock::new(CompiledCommands::compile(&default_voice_commands())),
            vocabulary: RwLock::default(),
        }
    }
}

impl PostProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Troca a lista de comandos de voz (vem de `Settings.voice_commands`).
    pub fn set_voice_commands(&self, commands: &[VoiceCommand]) {
        *self.commands.write().unwrap() = CompiledCommands::compile(commands);
    }

    /// Recompila a regex única. Chamar quando o dicionário mudar.
    pub fn reload(&self, entries: &[DictionaryEntry]) {
        *self.rules.write().unwrap() = Rules::from_entries(entries);
    }

    pub fn reload_from_db(&self, db: &Db) -> storage::Result<()> {
        self.reload(&db.dictionary().list()?);
        // Mesmo gancho de toda mudança do dicionário: o vocabulário do STT/LLM fica em
        // cache aqui e é refeito junto com as regras (V11b).
        *self.vocabulary.write().unwrap() = db.dictionary().vocabulary()?;
        Ok(())
    }

    /// Vocabulário do dicionário (favoritos e mais usados primeiro), para o prompt do STT
    /// e o pedido de limpeza do LLM. Cache refeito em [`PostProcessor::reload_from_db`].
    pub fn vocabulary(&self) -> Vec<String> {
        self.vocabulary.read().unwrap().clone()
    }

    pub fn process(&self, raw: &str, opts: &PostOptions) -> PostResult {
        process_with(
            &self.rules.read().unwrap(),
            &self.commands.read().unwrap(),
            raw,
            opts,
        )
    }
}

/// Comando de Enter falado, só no fim da fala. Frases em PT e EN.
fn press_enter_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|\s)(?:aperte|aperta|pressione|pressiona|dê|press)\s+(?:o\s+|the\s+)?enter[\s.,;:!?]*$",
        )
        .expect("regex fixa")
    })
}

pub fn process_with(
    rules: &Rules,
    commands: &CompiledCommands,
    raw: &str,
    opts: &PostOptions,
) -> PostResult {
    let raw = raw.trim();
    if opts.voice_commands && commands.is_undo(raw) {
        return PostResult {
            undo_last: true,
            ..PostResult::default()
        };
    }
    if let Some(snippet) = rules.whole_snippet(raw) {
        return PostResult {
            text: with_leading_space(opts.preceding.as_deref(), snippet.replacement.clone()),
            html: snippet.html.clone(),
            press_enter: false,
            undo_last: false,
        };
    }

    let mut text = raw.to_string();
    let mut press_enter = false;
    if opts.press_enter_command
        && let Some(m) = press_enter_regex().find(&text)
    {
        text.truncate(m.start());
        let kept = text
            .trim_end()
            .trim_end_matches([',', ';', ':'])
            .trim_end()
            .len();
        text.truncate(kept);
        press_enter = true;
    }

    if opts.voice_commands {
        text = commands.apply_inserts(&text);
    }

    text = match opts.format {
        TextFormat::Raw => text,
        TextFormat::Lowercase => text.to_lowercase(),
        TextFormat::Normal => capitalize_lines(&capitalize_if_sentence_start(
            &collapse_blanks(&text),
            opts.preceding.as_deref(),
        )),
    };
    text = rules.apply(&text);

    PostResult {
        text: with_leading_space(opts.preceding.as_deref(), text),
        html: None,
        press_enter,
        undo_last: false,
    }
}

/// Vários espaços/tabs viram um; quebras de linha ficam.
fn collapse_blanks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_blank = false;
    for c in s.chars() {
        let blank = c == ' ' || c == '\t';
        if !(blank && last_blank) {
            out.push(if blank { ' ' } else { c });
        }
        last_blank = blank;
    }
    out
}

/// Maiúscula na primeira letra se o cursor está no começo de uma frase.
fn capitalize_if_sentence_start(text: &str, preceding: Option<&str>) -> String {
    let at_sentence_start = match preceding {
        None => true,
        Some(p) if p.trim().is_empty() || p.ends_with('\n') => true,
        Some(p) => p.trim_end().ends_with(['.', '!', '?', '…']),
    };
    if !at_sentence_start {
        return text.to_string();
    }
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Maiúscula na primeira letra depois de cada quebra de linha.
fn capitalize_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_newline = false;
    for c in text.chars() {
        if after_newline && c.is_alphabetic() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        after_newline = c == '\n' || (after_newline && c == ' ');
    }
    out
}

/// Espaço inicial conforme o texto antes do cursor: falta espaço entre a
/// palavra anterior e a nova, mas não depois de espaço/abre-parêntese nem antes
/// de pontuação de fechamento.
pub fn needs_leading_space(preceding: Option<&str>, text: &str) -> bool {
    let (Some(prev), Some(first)) = (
        preceding.and_then(|p| p.chars().last()),
        text.chars().next(),
    ) else {
        return false;
    };
    if prev.is_whitespace() || "([{\"'“‘«/-".contains(prev) {
        return false;
    }
    !(first.is_whitespace() || ".,;:!?)]}%…".contains(first))
}

fn with_leading_space(preceding: Option<&str>, text: String) -> String {
    if text.is_empty() || !needs_leading_space(preceding, &text) {
        return text;
    }
    format!(" {text}")
}

/// Registra o [`PostProcessor`] com o dicionário e os comandos de voz atuais.
/// Chamar no `setup`, depois de `settings::init` e `storage::init`.
pub fn init(app: &tauri::AppHandle) {
    use tauri::Manager;
    let processor = PostProcessor::new();
    if let Some(db) = app.try_state::<Db>()
        && let Err(err) = processor.reload_from_db(&db)
    {
        tracing::warn!(error = %err, "dicionário não carregado no pós-processo");
    }
    if let Some(settings) = app.try_state::<crate::settings::SettingsStore>() {
        processor.set_voice_commands(&settings.get().voice_commands);
    }
    app.manage(processor);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(phrase: &str, replacement: Option<&str>, snippet: bool) -> DictionaryEntry {
        DictionaryEntry {
            id: 0,
            phrase: phrase.into(),
            replacement: replacement.map(Into::into),
            replacement_html: None,
            is_snippet: snippet,
            manual_entry: true,
            origin: None,
            observed_asr_error: None,
            frequency: 0,
            last_used_at: None,
            favorite: false,
            is_deleted: false,
            created_at: 0,
        }
    }

    fn pp(entries: &[DictionaryEntry]) -> PostProcessor {
        let p = PostProcessor::new();
        p.reload(entries);
        p
    }

    fn run(
        p: &PostProcessor,
        raw: &str,
        format: TextFormat,
        preceding: Option<&str>,
    ) -> PostResult {
        p.process(
            raw,
            &PostOptions {
                format,
                preceding: preceding.map(Into::into),
                press_enter_command: true,
                voice_commands: true,
            },
        )
    }

    #[test]
    fn press_enter_table() {
        let p = pp(&[]);
        let cases = [
            ("manda o relatório, aperte enter", "Manda o relatório", true),
            (
                "manda o relatório. Aperte enter.",
                "Manda o relatório.",
                true,
            ),
            ("ok pressione o enter", "Ok", true),
            ("send it, press enter", "Send it", true),
            ("Send it. Press the Enter!", "Send it.", true),
            ("enter", "Enter", false),
            ("aperte enter", "", true),
            ("press enter", "", true),
            // Só vale no fim da fala.
            (
                "press enter para confirmar",
                "Press enter para confirmar",
                false,
            ),
            (
                "o botão enter está quebrado",
                "O botão enter está quebrado",
                false,
            ),
        ];
        for (raw, text, enter) in cases {
            let r = run(&p, raw, TextFormat::Normal, None);
            assert_eq!((r.text.as_str(), r.press_enter), (text, enter), "{raw:?}");
        }
    }

    #[test]
    fn press_enter_can_be_turned_off() {
        let p = pp(&[]);
        let r = p.process(
            "oi, aperte enter",
            &PostOptions {
                press_enter_command: false,
                ..Default::default()
            },
        );
        assert_eq!(r.text, "Oi, aperte enter");
        assert!(!r.press_enter);
    }

    #[test]
    fn dictionary_replaces_case_insensitive_on_word_boundaries() {
        let p = pp(&[
            entry("virp", Some("Vyrp"), false),
            entry("ação", Some("AÇÃO"), false),
            entry("c++", Some("C++"), false),
        ]);
        let cases = [
            ("o virp e o VIRP funcionam", "O Vyrp e o Vyrp funcionam"),
            // "virpar" não é "virp": fronteira de palavra.
            ("virpar não muda", "Virpar não muda"),
            // Unicode: acento conta como letra.
            ("quero ação agora", "Quero AÇÃO agora"),
            ("preparação não muda", "Preparação não muda"),
            ("aprendi c++ ontem", "Aprendi C++ ontem"),
        ];
        for (raw, want) in cases {
            assert_eq!(run(&p, raw, TextFormat::Normal, None).text, want, "{raw:?}");
        }
    }

    #[test]
    fn dictionary_works_in_english_too() {
        let p = pp(&[entry("tory", Some("Tauri"), false)]);
        assert_eq!(
            run(
                &p,
                "we build it with Tory and rust",
                TextFormat::Normal,
                None
            )
            .text,
            "We build it with Tauri and rust"
        );
    }

    #[test]
    fn longest_phrase_wins_and_spaces_are_flexible() {
        let p = pp(&[
            entry("meu email", Some("eu@x.com"), false),
            entry("meu email pessoal", Some("pessoal@x.com"), false),
        ]);
        assert_eq!(
            run(&p, "manda para meu  email pessoal", TextFormat::Raw, None).text,
            "manda para pessoal@x.com"
        );
        assert_eq!(
            run(&p, "manda para meu email", TextFormat::Raw, None).text,
            "manda para eu@x.com"
        );
    }

    #[test]
    fn vocabulary_only_and_deleted_entries_do_not_replace() {
        let mut gone = entry("velho", Some("NOVO"), false);
        gone.is_deleted = true;
        let p = pp(&[entry("Kubernetes", None, false), gone]);
        assert_eq!(
            run(&p, "kubernetes velho", TextFormat::Raw, None).text,
            "kubernetes velho"
        );
    }

    #[test]
    fn whole_utterance_snippet_returns_only_the_expansion() {
        let mut snip = entry("meu endereço", Some("Rua A, 10 — Centro"), true);
        snip.replacement_html = Some("<b>Rua A, 10</b>".into());
        let p = pp(&[snip, entry("assinatura", Some("Abraço, Ana"), true)]);

        let r = run(&p, "Meu endereço.", TextFormat::Lowercase, None);
        assert_eq!(r.text, "Rua A, 10 — Centro");
        assert_eq!(r.html.as_deref(), Some("<b>Rua A, 10</b>"));
        // Formato minúsculo não mexe na expansão.
        assert_eq!(
            run(&p, "assinatura", TextFormat::Lowercase, None).text,
            "Abraço, Ana"
        );
        // No meio da frase o snippet é uma substituição comum, sem HTML.
        let inline = run(&p, "anota meu endereço aqui", TextFormat::Raw, None);
        assert_eq!(inline.text, "anota Rua A, 10 — Centro aqui");
        assert_eq!(inline.html, None);
    }

    #[test]
    fn non_snippet_entry_is_not_a_whole_utterance_trigger() {
        let p = pp(&[entry("virp", Some("Vyrp"), false)]);
        assert_eq!(run(&p, "virp", TextFormat::Normal, None).text, "Vyrp");
    }

    #[test]
    fn formats_raw_lowercase_normal() {
        let p = pp(&[entry("virp", Some("Vyrp"), false)]);
        let raw = "Olá   mundo, o VIRP chegou";
        assert_eq!(
            run(&p, raw, TextFormat::Raw, None).text,
            "Olá   mundo, o Vyrp chegou"
        );
        assert_eq!(
            run(&p, raw, TextFormat::Lowercase, None).text,
            "olá   mundo, o Vyrp chegou"
        );
        assert_eq!(
            run(&p, raw, TextFormat::Normal, None).text,
            "Olá mundo, o Vyrp chegou"
        );
        assert_eq!(
            run(&p, "hello   WORLD", TextFormat::Lowercase, None).text,
            "hello   world"
        );
    }

    #[test]
    fn capital_only_at_sentence_start() {
        let p = pp(&[]);
        let cases = [
            (None, "oi, tudo bem", "Oi, tudo bem"),
            (Some(""), "oi", "Oi"),
            (Some("Olá. "), "tudo bem?", "Tudo bem?"),
            (Some("Pronto!\n"), "ótimo", "Ótimo"),
            (Some("Eu fui para"), "casa", " casa"),
            (Some("Eu fui para "), "casa", "casa"),
        ];
        for (before, raw, want) in cases {
            assert_eq!(
                run(&p, raw, TextFormat::Normal, before).text,
                want,
                "{before:?} + {raw:?}"
            );
        }
    }

    #[test]
    fn leading_space_follows_the_text_before_the_cursor() {
        assert!(needs_leading_space(Some("palavra"), "outra"));
        assert!(needs_leading_space(Some("Fim."), "Começo"));
        assert!(!needs_leading_space(Some("palavra "), "outra"));
        assert!(!needs_leading_space(Some("linha\n"), "outra"));
        assert!(!needs_leading_space(Some("("), "dentro"));
        assert!(!needs_leading_space(Some("palavra"), ", vírgula"));
        assert!(!needs_leading_space(Some("palavra"), "."));
        assert!(!needs_leading_space(None, "início"));
        assert!(!needs_leading_space(Some("x"), ""));
        let p = pp(&[]);
        assert_eq!(
            run(&p, "mundo", TextFormat::Raw, Some("olá")).text,
            " mundo"
        );
        assert_eq!(run(&p, "", TextFormat::Raw, Some("olá")).text, "");
    }

    #[test]
    fn english_and_portuguese_sentences_round_trip() {
        let p = pp(&[entry("gee pee tee", Some("GPT"), false)]);
        let r = run(
            &p,
            "  i asked gee pee tee about it, press enter  ",
            TextFormat::Normal,
            None,
        );
        assert_eq!(r.text, "I asked GPT about it");
        assert!(r.press_enter);
        let r = run(
            &p,
            "perguntei ao gee pee tee, aperte enter",
            TextFormat::Normal,
            Some("Ana: "),
        );
        assert_eq!(r.text, "perguntei ao GPT");
        assert!(r.press_enter);
    }

    #[test]
    fn voice_commands_through_the_whole_pipeline() {
        let p = pp(&[]);
        let r = run(
            &p,
            "primeira linha. Nova linha. segunda linha",
            TextFormat::Normal,
            None,
        );
        assert_eq!(r.text, "Primeira linha.\nSegunda linha");
        let r = run(&p, "hello new paragraph world", TextFormat::Normal, None);
        assert_eq!(r.text, "Hello\n\nWorld");
        let r = run(&p, "ok, nova linha, aperte enter", TextFormat::Normal, None);
        assert_eq!((r.text.as_str(), r.press_enter), ("Ok\n", true));
        let r = run(&p, "nova linha", TextFormat::Raw, Some("fim"));
        assert_eq!(r.text, "\n");
    }

    #[test]
    fn undo_command_returns_no_text() {
        let p = pp(&[]);
        for said in ["Apagar isso.", "scratch that"] {
            let r = run(&p, said, TextFormat::Normal, Some("algo"));
            assert!(r.undo_last, "{said:?}");
            assert_eq!(r.text, "");
            assert!(!r.press_enter);
        }
        // No meio de uma frase é só texto.
        let r = run(&p, "não vou apagar isso", TextFormat::Normal, None);
        assert!(!r.undo_last);
        assert_eq!(r.text, "Não vou apagar isso");
    }

    #[test]
    fn voice_commands_can_be_disabled_or_replaced() {
        let p = pp(&[]);
        let off = PostOptions {
            voice_commands: false,
            ..Default::default()
        };
        assert_eq!(p.process("a nova linha b", &off).text, "A nova linha b");
        assert!(!p.process("apagar isso", &off).undo_last);
        p.set_voice_commands(&[VoiceCommand {
            phrases: vec!["quebra".into()],
            action: VoiceAction::Insert("\n".into()),
        }]);
        assert_eq!(run(&p, "a quebra b", TextFormat::Raw, None).text, "a\nb");
        assert_eq!(
            run(&p, "a nova linha b", TextFormat::Raw, None).text,
            "a nova linha b"
        );
    }

    #[test]
    fn reload_picks_up_dictionary_changes() {
        let p = pp(&[entry("a", Some("1"), false)]);
        assert_eq!(run(&p, "a", TextFormat::Raw, None).text, "1");
        p.reload(&[entry("a", Some("2"), false)]);
        assert_eq!(run(&p, "a", TextFormat::Raw, None).text, "2");
        p.reload(&[]);
        assert_eq!(run(&p, "a", TextFormat::Raw, None).text, "a");
    }

    #[test]
    fn reload_from_the_real_database() {
        let db = Db::open_in_memory().unwrap();
        db.dictionary()
            .upsert(
                &storage::dictionary::NewDictionaryEntry {
                    phrase: "virp".into(),
                    replacement: Some("Vyrp".into()),
                    manual_entry: true,
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let p = PostProcessor::new();
        p.reload_from_db(&db).unwrap();
        assert_eq!(run(&p, "o virp", TextFormat::Raw, None).text, "o Vyrp");
    }
}
