//! Deny-list e privacidade (V21): apps (exe) e sites (domínios) onde o Vyrp não ouve nem cola.
//!
//! - [`Privacy`] mora no Settings (campo aditivo, com padrões; sem bump de versão).
//! - [`check`] compara o contexto do app em foco com a lista: exe pelo nome do arquivo (sem
//!   caminho, sem `.exe`, sem diferenciar caixa) e site pelo host da URL (o próprio domínio e
//!   qualquer subdomínio). Gerenciadores de senha conhecidos vêm bloqueados por padrão.
//! - [`guard_inserter`] embrulha o `Inserter`: se o foco, **na hora de colar**, estiver num
//!   app bloqueado, nada é colado e o resultado traz [`BLOCKED_REASON`].
//!
//! O contexto da tela (`before`/`selection`/`after`) nunca é gravado: `HistoryEntry` nem tem
//! campo para ele.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::context::ContextSource;
use crate::core::types::{AppContext, InsertOutcome, InsertRequest};
use crate::insert::Inserter;
use crate::settings::SettingsStore;

/// Motivo (texto fixo) que o `Inserter` protegido devolve em `InsertOutcome.reason`.
pub const BLOCKED_REASON: &str = "bloqueado: app ou site na lista de privacidade";

/// Gerenciadores de senha conhecidos (nome do exe, sem `.exe`).
pub const DEFAULT_APPS: [&str; 11] = [
    "1password",
    "keepass",
    "keepassxc",
    "bitwarden",
    "lastpass",
    "dashlane",
    "enpass",
    "nordpass",
    "roboform",
    "keeper",
    "protonpass",
];

/// Sites de gerenciadores de senha (valem também os subdomínios, ex.: `vault.bitwarden.com`).
pub const DEFAULT_DOMAINS: [&str; 8] = [
    "1password.com",
    "bitwarden.com",
    "lastpass.com",
    "dashlane.com",
    "keepersecurity.com",
    "nordpass.com",
    "enpass.io",
    "roboform.com",
];

const MAX_ENTRIES: usize = 100;
const MAX_ENTRY_LEN: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// Bloquear os gerenciadores de senha conhecidos ([`DEFAULT_APPS`], [`DEFAULT_DOMAINS`]).
    pub block_defaults: bool,
    /// Apps extras bloqueados (nome do exe, ex.: `banco.exe`).
    pub apps: Vec<String>,
    /// Sites extras bloqueados (domínio, ex.: `meubanco.com.br`).
    pub domains: Vec<String>,
    /// Guardar o áudio de cada ditado no histórico. Desligado por padrão.
    pub save_audio: bool,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            block_defaults: true,
            apps: Vec::new(),
            domains: Vec::new(),
            save_audio: false,
        }
    }
}

/// `C:\Programas\KeePass\KeePass.exe` → `keepass`. Vazio/estranho → `None`.
pub fn normalize_app(input: &str) -> Option<String> {
    let name = input
        .trim()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let name = name
        .strip_suffix(".exe")
        .unwrap_or(&name)
        .trim()
        .to_string();
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_ENTRY_LEN
        && !name
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '<' || c == '>');
    valid.then_some(name)
}

/// Host em minúsculas de uma URL ou domínio digitado: sem esquema, usuário, porta, caminho,
/// `www.` ou `*.`. `https://www.Banco.com:8443/login?x` → `banco.com`.
pub fn host_of(input: &str) -> Option<String> {
    let s = input.trim().to_lowercase();
    let s = s.split_once("://").map_or(s.as_str(), |(_, rest)| rest);
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit('@').next().unwrap_or("");
    let s = s.split(':').next().unwrap_or("");
    let s = s.trim_matches('.');
    let s = s.strip_prefix("*.").unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let valid = !s.is_empty()
        && s.chars().count() <= MAX_ENTRY_LEN
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        && !s.contains("..");
    valid.then(|| s.to_string())
}

fn dedupe(list: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        if !out.contains(&item) {
            out.push(item);
        }
        if out.len() >= MAX_ENTRIES {
            break;
        }
    }
    out
}

impl Privacy {
    /// Normaliza as listas do usuário (descarta o inválido e o repetido, limita o tamanho).
    pub fn normalized(mut self) -> Self {
        self.apps = dedupe(self.apps.iter().filter_map(|a| normalize_app(a)));
        self.domains = dedupe(self.domains.iter().filter_map(|d| host_of(d)));
        self
    }

    /// Há alguma regra ativa? (Sem regra, nem vale capturar o contexto.)
    pub fn has_rules(&self) -> bool {
        self.block_defaults || !self.apps.is_empty() || !self.domains.is_empty()
    }
}

/// Por que o contexto foi bloqueado (nome da regra, nunca título/URL da tela).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    App(String),
    Domain(String),
    /// Campo de senha em foco (UIA `IsPassword`): bloqueado sempre, com ou sem lista.
    Password,
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocked::App(name) => write!(f, "app {name}"),
            Blocked::Domain(domain) => write!(f, "site {domain}"),
            Blocked::Password => write!(f, "campo de senha"),
        }
    }
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// O contexto está na lista? `None` = livre para ditar e colar.
pub fn check(privacy: &Privacy, ctx: &AppContext) -> Option<Blocked> {
    if ctx.password {
        return Some(Blocked::Password);
    }
    if !privacy.has_rules() {
        return None;
    }
    if let Some(exe) = normalize_app(&ctx.exe) {
        let defaults = privacy
            .block_defaults
            .then_some(DEFAULT_APPS.iter().copied())
            .into_iter()
            .flatten();
        let hit = defaults
            .chain(privacy.apps.iter().map(String::as_str))
            .find(|rule| *rule == exe);
        if let Some(rule) = hit {
            return Some(Blocked::App(rule.to_string()));
        }
    }
    if let Some(host) = ctx.url.as_deref().and_then(host_of) {
        let defaults = privacy
            .block_defaults
            .then_some(DEFAULT_DOMAINS.iter().copied())
            .into_iter()
            .flatten();
        let hit = defaults
            .chain(privacy.domains.iter().map(String::as_str))
            .find(|rule| host_matches(&host, rule));
        if let Some(rule) = hit {
            return Some(Blocked::Domain(rule.to_string()));
        }
    }
    None
}

/// `Inserter` que confere o foco **atual** contra a lista antes de colar.
pub struct GuardedInserter {
    inner: Arc<dyn Inserter>,
    context: Arc<dyn ContextSource>,
    rules: Arc<dyn Fn() -> Privacy + Send + Sync>,
}

impl GuardedInserter {
    pub fn new(
        inner: Arc<dyn Inserter>,
        context: Arc<dyn ContextSource>,
        rules: Arc<dyn Fn() -> Privacy + Send + Sync>,
    ) -> Self {
        Self {
            inner,
            context,
            rules,
        }
    }
}

impl Inserter for GuardedInserter {
    fn insert(&self, req: &InsertRequest) -> InsertOutcome {
        let privacy = (self.rules)();
        if privacy.has_rules()
            && let Some(blocked) = check(&privacy, &self.context.capture())
        {
            tracing::info!(%blocked, "inserção bloqueada pela lista de privacidade");
            // Não cola, mas deixa o texto no clipboard (fora do histórico/nuvem) para a
            // pessoa colar onde quiser; o overlay avisa.
            let copied = self.inner.copy_text(req);
            tracing::info!(
                copied = copied.ok,
                "texto do ditado bloqueado copiado para o clipboard"
            );
            return InsertOutcome {
                ok: false,
                method: req.method,
                verified: None,
                reason: Some(BLOCKED_REASON.into()),
            };
        }
        self.inner.insert(req)
    }

    fn undo_last(&self) -> InsertOutcome {
        self.inner.undo_last()
    }
}

/// Embrulha o inseridor do app com as regras vivas do Settings (mudanças valem no próximo ditado).
pub fn guard_inserter(
    app: AppHandle,
    inner: Arc<dyn Inserter>,
    context: Arc<dyn ContextSource>,
) -> Arc<dyn Inserter> {
    let rules = Arc::new(move || {
        app.try_state::<SettingsStore>()
            .map(|s| s.get().privacy)
            .unwrap_or_default()
    });
    Arc::new(GuardedInserter::new(inner, context, rules))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::InsertMethod;
    use std::sync::Mutex;

    fn ctx(exe: &str, url: Option<&str>) -> AppContext {
        AppContext {
            exe: exe.into(),
            url: url.map(Into::into),
            ..AppContext::default()
        }
    }

    #[test]
    fn app_names_are_normalized() {
        assert_eq!(normalize_app("KeePass.exe").as_deref(), Some("keepass"));
        assert_eq!(
            normalize_app(r"C:\Program Files\KeePass\KeePass.EXE").as_deref(),
            Some("keepass")
        );
        assert_eq!(normalize_app(" 1Password ").as_deref(), Some("1password"));
        assert_eq!(normalize_app("/usr/bin/foo").as_deref(), Some("foo"));
        assert_eq!(normalize_app("   "), None);
        assert_eq!(normalize_app(".exe"), None);
        assert_eq!(normalize_app("a\"b"), None);
        assert_eq!(normalize_app(&"x".repeat(500)), None);
    }

    #[test]
    fn hosts_are_extracted_from_urls_and_domains() {
        for (input, want) in [
            ("https://www.Banco.com:8443/login?x=1#a", Some("banco.com")),
            (
                "http://user:senha@vault.bitwarden.com/",
                Some("vault.bitwarden.com"),
            ),
            ("*.exemplo.com.br", Some("exemplo.com.br")),
            ("www.exemplo.com", Some("exemplo.com")),
            ("exemplo.com/caminho", Some("exemplo.com")),
            ("", None),
            ("https://", None),
            ("a b.com", None),
            ("a..com", None),
        ] {
            assert_eq!(host_of(input).as_deref(), want, "{input}");
        }
    }

    #[test]
    fn defaults_block_known_password_managers_by_app_and_site() {
        let p = Privacy::default();
        assert_eq!(
            check(&p, &ctx(r"C:\x\1Password.exe", None)),
            Some(Blocked::App("1password".into()))
        );
        assert_eq!(
            check(&p, &ctx("KeePassXC.exe", None)),
            Some(Blocked::App("keepassxc".into()))
        );
        assert_eq!(
            check(
                &p,
                &ctx("chrome.exe", Some("https://vault.bitwarden.com/#/login"))
            ),
            Some(Blocked::Domain("bitwarden.com".into()))
        );
        // Domínio parecido não conta.
        assert_eq!(
            check(&p, &ctx("chrome.exe", Some("https://notbitwarden.com"))),
            None
        );
        assert_eq!(check(&p, &ctx("notepad.exe", None)), None);
    }

    #[test]
    fn defaults_can_be_turned_off() {
        let p = Privacy {
            block_defaults: false,
            ..Privacy::default()
        };
        assert!(!p.has_rules());
        assert_eq!(check(&p, &ctx("1password.exe", None)), None);
    }

    #[test]
    fn user_lists_add_to_the_defaults() {
        let p = Privacy {
            apps: vec!["banco.exe".into()],
            domains: vec!["https://www.meubanco.com.br/entrar".into()],
            ..Privacy::default()
        }
        .normalized();
        assert_eq!(p.apps, ["banco"]);
        assert_eq!(p.domains, ["meubanco.com.br"]);
        assert_eq!(
            check(&p, &ctx("Banco.exe", None)),
            Some(Blocked::App("banco".into()))
        );
        assert_eq!(
            check(&p, &ctx("edge.exe", Some("https://app.meubanco.com.br/x"))),
            Some(Blocked::Domain("meubanco.com.br".into()))
        );
        assert!(check(&p, &ctx("1password.exe", None)).is_some());
    }

    #[test]
    fn user_rules_work_even_with_defaults_off() {
        let p = Privacy {
            block_defaults: false,
            apps: vec!["segredo".into()],
            ..Privacy::default()
        };
        assert!(p.has_rules());
        assert!(check(&p, &ctx("segredo.exe", None)).is_some());
        assert!(check(&p, &ctx("1password.exe", None)).is_none());
    }

    #[test]
    fn normalization_dedupes_drops_junk_and_caps() {
        let p = Privacy {
            apps: vec!["A.exe".into(), "a".into(), "  ".into(), "B".into()],
            domains: vec!["X.com".into(), "https://x.com/".into(), "###".into()],
            ..Privacy::default()
        }
        .normalized();
        assert_eq!(p.apps, ["a", "b"]);
        assert_eq!(p.domains, ["x.com"]);
        let many = Privacy {
            apps: (0..500).map(|i| format!("app{i}")).collect(),
            ..Privacy::default()
        }
        .normalized();
        assert_eq!(many.apps.len(), MAX_ENTRIES);
    }

    #[test]
    fn audio_in_history_is_off_by_default_and_old_settings_load() {
        assert!(!Privacy::default().save_audio);
        let p: Privacy = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Privacy::default());
        let p: Privacy = serde_json::from_str(r#"{"apps":["x"],"save_audio":true}"#).unwrap();
        assert!(p.save_audio && p.block_defaults);
    }

    // ------------------------------------------------------------ inseridor protegido

    struct FakeContext(Mutex<AppContext>);
    impl ContextSource for FakeContext {
        fn capture(&self) -> AppContext {
            self.0.lock().unwrap().clone()
        }
    }

    /// `.0` = colados, `.1` = copiados.
    #[derive(Default)]
    struct FakeInserter(Mutex<Vec<String>>, Mutex<Vec<String>>);
    impl Inserter for FakeInserter {
        fn copy_text(&self, req: &InsertRequest) -> InsertOutcome {
            self.1.lock().unwrap().push(req.text.clone());
            InsertOutcome {
                ok: true,
                method: req.method,
                verified: None,
                reason: None,
            }
        }
        fn insert(&self, req: &InsertRequest) -> InsertOutcome {
            self.0.lock().unwrap().push(req.text.clone());
            InsertOutcome {
                ok: true,
                method: req.method,
                verified: None,
                reason: None,
            }
        }
        fn undo_last(&self) -> InsertOutcome {
            InsertOutcome {
                ok: true,
                method: InsertMethod::Paste,
                verified: None,
                reason: None,
            }
        }
    }

    fn request(text: &str) -> InsertRequest {
        InsertRequest {
            text: text.into(),
            ..InsertRequest::default()
        }
    }

    #[test]
    fn guarded_inserter_pastes_in_normal_apps_and_refuses_in_blocked_ones() {
        let inner = Arc::new(FakeInserter::default());
        let context = Arc::new(FakeContext(Mutex::new(ctx("notepad.exe", None))));
        let guarded =
            GuardedInserter::new(inner.clone(), context.clone(), Arc::new(Privacy::default));
        assert!(guarded.insert(&request("olá")).ok);
        assert_eq!(inner.0.lock().unwrap().as_slice(), ["olá"]);

        // O foco muda para o gerenciador de senhas entre a fala e a colagem.
        *context.0.lock().unwrap() = ctx("1Password.exe", None);
        let outcome = guarded.insert(&request("segredo ditado"));
        assert!(!outcome.ok);
        assert_eq!(outcome.reason.as_deref(), Some(BLOCKED_REASON));
        assert_eq!(inner.0.lock().unwrap().len(), 1, "nada foi colado");
        assert_eq!(
            inner.1.lock().unwrap().as_slice(),
            ["segredo ditado"],
            "mas ficou no clipboard para colar à mão"
        );
    }

    #[test]
    fn password_field_is_blocked_even_without_rules() {
        let none = Privacy {
            block_defaults: false,
            ..Privacy::default()
        };
        assert!(!none.has_rules());
        let field = AppContext {
            exe: "notepad.exe".into(),
            password: true,
            ..AppContext::default()
        };
        assert_eq!(check(&none, &field), Some(Blocked::Password));
        assert_eq!(check(&Privacy::default(), &field), Some(Blocked::Password));
        assert_eq!(Blocked::Password.to_string(), "campo de senha");
        assert_eq!(check(&none, &ctx("notepad.exe", None)), None);
    }

    #[test]
    fn guarded_inserter_follows_live_settings_and_skips_capture_without_rules() {
        let inner = Arc::new(FakeInserter::default());
        let context = Arc::new(FakeContext(Mutex::new(ctx("1password.exe", None))));
        let live = Arc::new(Mutex::new(Privacy::default()));
        let rules = {
            let live = live.clone();
            Arc::new(move || live.lock().unwrap().clone())
        };
        let guarded = GuardedInserter::new(inner.clone(), context, rules);
        assert!(!guarded.insert(&request("a")).ok);
        live.lock().unwrap().block_defaults = false;
        assert!(guarded.insert(&request("b")).ok, "regra desligada vale já");
        assert_eq!(inner.0.lock().unwrap().as_slice(), ["b"]);
        assert!(guarded.undo_last().ok);
    }

    #[test]
    fn blocked_reasons_name_the_rule_not_the_screen() {
        assert_eq!(Blocked::App("keepass".into()).to_string(), "app keepass");
        assert_eq!(
            Blocked::Domain("bitwarden.com".into()).to_string(),
            "site bitwarden.com"
        );
    }
}
