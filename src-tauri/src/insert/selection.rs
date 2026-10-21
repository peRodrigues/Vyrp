//! V25: roda na MESMA STA do clipboard/insert; nenhuma chamada concorrente ao clipboard.
use super::*;
use crate::privacy::{self, Privacy};
use windows_sys::Win32::System::DataExchange::{GetClipboardData, GetClipboardOwner};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

#[derive(Clone)]
pub(crate) struct Selection {
    pub id: String,
    pub text: String,
    snapshot: FocusSnapshot,
    pid: u32,
    title: String,
    diag: Option<DiagTarget>,
}

impl Selection {
    pub fn permitted(&self, privacy: &Privacy) -> Result<(), String> {
        let current = context::win::selection_target(self.snapshot.hwnd, Some(self.title.clone()));
        check_snapshot(self, &current, privacy)
    }
}

pub(crate) fn undo(id: String) -> InsertOutcome {
    submit(Operation::UndoTransform(id), None)
}

pub(crate) fn selected(privacy: Privacy, diag: Option<DiagTarget>) -> Result<Selection, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    let result = submit(Operation::Selection(privacy, tx), diag);
    if !result.ok {
        return Err("seleção indisponível ou bloqueada".into());
    }
    rx.try_recv()
        .map_err(|_| "captura interrompida".to_string())?
}

pub(crate) fn replace(selection: Selection, text: String, privacy: Privacy) -> InsertOutcome {
    let diag = selection.diag.clone();
    submit(
        Operation::Replace(
            Box::new(selection),
            InsertRequest {
                text,
                html: None,
                method: InsertMethod::Paste,
                press_enter: false,
            },
            privacy,
        ),
        diag,
    )
}

fn allowed(snapshot: &FocusSnapshot, privacy: &Privacy) -> Result<(), String> {
    if snapshot.hwnd == 0
        || snapshot.focus_id.is_empty()
        || !snapshot.context.editable
        || snapshot.context.exe.is_empty()
        || snapshot.password
        || snapshot.selection_overflow
        || privacy::check(privacy, &snapshot.context).is_some()
    {
        return Err("campo desconhecido, protegido, bloqueado ou seleção grande demais".into());
    }
    // A browser whose address is unavailable cannot prove it is outside the deny-list.
    if matches!(
        snapshot.context.exe.as_str(),
        "msedge.exe" | "chrome.exe" | "firefox.exe" | "brave.exe"
    ) && snapshot.context.url.is_none()
        && (privacy.block_defaults || !privacy.domains.is_empty())
    {
        return Err("URL indisponível para conferir privacidade".into());
    }
    Ok(())
}

pub(super) fn capture(
    owner: HWND,
    state: &mut State,
    job: &Job,
    privacy: &Privacy,
) -> Result<Selection, String> {
    let target = Target::acquire(job.diag.as_ref())?;
    if elevated_target(target.pid) != Some(false) {
        return Err("integridade do alvo desconhecida ou elevada".into());
    }
    if !wait_modifiers((Instant::now() + Duration::from_millis(600)).min(job.deadline)) {
        return Err("modificadores pressionados".into());
    }
    let snapshot = context::win::selection_target(
        target.hwnd,
        job.diag.as_ref().map(|d| d.title_contains.clone()),
    );
    allowed(&snapshot, privacy)?;
    let title = snapshot.context.title.clone();
    let text = match &snapshot.full_selection {
        Some(text) => text.clone(),
        None => copy_selection(owner, state, &target, job.deadline)?,
    };
    if text.trim().is_empty()
        || text.chars().count() > crate::llm::transform::MAX_SELECTION
        || text.contains('\0')
    {
        return Err("seleção vazia ou grande demais".into());
    }
    target.check()?;
    Ok(Selection {
        id: uuid::Uuid::new_v4().to_string(),
        text,
        snapshot,
        pid: target.pid,
        title,
        diag: job.diag.clone(),
    })
}

pub(super) fn check_snapshot(
    selection: &Selection,
    current: &FocusSnapshot,
    privacy: &Privacy,
) -> Result<(), String> {
    allowed(current, privacy)?;
    if current.hwnd != selection.snapshot.hwnd
        || current.focus_id != selection.snapshot.focus_id
        || current.context != selection.snapshot.context
    {
        return Err("foco, campo ou seleção mudou; transformação abortada".into());
    }
    Ok(())
}

pub(super) fn validate_current(
    owner: HWND,
    state: &mut State,
    job: &Job,
    selection: &Selection,
    privacy: &Privacy,
) -> Result<(), String> {
    let target = Target {
        hwnd: selection.snapshot.hwnd,
        pid: selection.pid,
        diag: Some(DiagTarget {
            hwnd: selection.snapshot.hwnd,
            title_contains: selection.title.clone(),
        }),
    };
    // Never activate the old window: the user must still be working in it.
    target.check()?;
    let current = context::win::selection_target(target.hwnd, Some(selection.title.clone()));
    check_snapshot(selection, &current, privacy)?;
    let selected = match current.full_selection {
        Some(text) => text,
        None => copy_selection(owner, state, &target, job.deadline)?,
    };
    if selected != selection.text {
        return Err("seleção mudou; substituição abortada".into());
    }
    target.check()
}

fn copy_selection(
    owner: HWND,
    state: &mut State,
    target: &Target,
    deadline: Instant,
) -> Result<String, String> {
    let open = clipboard::Open::acquire(owner, deadline)?;
    let original = clipboard::Snapshot::capture(&open, deadline)?;
    drop(open);
    let prior = clipboard::sequence();
    target.keys(
        &[
            (VK_CONTROL, false),
            (0x43, false),
            (0x43, true),
            (VK_CONTROL, true),
        ],
        deadline,
    )?;
    let wait_until = (Instant::now() + Duration::from_millis(300)).min(deadline);
    while clipboard::sequence() == prior && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(10));
    }
    if clipboard::sequence() == prior {
        return Err("Ctrl+C não publicou seleção".into());
    }
    let sequence = clipboard::sequence();
    let source = unsafe { GetClipboardOwner() };
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(source, &mut pid);
    }
    if source.is_null() || pid != target.pid {
        return Err("clipboard mudou em outro processo; preservado".into());
    }
    let result = match clipboard::Open::acquire(owner, deadline) {
        Ok(open) => {
            if clipboard::sequence() != sequence || unsafe { GetClipboardOwner() } != source {
                return Err("clipboard mudou; preservado".into());
            }
            let text = read_text();
            original.restore(&open)?;
            text
        }
        Err(e) => {
            state.selection_restore = Some((sequence, source as isize, original));
            Err(e)
        }
    };
    target.check()?;
    result
}

fn read_text() -> Result<String, String> {
    // SAFETY: clipboard is open on this STA; lock bounds checked before reading.
    unsafe {
        let handle = GetClipboardData(13);
        if handle.is_null() {
            return Err("clipboard sem texto Unicode".into());
        }
        let bytes = GlobalSize(handle);
        if !(2..=2 * (crate::llm::transform::MAX_SELECTION + 1)).contains(&bytes) {
            return Err("seleção grande demais".into());
        }
        let ptr = GlobalLock(handle).cast::<u16>();
        if ptr.is_null() {
            return Err("clipboard ilegível".into());
        }
        let data = std::slice::from_raw_parts(ptr, bytes / 2);
        let end = data.iter().position(|v| *v == 0);
        let text = end.map(|end| String::from_utf16_lossy(&data[..end]));
        GlobalUnlock(handle);
        text.ok_or_else(|| "clipboard sem terminador".into())
    }
}

pub(super) fn retry_restore(owner: HWND, state: &mut State) {
    let Some((sequence, source, _)) = state.selection_restore.as_ref() else {
        return;
    };
    if clipboard::sequence() != *sequence || unsafe { GetClipboardOwner() } as isize != *source {
        state.selection_restore = None;
        return;
    }
    let Ok(open) = clipboard::Open::acquire(owner, Instant::now() + Duration::from_millis(10))
    else {
        return;
    };
    if let Some((sequence, source, original)) = state.selection_restore.take() {
        if clipboard::sequence() == sequence
            && unsafe { GetClipboardOwner() } as isize == source
            && original.restore(&open).is_err()
        {
            tracing::error!("falha ao restaurar clipboard da seleção");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_password_blocklist_and_stale_selection_fail_closed() {
        let mut snapshot = FocusSnapshot {
            hwnd: 1,
            focus_id: vec![1],
            ..Default::default()
        };
        snapshot.context.exe = "editor.exe".into();
        snapshot.context.editable = true;
        let privacy = Privacy::default();
        let selected = Selection {
            id: "test".into(),
            text: "abc".into(),
            snapshot: snapshot.clone(),
            pid: 1,
            title: "test".into(),
            diag: None,
        };
        assert!(check_snapshot(&selected, &snapshot, &privacy).is_ok());
        snapshot.password = true;
        assert!(check_snapshot(&selected, &snapshot, &privacy).is_err());
        snapshot.password = false;
        snapshot.context.selection = "new".into();
        assert!(check_snapshot(&selected, &snapshot, &privacy).is_err());
        snapshot = selected.snapshot.clone();
        snapshot.context.exe = "keepass.exe".into();
        assert!(allowed(&snapshot, &privacy).is_err());
        assert!(allowed(&FocusSnapshot::default(), &privacy).is_err());
    }
}
