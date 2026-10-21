//! Inserção serializada numa STA, com clipboard transacional e foco conferido
//! imediatamente antes de cada SendInput. As fachadas V11 permanecem estáveis.
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub(crate) use win::selection;

use crate::core::types::{InsertMethod, InsertOutcome, InsertRequest};
use std::sync::Arc;

pub trait Inserter: Send + Sync {
    fn insert(&self, req: &InsertRequest) -> InsertOutcome;
    fn undo_last(&self) -> InsertOutcome;
    /// Só copia o texto (e o HTML) para o clipboard, sem colar e sem tocar no teclado;
    /// excluído do histórico/nuvem do clipboard. Usado quando colar foi bloqueado ou
    /// falhou ("Copiado. Cole manualmente.").
    fn copy_text(&self, req: &InsertRequest) -> InsertOutcome {
        fail(req.method, "este inseridor não copia")
    }
}

/// HWND da janela aberta pelo próprio teste e marcador ÚNICO do seu título.
/// O teste deve obter o handle ao abrir, nunca usar a janela casualmente em foco.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagTarget {
    pub hwnd: isize,
    pub title_contains: String,
}

#[derive(Debug, Default)]
pub struct ClipboardInserter {
    pub diag_target: Option<DiagTarget>,
}

impl Inserter for ClipboardInserter {
    fn insert(&self, req: &InsertRequest) -> InsertOutcome {
        #[cfg(windows)]
        {
            win::submit(
                win::Operation::Insert(req.clone()),
                self.diag_target.clone(),
            )
        }
        #[cfg(not(windows))]
        {
            fail(req.method, "inserção só existe no Windows")
        }
    }
    fn undo_last(&self) -> InsertOutcome {
        #[cfg(windows)]
        {
            win::submit(win::Operation::Undo, self.diag_target.clone())
        }
        #[cfg(not(windows))]
        {
            fail(InsertMethod::Paste, "inserção só existe no Windows")
        }
    }
    fn copy_text(&self, req: &InsertRequest) -> InsertOutcome {
        #[cfg(windows)]
        {
            win::submit(win::Operation::Copy(req.clone()), None)
        }
        #[cfg(not(windows))]
        {
            fail(req.method, "clipboard só existe no Windows")
        }
    }
}

pub fn default_inserter() -> Arc<dyn Inserter> {
    Arc::new(ClipboardInserter::default())
}
pub fn diag_inserter(target: DiagTarget) -> Arc<dyn Inserter> {
    Arc::new(ClipboardInserter {
        diag_target: Some(target),
    })
}

/// Reinsere no foco atual; não repete o Enter do ditado original.
pub fn paste_last() -> InsertOutcome {
    #[cfg(windows)]
    {
        win::submit(win::Operation::PasteLast, None)
    }
    #[cfg(not(windows))]
    {
        fail(InsertMethod::Paste, "inserção só existe no Windows")
    }
}

/// Cópia explícita: deixa o último texto no clipboard, excluído do histórico/nuvem.
pub fn copy_last() -> InsertOutcome {
    #[cfg(windows)]
    {
        win::submit(win::Operation::CopyLast, None)
    }
    #[cfg(not(windows))]
    {
        fail(InsertMethod::Paste, "clipboard só existe no Windows")
    }
}

#[tauri::command]
pub async fn insert_copy_last() -> Result<InsertOutcome, String> {
    tauri::async_runtime::spawn_blocking(copy_last)
        .await
        .map_err(|e| e.to_string())
}

fn fail(method: InsertMethod, reason: impl Into<String>) -> InsertOutcome {
    InsertOutcome {
        ok: false,
        method,
        verified: None,
        reason: Some(reason.into()),
    }
}

fn html_fragment(fragment: &str) -> String {
    let prefix = "<html><body><!--StartFragment-->";
    let suffix = "<!--EndFragment--></body></html>";
    let header = |start, end, from, to| {
        format!(
            "Version:0.9\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{from:010}\r\nEndFragment:{to:010}\r\n"
        )
    };
    let start = header(0, 0, 0, 0).len();
    let from = start + prefix.len();
    let to = from + fragment.len(); // CF_HTML offsets are BYTES, not characters.
    format!(
        "{}{prefix}{fragment}{suffix}",
        header(start, to + suffix.len(), from, to)
    )
}

/// Terminal (cola com Shift+Insert) pela classe da janela em primeiro plano: console
/// clássico, Windows Terminal (também quando hospeda cmd/PowerShell), ConEmu e mintty.
/// O exe só decide para terminais de classe genérica (WezTerm, Alacritty).
fn is_terminal(class: &str, exe: &str) -> bool {
    const CLASSES: [&str; 4] = [
        "ConsoleWindowClass",
        "CASCADIA_HOSTING_WINDOW_CLASS",
        "VirtualConsoleClass",
        "mintty",
    ];
    const EXES: [&str; 2] = ["wezterm-gui.exe", "alacritty.exe"];
    CLASSES.iter().any(|c| class.eq_ignore_ascii_case(c))
        || EXES.iter().any(|e| exe.eq_ignore_ascii_case(e))
}

fn verified(
    before: &crate::context::FocusSnapshot,
    after: &crate::context::FocusSnapshot,
    text: &str,
) -> Option<bool> {
    if !before.text_range
        || !after.text_range
        || before.hwnd != after.hwnd
        || before.focus_id.is_empty()
        || before.focus_id != after.focus_id
        || after.password
    {
        return None;
    }
    let normalize = |s: &str| s.replace("\r\n", "\n").replace('\r', "\n");
    let expected = normalize(&format!("{}{text}", before.context.before));
    let observed = normalize(&after.context.before);
    // A suffix alone that was already present is not evidence of a new insertion.
    let changed =
        before.context.before != after.context.before || !before.context.selection.is_empty();
    let tail: String = expected
        .chars()
        .rev()
        .take(128)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(
        changed
            && !tail.is_empty()
            && observed.ends_with(&tail)
            && after.context.selection.is_empty(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_offsets_are_utf8_byte_positions() {
        let html = html_fragment("<b>ação 🦀</b>");
        let offset = |key: &str| {
            html.lines()
                .find_map(|l| l.strip_prefix(key))
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        assert_eq!(
            &html[offset("StartFragment:")..offset("EndFragment:")],
            "<b>ação 🦀</b>"
        );
        assert_eq!(offset("EndHTML:"), html.len());
    }
    #[test]
    fn verification_does_not_accept_preexisting_text_or_changed_focus() {
        let mut before = crate::context::FocusSnapshot {
            hwnd: 1,
            focus_id: vec![1],
            text_range: true,
            ..Default::default()
        };
        before.context.before = "already ".into();
        let mut after = before.clone();
        assert_eq!(verified(&before, &after, "already "), Some(false));
        after.context.before.push_str("new 🦀");
        assert_eq!(verified(&before, &after, "new 🦀"), Some(true));
        after.focus_id = vec![2];
        assert_eq!(verified(&before, &after, "new 🦀"), None);
    }
    #[test]
    fn terminal_is_detected_by_window_class() {
        assert!(is_terminal(
            "CASCADIA_HOSTING_WINDOW_CLASS",
            "WindowsTerminal.exe"
        ));
        assert!(is_terminal("ConsoleWindowClass", "conhost.exe"));
        assert!(is_terminal("VirtualConsoleClass", "ConEmu64.exe"));
        assert!(is_terminal("Window Class", "wezterm-gui.exe"));
        // O exe sozinho não basta: o PowerShell pode ser dono de um formulário WinForms.
        assert!(!is_terminal(
            "WindowsForms10.Window.8.app.0.1",
            "powershell.exe"
        ));
        assert!(!is_terminal(
            "Chrome_WidgetWin_1",
            "not-windowsterminal.exe"
        ));
    }
}
