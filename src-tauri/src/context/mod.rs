//! Contexto transitório: nunca grava texto, título ou URL em disco/log.
//! UIA fica numa única STA; o chamador espera no máximo 150 ms. Um provider COM
//! travado pode ocupar essa STA, mas não cria novas threads nem bloqueia o ditado.

mod rules;
#[cfg(windows)]
pub(crate) mod win;

use crate::core::types::AppContext;
pub use rules::{CategoryRule, ContextRules};
use std::sync::Arc;

pub trait ContextSource: Send + Sync {
    fn capture(&self) -> AppContext;
}

#[derive(Debug, Default)]
pub struct UiaContext;

impl ContextSource for UiaContext {
    fn capture(&self) -> AppContext {
        snapshot().context
    }
}

pub fn default_source() -> Arc<dyn ContextSource> {
    #[cfg(windows)]
    win::warm_up();
    Arc::new(UiaContext)
}

/// Fonte de diagnóstico restrita à janela que o teste abriu.
pub fn diag_source(hwnd: isize, title_contains: String) -> Arc<dyn ContextSource> {
    struct Diagnostic {
        hwnd: isize,
        title: String,
    }
    impl ContextSource for Diagnostic {
        fn capture(&self) -> AppContext {
            snapshot_target(self.hwnd, Some(&self.title)).context
        }
    }
    #[cfg(windows)]
    win::warm_up();
    Arc::new(Diagnostic {
        hwnd,
        title: title_contains,
    })
}

pub(crate) fn snapshot_target(hwnd: isize, title: Option<&str>) -> FocusSnapshot {
    #[cfg(windows)]
    {
        win::capture_target(hwnd, title.map(str::to_owned))
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, title);
        FocusSnapshot::default()
    }
}

/// Metadados internos para verificar inserção, sem alterar AppContext (V11/V13).
#[derive(Debug, Clone, Default)]
pub(crate) struct FocusSnapshot {
    pub context: AppContext,
    pub hwnd: isize,
    pub password: bool,
    pub text_range: bool,
    pub focus_id: Vec<i32>,
    /// V25 only: complete selection, never the normal truncated context.
    pub full_selection: Option<String>,
    pub selection_overflow: bool,
}

pub(crate) fn snapshot() -> FocusSnapshot {
    #[cfg(windows)]
    {
        win::capture()
    }
    #[cfg(not(windows))]
    {
        FocusSnapshot::default()
    }
}

pub fn init(app: &tauri::AppHandle) {
    rules::init(app);
    #[cfg(windows)]
    win::warm_up();
}

#[tauri::command]
pub fn context_rules() -> ContextRules {
    rules::get()
}

#[tauri::command]
pub fn set_context_rules(app: tauri::AppHandle, rules: ContextRules) -> Result<(), String> {
    rules::save(&app, rules)
}

/// Mantém palavras inteiras na borda cortada, sem cortar UTF-8. Espaços do lado
/// do cursor não são aparados: são relevantes para a inserção.
pub(crate) fn truncate(text: &str, max: usize, suffix: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.into();
    }
    if max == 0 {
        return String::new();
    }
    if suffix {
        let mut start = chars.len() - max;
        if !chars[start - 1].is_whitespace() && !chars[start].is_whitespace() {
            if let Some(offset) = chars[start..].iter().position(|c| c.is_whitespace()) {
                start += offset + 1;
            }
        }
        chars[start..].iter().collect()
    } else {
        let mut end = max;
        if !chars[end - 1].is_whitespace() && !chars[end].is_whitespace() {
            if let Some(offset) = chars[..end].iter().rposition(|c| c.is_whitespace()) {
                end = offset;
            }
        }
        chars[..end].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_cuts_only_outer_edge_and_preserves_unicode() {
        assert_eq!(truncate("ação final ", 8, true), "final ");
        assert_eq!(truncate(" início enorme", 9, false), " início");
        assert_eq!(truncate("🦀🦀🦀", 2, false), "🦀🦀");
        assert_eq!(truncate(" a ", 3, true), " a ");
        assert_eq!(truncate("texto", 0, false), "");
    }
}
