use super::{DiagTarget, clipboard, fail};
use crate::context::{self, FocusSnapshot};
use crate::core::types::{InsertMethod, InsertOutcome, InsertRequest};
use crate::hotkey::gesture::KeyState;
use crate::hotkey::hook::{AsyncKeyState, send_keys};
use crate::hotkey::hook_logic::VYRP_INJECT_MAGIC;
use crate::hotkey::keys::{
    VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_RWIN,
};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetClassNameW, GetForegroundWindow,
    GetWindowThreadProcessId, HWND_MESSAGE, IsWindow, MSG, PM_REMOVE, PeekMessageW,
    SetForegroundWindow, TranslateMessage,
};

const BUDGET: Duration = Duration::from_millis(1500);
#[path = "selection.rs"]
pub(crate) mod selection;
const MODIFIERS: [u16; 8] = [
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LWIN,
    VK_RWIN,
];
pub enum Operation {
    UndoTransform(String),
    Selection(
        crate::privacy::Privacy,
        mpsc::SyncSender<Result<selection::Selection, String>>,
    ),
    Replace(
        Box<selection::Selection>,
        InsertRequest,
        crate::privacy::Privacy,
    ),
    Insert(InsertRequest),
    PasteLast,
    CopyLast,
    /// Copia um texto arbitrário (sem colar); vira o "último texto".
    Copy(InsertRequest),
    Undo,
}
struct Job {
    op: Operation,
    diag: Option<DiagTarget>,
    deadline: Instant,
    reply: mpsc::SyncSender<InsertOutcome>,
}
struct Worker {
    tx: mpsc::SyncSender<Job>,
    busy: Arc<AtomicBool>,
}
#[derive(Default)]
struct State {
    transform_undo: Option<String>,
    selection_restore: Option<(u32, isize, clipboard::Snapshot)>,
    last: Option<InsertRequest>,
    undo: Option<FocusSnapshot>,
    pending_restore: Option<(u32, clipboard::Snapshot)>,
}

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let active = busy.clone();
        let _ = std::thread::Builder::new()
            .name("vyrp-clipboard-sta".into())
            .spawn(move || {
                // SAFETY: a dedicated STA owns the clipboard window for its lifetime.
                // Keep it alive for explicit CopyLast, including delayed system formats.
                unsafe {
                    if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
                        return;
                    }
                    let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
                    let owner = CreateWindowExW(
                        0,
                        class.as_ptr(),
                        std::ptr::null(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        HWND_MESSAGE,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    );
                    if owner.is_null() {
                        return;
                    }
                    let mut state = State::default();
                    loop {
                        retry_restore(owner, &mut state);
                        selection::retry_restore(owner, &mut state);
                        let mut msg = MSG::default();
                        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                        match rx.recv_timeout(Duration::from_millis(5)) {
                            Ok(job) => {
                                let result = run(owner, &mut state, &job);
                                let _ = job.reply.try_send(result);
                                active.store(false, Ordering::Release);
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                    }
                }
            });
        Worker { tx, busy }
    })
}

pub fn submit(op: Operation, diag: Option<DiagTarget>) -> InsertOutcome {
    let deadline = Instant::now() + BUDGET;
    let method = match &op {
        Operation::Insert(r) => r.method,
        _ => InsertMethod::Paste,
    };
    let worker = worker();
    if worker
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return fail(method, "inserção anterior ainda em andamento");
    }
    let (reply, rx) = mpsc::sync_channel(1);
    if worker
        .tx
        .try_send(Job {
            op,
            diag,
            deadline,
            reply,
        })
        .is_err()
    {
        worker.busy.store(false, Ordering::Release);
        return fail(method, "thread de inserção indisponível");
    }
    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_else(|_| {
            fail(
                method,
                "prazo de inserção esgotado; confira o alvo antes de tentar novamente",
            )
        })
}

fn run(owner: HWND, state: &mut State, job: &Job) -> InsertOutcome {
    if state.selection_restore.is_some() {
        return fail(
            InsertMethod::Auto,
            "restauração da cópia de seleção pendente",
        );
    }
    if state.pending_restore.is_some() {
        return fail(
            InsertMethod::Auto,
            "restauração do clipboard anterior ainda pendente",
        );
    }
    if Instant::now() >= job.deadline {
        return fail(InsertMethod::Auto, "operação expirada");
    }
    match &job.op {
        Operation::UndoTransform(id) => {
            if state.transform_undo.as_ref() != Some(id) {
                return fail(
                    InsertMethod::Paste,
                    "transformação não é a última inserção verificada",
                );
            }
            state.transform_undo = None;
            let Some(last) = state.undo.as_ref() else {
                return fail(InsertMethod::Paste, "sem inserção verificada");
            };
            let undo_job = Job {
                op: Operation::Undo,
                diag: Some(DiagTarget {
                    hwnd: last.hwnd,
                    title_contains: last.context.title.clone(),
                }),
                deadline: job.deadline,
                reply: job.reply.clone(),
            };
            undo(state, &undo_job)
        }
        Operation::Selection(privacy, reply) => {
            let result = selection::capture(owner, state, job, privacy);
            let ok = result.is_ok();
            let _ = reply.try_send(result);
            if ok {
                InsertOutcome {
                    ok: true,
                    method: InsertMethod::Auto,
                    verified: None,
                    reason: None,
                }
            } else {
                fail(InsertMethod::Auto, "seleção indisponível")
            }
        }
        Operation::Replace(selection, req, privacy) => {
            if let Err(e) = selection::validate_current(owner, state, job, selection, privacy) {
                return fail(req.method, e);
            }
            let outcome = insert(owner, state, req, job);
            if outcome.ok && outcome.verified == Some(true) {
                state.transform_undo = Some(selection.id.clone());
            }
            outcome
        }
        Operation::Undo => undo(state, job),
        Operation::CopyLast => {
            let Some(req) = state.last.clone() else {
                return fail(InsertMethod::Paste, "nenhum texto anterior");
            };
            copy(owner, &req, job.deadline, "último texto copiado")
        }
        Operation::Copy(req) => {
            if req.text.is_empty() {
                return fail(req.method, "texto vazio");
            }
            let mut saved = req.clone();
            saved.press_enter = false;
            state.last = Some(saved);
            copy(owner, req, job.deadline, "texto copiado")
        }
        Operation::Insert(req) => {
            if !req.text.is_empty() {
                let mut saved = req.clone();
                saved.press_enter = false;
                state.last = Some(saved);
            }
            insert(owner, state, req, job)
        }
        Operation::PasteLast => {
            let Some(req) = state.last.clone() else {
                return fail(InsertMethod::Paste, "nenhum texto anterior");
            };
            insert(owner, state, &req, job)
        }
    }
}

/// Publica o texto no clipboard e o deixa lá (sem restaurar o anterior: é uma cópia
/// pedida). Se publicar falhar, devolve o clipboard anterior.
fn copy(owner: HWND, req: &InsertRequest, deadline: Instant, done: &str) -> InsertOutcome {
    let result = (|| {
        let data = clipboard::prepare(req)?;
        let open = clipboard::Open::acquire(owner, deadline)?;
        let old = clipboard::Snapshot::capture(&open, deadline)?;
        if Instant::now() >= deadline {
            return Err("cópia expirada".into());
        }
        if let Err(e) = clipboard::publish(data, &open) {
            let _ = old.restore(&open);
            return Err(e);
        }
        Ok(())
    })();
    match result {
        Ok(()) => InsertOutcome {
            ok: true,
            method: InsertMethod::Paste,
            verified: None,
            reason: Some(done.into()),
        },
        Err(e) => fail(InsertMethod::Paste, e),
    }
}

struct Target {
    hwnd: isize,
    pid: u32,
    diag: Option<DiagTarget>,
}
impl Target {
    fn snapshot(&self) -> FocusSnapshot {
        context::snapshot_target(
            self.hwnd,
            self.diag.as_ref().map(|d| d.title_contains.as_str()),
        )
    }
    fn acquire(diag: Option<&DiagTarget>) -> Result<Self, String> {
        if let Some(diag) = diag {
            if diag.hwnd == 0 || diag.title_contains.trim().is_empty() {
                return Err("diagnóstico exige HWND e título único".into());
            }
            // SAFETY: diagnostic target supplied by the test that created it.
            unsafe {
                SetForegroundWindow(diag.hwnd as HWND);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let hwnd = unsafe { GetForegroundWindow() } as isize;
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd as HWND, &mut pid);
        }
        let target = Self {
            hwnd,
            pid,
            diag: diag.cloned(),
        };
        target.check()?;
        Ok(target)
    }
    /// Must run immediately before EACH SendInput call, including Enter/undo.
    fn check(&self) -> Result<(), String> {
        let mut pid = 0;
        let foreground = unsafe { GetForegroundWindow() } as isize;
        unsafe {
            GetWindowThreadProcessId(self.hwnd as HWND, &mut pid);
        }
        let diag_ok = self.diag.as_ref().is_none_or(|d| {
            !d.title_contains.is_empty()
                && foreground == d.hwnd
                && context::win::window_title(foreground).contains(&d.title_contains)
        });
        if self.hwnd == 0
            || foreground != self.hwnd
            || pid != self.pid
            || unsafe { IsWindow(self.hwnd as HWND) } == 0
            || !diag_ok
        {
            tracing::warn!(
                target_hwnd = self.diag.as_ref().map_or(self.hwnd, |d| d.hwnd),
                captured_hwnd = self.hwnd,
                foreground_hwnd = foreground,
                "inserção ABORTADA: foco não corresponde ao alvo; nenhuma tecla enviada nesta etapa"
            );
            return Err("foco mudou ou alvo de diagnóstico inválido; injeção abortada".into());
        }
        Ok(())
    }
    fn keys(&self, keys: &[(u16, bool)], deadline: Instant) -> Result<(), String> {
        if Instant::now() >= deadline {
            return Err("operação expirada antes da injeção".into());
        }
        if modifiers_down() {
            return Err("modificador pressionado antes da injeção".into());
        }
        self.check()?;
        let sent = send_keys(keys) as usize;
        if sent != keys.len() {
            return Err(format!(
                "SendInput aceitou {sent}/{} eventos; UIPI ou desktop protegido pode bloquear; não repetido",
                keys.len()
            ));
        }
        Ok(())
    }
    fn unicode(&self, text: &str, deadline: Instant) -> Result<(), String> {
        if text.encode_utf16().count() > 4096 {
            return Err("digitação Unicode limitada a 4096 unidades UTF-16".into());
        }
        let inputs: Vec<INPUT> = text
            .encode_utf16()
            .flat_map(|unit| {
                [0, KEYEVENTF_KEYUP].map(|up| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: unit,
                            dwFlags: KEYEVENTF_UNICODE | up,
                            time: 0,
                            dwExtraInfo: VYRP_INJECT_MAGIC,
                        },
                    },
                })
            })
            .collect();
        if Instant::now() >= deadline || modifiers_down() {
            return Err("prazo esgotado ou modificador pressionado".into());
        }
        self.check()?;
        // SAFETY: contiguous INPUT array, correct size and own injection marker.
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        if sent as usize != inputs.len() {
            return Err("SendInput Unicode incompleto; não repetido para evitar duplicação".into());
        }
        Ok(())
    }
}

fn modifiers_down() -> bool {
    MODIFIERS.iter().any(|&vk| AsyncKeyState.is_down(vk))
}
fn wait_modifiers(deadline: Instant) -> bool {
    while modifiers_down() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

fn window_class(hwnd: isize) -> String {
    let mut class = [0u16; 128];
    // SAFETY: buffer válido; hwnd inválido só devolve 0.
    let len = unsafe { GetClassNameW(hwnd as HWND, class.as_mut_ptr(), class.len() as i32) };
    String::from_utf16_lossy(&class[..len.max(0) as usize])
}

fn insert(owner: HWND, state: &mut State, req: &InsertRequest, job: &Job) -> InsertOutcome {
    state.transform_undo = None;
    state.undo = None;
    crate::learn::forget();
    if req.text.is_empty() {
        return fail(req.method, "texto vazio");
    }
    if req.text.contains('\0')
        || req.text.len() > 4 * 1024 * 1024
        || req.html.as_ref().is_some_and(|s| s.len() > 4 * 1024 * 1024)
    {
        return fail(req.method, "texto inválido ou maior que 4 MiB");
    }
    let target = match Target::acquire(job.diag.as_ref()) {
        Ok(t) => t,
        Err(e) => return fail(req.method, e),
    };
    if elevated_target(target.pid) == Some(true) {
        return fail(
            req.method,
            "UIPI: alvo tem nível de integridade maior que o Vyrp",
        );
    }
    // Leave enough budget for capture, clipboard restoration and verification.
    if !wait_modifiers((Instant::now() + Duration::from_millis(600)).min(job.deadline)) {
        return fail(
            req.method,
            "modificadores ainda pressionados; nenhuma tecla enviada",
        );
    }
    let before = target.snapshot();
    if let Operation::Replace(selection, _, privacy) = &job.op {
        if let Err(e) = selection::check_snapshot(selection, &before, privacy) {
            return fail(req.method, e);
        }
    }
    if before.password {
        return fail(req.method, "campo de senha: inserção bloqueada");
    }
    if before.hwnd != 0 && before.hwnd != target.hwnd {
        return fail(req.method, "foco mudou durante captura");
    }
    let exe = context::win::window_exe(target.hwnd);
    let class = window_class(target.hwnd);
    let mut method = match req.method {
        InsertMethod::Auto if super::is_terminal(&class, &exe) => InsertMethod::ShiftInsert,
        InsertMethod::Auto => InsertMethod::Paste,
        m => m,
    };
    let mut note = None;
    let result = if method == InsertMethod::Type {
        target.unicode(&req.text, job.deadline)
    } else {
        match paste(owner, state, &target, req, method, job.deadline) {
            Ok(warning) => {
                note = warning;
                Ok(())
            }
            // Only fallback if clipboard was never modified and no key was sent.
            Err(PasteError::BeforeChange(e)) if req.method == InsertMethod::Auto => {
                method = InsertMethod::Type;
                note = Some(format!("clipboard indisponível; usado Unicode ({e})"));
                target.unicode(&req.text, job.deadline)
            }
            Err(PasteError::BeforeChange(e) | PasteError::AfterChange(e)) => Err(e),
        }
    };
    if let Err(e) = result {
        return fail(method, e);
    }
    let mut after = target.snapshot();
    let mut verified = super::verified(&before, &after, &req.text);
    if verified == Some(false) && Instant::now() + Duration::from_millis(200) < job.deadline {
        std::thread::sleep(Duration::from_millis(50));
        after = target.snapshot();
        verified = super::verified(&before, &after, &req.text);
    }
    if verified == Some(false) {
        return InsertOutcome {
            ok: false,
            method,
            verified,
            reason: Some("UIA não confirmou o texto; confira o alvo antes de repetir".into()),
        };
    }
    if req.press_enter {
        if let Err(e) = target.keys(&[(0x0d, false), (0x0d, true)], job.deadline) {
            return fail(method, format!("texto enviado; Enter abortado: {e}"));
        }
    } else if verified == Some(true) {
        // Âncoras para o auto-aprendizado (V24): o que havia antes e depois da colagem.
        // Terminal fica de fora: a saída do console mudaria o "campo" sozinha.
        if method != InsertMethod::ShiftInsert {
            crate::learn::remember_paste(
                after.hwnd,
                after.focus_id.clone(),
                &before.context.before,
                &after.context.after,
                &req.text,
            );
        }
        state.undo = Some(after);
    }
    InsertOutcome {
        ok: true,
        method,
        verified,
        reason: note.or_else(|| {
            verified
                .is_none()
                .then(|| "teclas enviadas; alvo não permite verificação UIA".into())
        }),
    }
}

enum PasteError {
    BeforeChange(String),
    AfterChange(String),
}

fn paste(
    owner: HWND,
    state: &mut State,
    target: &Target,
    req: &InsertRequest,
    method: InsertMethod,
    deadline: Instant,
) -> Result<Option<String>, PasteError> {
    let data = clipboard::prepare(req).map_err(PasteError::BeforeChange)?;
    let open = clipboard::Open::acquire(
        owner,
        (Instant::now() + Duration::from_millis(150)).min(deadline),
    )
    .map_err(PasteError::BeforeChange)?;
    let original =
        clipboard::Snapshot::capture(&open, deadline).map_err(PasteError::BeforeChange)?;
    target.check().map_err(PasteError::BeforeChange)?;
    if Instant::now() + Duration::from_millis(180) >= deadline {
        return Err(PasteError::BeforeChange(
            "prazo insuficiente para colar/restaurar".into(),
        ));
    }
    if let Err(e) = clipboard::publish(data, &open) {
        let restored = original.restore(&open);
        return Err(PasteError::AfterChange(format!(
            "{e}; restauração: {}",
            restored.is_ok()
        )));
    }
    drop(open); // Target must be able to open clipboard while handling paste.
    // Windows may finalize/synthesize formats on CloseClipboard, changing its
    // sequence. Take the post-close value, but also require our owner HWND so a
    // concurrent user's copy can never be mistaken for our publication.
    let sequence = clipboard::sequence();
    let chord = if method == InsertMethod::ShiftInsert {
        [(0x10, false), (0x2d, false), (0x2d, true), (0x10, true)]
    } else {
        [
            (VK_CONTROL, false),
            (0x56, false),
            (0x56, true),
            (VK_CONTROL, true),
        ]
    };
    let result = if clipboard::sequence() == sequence && clipboard::owned_by(owner) {
        target.keys(&chord, deadline)
    } else {
        Err("clipboard mudou antes de colar; nenhuma tecla enviada".into())
    };
    if result.is_ok() {
        std::thread::sleep(Duration::from_millis(120));
    }
    // Restoration also runs after injection failure. Never overwrite a new copy.
    let restoration = match clipboard::Open::acquire(owner, deadline) {
        Ok(open) if clipboard::sequence() == sequence && clipboard::owned_by(owner) => {
            original.restore(&open).map(|_| None)
        }
        Ok(_) => Ok(Some(
            "clipboard mudou; restauração cancelada para preservar a nova cópia".into(),
        )),
        Err(e) => {
            // Keep every original handle alive; retry on the owning STA instead
            // of losing the user's clipboard because another app held it open.
            state.pending_restore = Some((sequence, original));
            Err(format!(
                "{e}; restauração mantida em memória para nova tentativa"
            ))
        }
    };
    if let Err(e) = result {
        return Err(PasteError::AfterChange(e));
    }
    restoration.map_err(|e| {
        PasteError::AfterChange(format!(
            "texto enviado; clipboard não pôde ser restaurado: {e}"
        ))
    })
}

fn retry_restore(owner: HWND, state: &mut State) {
    let Some((sequence, _)) = state.pending_restore.as_ref() else {
        return;
    };
    if clipboard::sequence() != *sequence || !clipboard::owned_by(owner) {
        state.pending_restore = None;
        return;
    }
    let Ok(open) = clipboard::Open::acquire(owner, Instant::now() + Duration::from_millis(10))
    else {
        return;
    };
    if let Some((sequence, original)) = state.pending_restore.take() {
        if clipboard::sequence() == sequence && clipboard::owned_by(owner) {
            if let Err(error) = original.restore(&open) {
                tracing::error!(%error, "falha ao restaurar formatos do clipboard");
            }
        }
    }
}

fn undo(state: &mut State, job: &Job) -> InsertOutcome {
    let Some(last) = state.undo.take() else {
        return fail(
            InsertMethod::Paste,
            "nenhuma inserção verificada disponível para desfazer",
        );
    };
    let target = match Target::acquire(job.diag.as_ref()) {
        Ok(t) => t,
        Err(e) => return fail(InsertMethod::Paste, e),
    };
    let current = target.snapshot();
    if current.hwnd != last.hwnd
        || current.focus_id != last.focus_id
        || !current.text_range
        || current.context != last.context
    {
        return fail(
            InsertMethod::Paste,
            "campo ou conteúdo mudou; desfazer abortado",
        );
    }
    if !wait_modifiers(job.deadline) {
        return fail(InsertMethod::Paste, "modificadores ainda pressionados");
    }
    match target.keys(
        &[
            (VK_CONTROL, false),
            (0x5a, false),
            (0x5a, true),
            (VK_CONTROL, true),
        ],
        job.deadline,
    ) {
        Ok(()) => InsertOutcome {
            ok: true,
            method: InsertMethod::Paste,
            verified: None,
            reason: Some("Ctrl+Z enviado ao mesmo campo verificado".into()),
        },
        Err(e) => fail(InsertMethod::Paste, e),
    }
}

fn elevated_target(pid: u32) -> Option<bool> {
    // SAFETY: process/token handles are closed, token buffer is pointer-aligned.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let target = integrity(process);
        CloseHandle(process);
        Some(target? > integrity(GetCurrentProcess())?)
    }
}

fn integrity(process: HANDLE) -> Option<u32> {
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut required = 0;
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut required,
        );
        if required == 0 || required > 65536 {
            CloseHandle(token);
            return None;
        }
        let mut storage = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            storage.as_mut_ptr().cast(),
            required,
            &mut required,
        );
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let label = &*storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)))
    }
}
