use super::{FocusSnapshot, rules, truncate};
use std::collections::VecDeque;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
};
use windows::Win32::System::Variant::VariantToBoolean;
use windows::Win32::UI::Accessibility::*;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, MSG,
    PM_REMOVE, PeekMessageW, TranslateMessage,
};

const BUDGET: Duration = Duration::from_millis(150);
struct Request {
    hwnd: isize,
    title: Option<String>,
    deadline: Instant,
    reply: mpsc::SyncSender<FocusSnapshot>,
    full_selection: bool,
}
struct Worker {
    tx: mpsc::SyncSender<Request>,
    busy: Arc<AtomicBool>,
}

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Request>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let active = busy.clone();
        let _ = std::thread::Builder::new()
            .name("vyrp-context-sta".into())
            .spawn(move || {
                // SAFETY: COM and every UIA interface stay exclusively on this thread.
                unsafe {
                    if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
                        return;
                    }
                    let automation: windows::core::Result<IUIAutomation> =
                        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
                    if let Ok(automation) = automation {
                        loop {
                            let mut msg = MSG::default();
                            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0
                            {
                                TranslateMessage(&msg);
                                DispatchMessageW(&msg);
                            }
                            match rx.recv_timeout(Duration::from_millis(5)) {
                                Ok(req) => {
                                    let result = if Instant::now() < req.deadline {
                                        read(&automation, &req).unwrap_or_default()
                                    } else {
                                        FocusSnapshot::default()
                                    };
                                    let _ = req.reply.try_send(result);
                                    active.store(false, Ordering::Release);
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                            }
                        }
                    }
                    CoUninitialize();
                }
            });
        Worker { tx, busy }
    })
}

pub fn warm_up() {
    let _ = worker();
}

pub fn capture() -> FocusSnapshot {
    let hwnd = unsafe { GetForegroundWindow() } as isize;
    capture_target(hwnd, None)
}

pub fn capture_target(hwnd: isize, title: Option<String>) -> FocusSnapshot {
    let wanted = title.clone();
    let snapshot = capture_uia(hwnd, title);
    with_fallback(snapshot, hwnd, wanted.as_deref())
}

/// UIA não respondeu (provider travado, prazo de 150 ms, foco fora da janela): app,
/// título e categoria ainda saem do Win32 (barato e sem COM), para a lista de
/// privacidade por app e a categoria funcionarem. Texto e senha ficam desconhecidos.
fn with_fallback(snapshot: FocusSnapshot, hwnd: isize, title: Option<&str>) -> FocusSnapshot {
    if snapshot.hwnd != 0 || hwnd == 0 {
        return snapshot;
    }
    // SAFETY: sem pré-condições.
    if unsafe { GetForegroundWindow() } as isize != hwnd {
        return snapshot;
    }
    let window = window_title(hwnd);
    if title.is_some_and(|t| t.is_empty() || !window.contains(t)) {
        return snapshot;
    }
    let mut out = FocusSnapshot {
        hwnd,
        ..Default::default()
    };
    out.context.exe = window_exe(hwnd);
    out.context.title = window;
    out.context.category = rules::get().classify(&out.context.exe, None);
    out
}

fn capture_uia(hwnd: isize, title: Option<String>) -> FocusSnapshot {
    capture_request(hwnd, title, false)
}

pub(crate) fn selection_target(hwnd: isize, title: Option<String>) -> FocusSnapshot {
    // No Win32-only fallback: unknown password/focused element must fail closed.
    capture_request(hwnd, title, true)
}

fn capture_request(hwnd: isize, title: Option<String>, full_selection: bool) -> FocusSnapshot {
    let deadline = Instant::now() + BUDGET;
    let worker = worker();
    if worker
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return FocusSnapshot::default();
    }
    let (reply, rx) = mpsc::sync_channel(1);
    if worker
        .tx
        .try_send(Request {
            hwnd,
            title,
            deadline,
            reply,
            full_selection,
        })
        .is_err()
    {
        worker.busy.store(false, Ordering::Release);
        return FocusSnapshot::default();
    }
    receive_until(rx, deadline)
}

fn receive_until(rx: mpsc::Receiver<FocusSnapshot>, deadline: Instant) -> FocusSnapshot {
    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_default()
}

pub(crate) fn window_title(hwnd: isize) -> String {
    let mut buf = [0u16; 1024];
    // SAFETY: valid output buffer, stale window handles simply return zero.
    let len = unsafe { GetWindowTextW(hwnd as _, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

pub(crate) fn window_exe(hwnd: isize) -> String {
    // SAFETY: handles/buffers stay local and the owned process handle is closed.
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd as _, &mut pid);
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 32768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize])
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    }
}

fn read(uia: &IUIAutomation, req: &Request) -> Option<FocusSnapshot> {
    // SAFETY: all interfaces belong to this STA; every returned COM allocation is
    // owned by a windows wrapper or explicitly freed (runtime id SAFEARRAY).
    unsafe {
        if !matches_request(req) {
            return None;
        }
        let rules = rules::get();
        let focus = uia.GetFocusedElement().ok()?;
        let root = uia.ElementFromHandle(HWND(req.hwnd as _)).ok()?;
        // Browser/WebView descendants may live in another process. Verify ancestry
        // instead of requiring the renderer PID to equal the top-level window PID.
        if !belongs_to(uia, &focus, &root, req.deadline) {
            return None;
        }
        let password = focus.CurrentIsPassword().ok()?.as_bool();
        let mut result = FocusSnapshot {
            hwnd: req.hwnd,
            password,
            ..Default::default()
        };
        result.context.password = password;
        result.context.exe = window_exe(req.hwnd);
        result.context.title = window_title(req.hwnd);
        result.focus_id = runtime_id(&focus);
        // Never query a password's text/name/value/selection, even as a fallback.
        if !password && matches_request(req) {
            let control = focus.CurrentControlType().ok();
            let value: Option<IUIAutomationValuePattern> =
                focus.GetCurrentPatternAs(UIA_ValuePatternId).ok();
            result.context.editable = value
                .as_ref()
                .and_then(|v| v.CurrentIsReadOnly().ok())
                .is_some_and(|v| !v.as_bool());
            if let Ok(pattern) =
                focus.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            {
                if let Ok(selection) = pattern.GetSelection().and_then(|a| a.GetElement(0)) {
                    if req.full_selection {
                        let single = pattern
                            .GetSelection()
                            .and_then(|a| a.Length())
                            .is_ok_and(|len| len == 1);
                        if !single {
                            result.selection_overflow = true;
                        }
                        if single
                            && let Ok(text) =
                                selection.GetText(crate::llm::transform::MAX_SELECTION as i32 + 1)
                        {
                            let text = text.to_string();
                            // UIA/BSTR lengths may count UTF-16 code units. A truncated
                            // surrogate-rich selection must never look complete.
                            result.selection_overflow =
                                text.encode_utf16().count() > crate::llm::transform::MAX_SELECTION;
                            if !result.selection_overflow {
                                result.full_selection = Some(text);
                            }
                        }
                    }
                    let readonly = selection
                        .GetAttributeValue(UIA_IsReadOnlyAttributeId)
                        .ok()
                        .and_then(|v| VariantToBoolean(&v).ok())
                        .map(|v| v.as_bool());
                    if readonly == Some(false)
                        && (control == Some(UIA_EditControlTypeId)
                            || control == Some(UIA_DocumentControlTypeId))
                    {
                        result.context.editable = true;
                    }
                    if let Some((before, selected, after)) =
                        text_around(&selection, rules.max_chars)
                    {
                        result.context.before = before;
                        result.context.selection = selected;
                        result.context.after = after;
                        result.text_range = true;
                    }
                }
            }
            // Value/Legacy expose read-only state but no caret offset. Do not
            // pretend that their full value is text before the cursor.
            if !result.context.editable && control == Some(UIA_EditControlTypeId) {
                if let Ok(legacy) = focus
                    .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                        UIA_LegacyIAccessiblePatternId,
                    )
                {
                    if let Ok(state) = legacy.CurrentState() {
                        result.context.editable = state & (0x40 | 0x20000000 | 0x1) == 0;
                    }
                }
            }
            if !result.text_range && result.context.editable && Instant::now() < req.deadline {
                if let Some((before, selection, after)) =
                    native_value_range(&focus, rules.max_chars)
                {
                    result.context.before = before;
                    result.context.selection = selection;
                    result.context.after = after;
                    result.text_range = true;
                }
            }
            if req.full_selection && result.full_selection.is_none() && !result.selection_overflow {
                if let Some((_, selected, _)) =
                    native_value_range(&focus, crate::llm::transform::MAX_SELECTION + 1)
                {
                    result.selection_overflow =
                        selected.chars().count() > crate::llm::transform::MAX_SELECTION;
                    if !result.selection_overflow {
                        result.full_selection = Some(selected);
                    }
                }
            }
            if Instant::now() < req.deadline
                && matches_request(req)
                && matches!(
                    result.context.exe.as_str(),
                    "chrome.exe" | "msedge.exe" | "firefox.exe" | "brave.exe"
                )
            {
                result.context.url = browser_url(uia, &root, req.deadline);
            }
        }
        result.context.category =
            rules.classify(&result.context.exe, result.context.url.as_deref());
        if Instant::now() >= req.deadline || !matches_request(req) {
            return None;
        }
        if !uia
            .CompareElements(&focus, &uia.GetFocusedElement().ok()?)
            .ok()?
            .as_bool()
        {
            return None;
        }
        Some(result)
    }
}

fn matches_request(req: &Request) -> bool {
    req.hwnd != 0
        && unsafe { GetForegroundWindow() } as isize == req.hwnd
        && req
            .title
            .as_ref()
            .is_none_or(|title| !title.is_empty() && window_title(req.hwnd).contains(title))
}

fn belongs_to(
    uia: &IUIAutomation,
    focus: &IUIAutomationElement,
    root: &IUIAutomationElement,
    deadline: Instant,
) -> bool {
    unsafe {
        let Ok(walker) = uia.ControlViewWalker() else {
            return false;
        };
        let mut current = focus.clone();
        for _ in 0..24 {
            if Instant::now() >= deadline {
                return false;
            }
            if uia
                .CompareElements(&current, root)
                .is_ok_and(|v| v.as_bool())
            {
                return true;
            }
            match walker.GetParentElement(&current) {
                Ok(parent) => current = parent,
                Err(_) => return false,
            }
        }
        false
    }
}

fn runtime_id(element: &IUIAutomationElement) -> Vec<i32> {
    // SAFETY: GetRuntimeId returns an owned one-dimensional SAFEARRAY of i32.
    unsafe {
        let Ok(array) = element.GetRuntimeId() else {
            return Vec::new();
        };
        if array.is_null() {
            return Vec::new();
        }
        let mut result = Vec::new();
        if SafeArrayGetDim(array) == 1 {
            if let (Ok(lo), Ok(hi)) = (SafeArrayGetLBound(array, 1), SafeArrayGetUBound(array, 1)) {
                if hi >= lo && hi.saturating_sub(lo) < 64 {
                    for i in lo..=hi {
                        let mut value: i32 = 0;
                        if SafeArrayGetElement(array, &i, (&mut value as *mut i32).cast()).is_err()
                        {
                            result.clear();
                            break;
                        }
                        result.push(value);
                    }
                }
            }
        }
        let _ = SafeArrayDestroy(array);
        result
    }
}

/// ValuePattern/Legacy have no selection offsets. Only a native edit that answers
/// EM_GETSEL can supply them; otherwise leave context empty instead of guessing.
fn native_value_range(
    focus: &IUIAutomationElement,
    max: usize,
) -> Option<(String, String, String)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    };
    unsafe {
        let hwnd = focus.CurrentNativeWindowHandle().ok()?.0;
        if hwnd.is_null() {
            return None;
        }
        let mut class = [0u16; 128];
        let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        let class = String::from_utf16_lossy(&class[..len.max(0) as usize]).to_ascii_lowercase();
        if !is_native_edit_class(&class) {
            return None;
        }
        let mut start = u32::MAX;
        let mut end = u32::MAX;
        let mut response = 0;
        if SendMessageTimeoutW(
            hwnd,
            0x00b0,
            (&mut start as *mut u32) as usize,
            (&mut end as *mut u32) as isize,
            SMTO_ABORTIFHUNG,
            20,
            &mut response,
        ) == 0
        {
            return None;
        }
        if start == u32::MAX || end == u32::MAX || end < start {
            return None;
        }
        let value = if let Ok(pattern) =
            focus.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        {
            pattern.CurrentValue().ok()?
        } else {
            let legacy = focus
                .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                    UIA_LegacyIAccessiblePatternId,
                )
                .ok()?;
            if legacy.CurrentState().ok()? & 0x20000000 != 0 {
                return None;
            }
            legacy.CurrentValue().ok()?
        };
        let wide: &[u16] = &value;
        if wide.len() > 1024 * 1024 || end as usize > wide.len() {
            return None;
        }
        // Native offsets are UTF-16. Decoding after slicing avoids byte-index bugs.
        let start = start as usize;
        let end = end as usize;
        Some((
            truncate(&String::from_utf16_lossy(&wide[..start]), max, true),
            if max == crate::llm::transform::MAX_SELECTION + 1 {
                String::from_utf16_lossy(&wide[start..end])
                    .chars()
                    .take(max)
                    .collect()
            } else {
                truncate(&String::from_utf16_lossy(&wide[start..end]), max, false)
            },
            truncate(&String::from_utf16_lossy(&wide[end..]), max, false),
        ))
    }
}

/// Controles de edição nativos (aceitam EM_GETSEL): EDIT/RichEdit do Win32 e os mesmos
/// embrulhados pelo WinForms (`WindowsForms10.EDIT.app.0.…`, `….RichEdit20W.…`).
fn is_native_edit_class(class: &str) -> bool {
    let class = class.to_ascii_lowercase();
    class == "edit"
        || class.starts_with("richedit")
        || (class.starts_with("windowsforms")
            && (class.contains(".edit.") || class.contains(".richedit")))
}

fn text_around(range: &IUIAutomationTextRange, max: usize) -> Option<(String, String, String)> {
    // SAFETY: cloned ranges remain on the UIA STA. GetText is bounded, including
    // the selection, so a whole-document selection never requests unbounded text.
    unsafe {
        let before = range.Clone().ok()?;
        before
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                range,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?;
        before
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_Start,
                TextUnit_Character,
                -(max as i32 + 1),
            )
            .ok()?;
        let after = range.Clone().ok()?;
        after
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                range,
                TextPatternRangeEndpoint_End,
            )
            .ok()?;
        after
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_End,
                TextUnit_Character,
                max as i32 + 1,
            )
            .ok()?;
        Some((
            truncate(&before.GetText(max as i32 + 1).ok()?.to_string(), max, true),
            truncate(&range.GetText(max as i32 + 1).ok()?.to_string(), max, false),
            truncate(&after.GetText(max as i32 + 1).ok()?.to_string(), max, false),
        ))
    }
}

/// Teto da busca pela barra de endereços (a interface do navegador, sem o documento da
/// página, cabe nisso; o prazo de 150 ms continua valendo).
const URL_SCAN_NODES: usize = 400;
const URL_SCAN_DEPTH: usize = 12;

fn browser_url(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    deadline: Instant,
) -> Option<String> {
    // SAFETY: tree walker and elements remain local to the STA. Traversal is
    // bounded by depth, node count and deadline. No keyboard shortcut steals focus.
    unsafe {
        let walker = uia.ControlViewWalker().ok()?;
        let mut queue = VecDeque::from([(root.clone(), 0)]);
        let mut count = 0;
        while let Some((element, depth)) = queue.pop_front() {
            count += 1;
            if count > URL_SCAN_NODES || Instant::now() >= deadline {
                break;
            }
            if element.CurrentControlType().ok() == Some(UIA_EditControlTypeId)
                && element
                    .CurrentIsPassword()
                    .ok()
                    .is_some_and(|p| !p.as_bool())
            {
                let id = element
                    .CurrentAutomationId()
                    .unwrap_or_default()
                    .to_string()
                    .to_lowercase();
                let name = element
                    .CurrentName()
                    .unwrap_or_default()
                    .to_string()
                    .to_lowercase();
                // Chromium (Chrome/Edge/Brave): classe estável, independe do idioma.
                let class = element.CurrentClassName().unwrap_or_default().to_string();
                if class == "OmniboxViewViews"
                    || id == "urlbar-input"
                    || id == "addresseditbox"
                    || name.contains("address and search")
                    || name.contains("endereço e pesquisa")
                    || name.contains("barra de endereços")
                    || name.contains("search or enter address")
                    || name.contains("pesquisar ou digitar")
                {
                    if let Ok(value) = element
                        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                        .and_then(|p| p.CurrentValue())
                    {
                        if let Some(url) = normalize_url(&value.to_string()) {
                            return Some(url);
                        }
                    }
                }
            }
            if depth >= URL_SCAN_DEPTH {
                continue;
            }
            // Skip page documents: an address bar lives in browser chrome, and a
            // page must not impersonate it with a matching accessible label.
            if element.CurrentControlType().ok() == Some(UIA_DocumentControlTypeId) {
                continue;
            }
            if let Ok(mut child) = walker.GetFirstChildElement(&element) {
                loop {
                    if queue.len() + count >= URL_SCAN_NODES || Instant::now() >= deadline {
                        break;
                    }
                    queue.push_back((child.clone(), depth + 1));
                    match walker.GetNextSiblingElement(&child) {
                        Ok(next) => child = next,
                        Err(_) => break,
                    }
                }
            }
        }
        None
    }
}

fn normalize_url(value: &str) -> Option<String> {
    if value.len() > 4096 || value.contains(char::is_whitespace) {
        return None;
    }
    let full = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let mut url = reqwest::Url::parse(&full).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url
            .host_str()
            .is_none_or(|h| !h.contains('.') && h != "localhost")
    {
        return None;
    }
    // Credentials/query/fragment often contain secrets, unnecessary for category.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unresponsive_provider_has_bounded_caller_wait() {
        let (_tx, rx) = mpsc::sync_channel(1);
        let start = Instant::now();
        assert_eq!(receive_until(rx, start + Duration::from_millis(20)).hwnd, 0);
        assert!(start.elapsed() < Duration::from_millis(150));
    }
    #[test]
    fn native_edit_classes_include_winforms_wrappers() {
        assert!(is_native_edit_class("Edit"));
        assert!(is_native_edit_class("RichEdit20W"));
        assert!(is_native_edit_class(
            "WindowsForms10.EDIT.app.0.141b42a_r6_ad1"
        ));
        assert!(is_native_edit_class("WindowsForms10.RichEdit20W.app.0.1"));
        assert!(!is_native_edit_class("WindowsForms10.BUTTON.app.0.1"));
        assert!(!is_native_edit_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn url_removes_credentials_query_and_fragment() {
        assert_eq!(
            normalize_url("https://u:secret@example.com/mail?token=x#y").as_deref(),
            Some("https://example.com/mail")
        );
        assert!(normalize_url("words entered into search").is_none());
        assert!(normalize_url("javascript://example.com").is_none());
    }
}
