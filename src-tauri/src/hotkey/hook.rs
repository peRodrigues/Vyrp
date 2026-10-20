//! Hook global de teclado/mouse (V5), Windows. Uma thread dedicada instala
//! `WH_KEYBOARD_LL` + `WH_MOUSE_LL` e roda o loop de mensagens; o callback só traduz e
//! enfileira (o Windows derruba hooks lentos). Um watchdog reinstala o hook quando o
//! sistema vê entrada que o hook não viu.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetLastInputInfo, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, LASTINPUTINFO, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_APP, WM_QUIT,
};

use super::gesture::KeyState;
use super::hook_logic::{
    StormDetector, VYRP_INJECT_MAGIC, hook_seems_dead, translate_key, translate_mouse,
};
use super::keys::{VK_CAPITAL, VK_MASK};
use crate::core::types::KeyEvent;

const WM_VYRP_REINSTALL: u32 = WM_APP + 1;
/// Folga antes de considerar o hook morto (ms de entrada do sistema sem evento no hook).
const WATCHDOG_GRACE_MS: u32 = 2_000;
const WATCHDOG_PERIOD: Duration = Duration::from_secs(1);

struct Shared {
    tx: Sender<KeyEvent>,
    /// `GetTickCount` do último evento (qualquer um, inclusive movimento de mouse).
    last_tick: AtomicU32,
    suppressed: [AtomicBool; 256],
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// Hooks instalados (só a thread do hook mexe; Mutex para o tipo ser `Sync`).
struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
}
// SAFETY: HHOOK é um handle opaco; só a thread do hook o usa.
unsafe impl Send for Hooks {}

/// Controle do hook em execução.
#[derive(Debug, Clone, Copy)]
pub struct HookHandle {
    thread_id: u32,
}

impl HookHandle {
    /// Engole a tecla no sistema enquanto marcada (para atalhos que não devem vazar).
    pub fn set_suppressed(&self, vk: u16, suppress: bool) {
        if let Some(shared) = SHARED.get() {
            shared.suppressed[usize::from(vk & 0xFF)].store(suppress, Ordering::Relaxed);
        }
    }

    pub fn stop(&self) {
        // SAFETY: mensagem para uma thread nossa; falha só se ela já terminou.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
    }
}

/// Instala o hook numa thread própria e publica cada [`KeyEvent`] em `tx`.
/// Só pode ser chamado uma vez por processo.
pub fn start(tx: Sender<KeyEvent>) -> Result<HookHandle, String> {
    SHARED
        .set(Shared {
            tx,
            last_tick: AtomicU32::new(now_tick()),
            suppressed: std::array::from_fn(|_| AtomicBool::new(false)),
        })
        .map_err(|_| "hook já iniciado".to_string())?;

    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    std::thread::Builder::new()
        .name("vyrp-hook".into())
        .spawn(move || hook_thread(ready_tx))
        .map_err(|e| e.to_string())?;
    let thread_id = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "a thread do hook não respondeu".to_string())??;

    std::thread::Builder::new()
        .name("vyrp-hook-watchdog".into())
        .spawn(move || watchdog(thread_id))
        .map_err(|e| e.to_string())?;
    Ok(HookHandle { thread_id })
}

fn hook_thread(ready: Sender<Result<u32, String>>) {
    let hooks = Mutex::new(None::<Hooks>);
    match install() {
        Ok(h) => *hooks.lock().unwrap() = Some(h),
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    }
    // SAFETY: sem pré-condições.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Ok(thread_id));
    tracing::info!("hook de teclado/mouse instalado");

    // SAFETY: MSG é POD; zerado é válido.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: `msg` vive o loop todo; hwnd nulo = mensagens da thread.
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        if msg.message == WM_VYRP_REINSTALL {
            let mut guard = hooks.lock().unwrap();
            if let Some(old) = guard.take() {
                uninstall(old);
            }
            match install() {
                Ok(h) => {
                    *guard = Some(h);
                    tracing::warn!("hook reinstalado pelo watchdog");
                }
                Err(e) => tracing::error!(error = %e, "falha ao reinstalar o hook"),
            }
        }
    }
    if let Some(old) = hooks.lock().unwrap().take() {
        uninstall(old);
    }
    tracing::info!("hook encerrado");
}

fn install() -> Result<Hooks, String> {
    // SAFETY: módulo do próprio exe; callbacks são `extern "system"` com a assinatura certa.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        if keyboard.is_null() {
            return Err(format!(
                "SetWindowsHookExW(teclado): {}",
                std::io::Error::last_os_error()
            ));
        }
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        if mouse.is_null() {
            UnhookWindowsHookEx(keyboard);
            return Err(format!(
                "SetWindowsHookExW(mouse): {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Hooks { keyboard, mouse })
    }
}

fn uninstall(hooks: Hooks) {
    // SAFETY: handles devolvidos por SetWindowsHookExW nesta thread.
    unsafe {
        UnhookWindowsHookEx(hooks.keyboard);
        UnhookWindowsHookEx(hooks.mouse);
    }
}

/// Callback do teclado: traduz, enfileira e (se pedido) engole. Nada de trabalho pesado.
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && let Some(shared) = SHARED.get()
    {
        // SAFETY: com HC_ACTION, lparam aponta para um KBDLLHOOKSTRUCT válido.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        shared.last_tick.store(now_tick(), Ordering::Relaxed);
        if let Some(ev) = translate_key(
            wparam as u32,
            info.vkCode,
            info.flags,
            info.dwExtraInfo,
            Instant::now(),
        ) {
            let _ = shared.tx.send(ev);
            if !ev.injected && shared.suppressed[usize::from(ev.vk & 0xFF)].load(Ordering::Relaxed)
            {
                return 1;
            }
        }
    }
    // SAFETY: repassa o evento para o próximo hook da cadeia.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Callback do mouse: marca atividade (watchdog) e enfileira só os botões de atalho.
unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && let Some(shared) = SHARED.get()
    {
        // SAFETY: com HC_ACTION, lparam aponta para um MSLLHOOKSTRUCT válido.
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        shared.last_tick.store(now_tick(), Ordering::Relaxed);
        if let Some(ev) = translate_mouse(
            wparam as u32,
            info.mouseData,
            info.flags,
            info.dwExtraInfo,
            Instant::now(),
        ) {
            let _ = shared.tx.send(ev);
        }
    }
    // SAFETY: repassa o evento para o próximo hook da cadeia.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

fn watchdog(hook_thread: u32) {
    let mut storm = StormDetector::new(Duration::from_secs(60), 5);
    loop {
        std::thread::sleep(WATCHDOG_PERIOD);
        let Some(shared) = SHARED.get() else { return };
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` inicializado com cbSize correto.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            continue;
        }
        let hook_last = shared.last_tick.load(Ordering::Relaxed);
        if hook_seems_dead(hook_last, info.dwTime, WATCHDOG_GRACE_MS) {
            // Evita reinstalar de novo no próximo ciclo pela mesma entrada.
            shared.last_tick.store(now_tick(), Ordering::Relaxed);
            if storm.record(Instant::now()) {
                tracing::error!(
                    "tempestade de quedas do hook (5 em 60 s); reinstalando mesmo assim"
                );
            }
            // SAFETY: mensagem para a thread do hook; falha só se ela terminou.
            if unsafe { PostThreadMessageW(hook_thread, WM_VYRP_REINSTALL, 0, 0) } == 0 {
                return;
            }
        }
    }
}

fn now_tick() -> u32 {
    // SAFETY: sem pré-condições.
    unsafe { GetTickCount() }
}

/// Estado físico real das teclas, para a checagem de chord fantasma e a reconciliação.
pub struct AsyncKeyState;

impl KeyState for AsyncKeyState {
    fn is_down(&self, vk: u16) -> bool {
        // SAFETY: sem pré-condições; bit alto = pressionada agora.
        unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
    }
}

/// Injeta teclas marcadas com [`VYRP_INJECT_MAGIC`] (o próprio hook as descarta).
fn send_keys(seq: &[(u16, bool)]) -> u32 {
    let inputs: Vec<INPUT> = seq
        .iter()
        .map(|&(vk, up)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: VYRP_INJECT_MAGIC,
                },
            },
        })
        .collect();
    // SAFETY: slice válido de INPUTs com o tamanho de struct correto.
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    }
}

/// Sequência de mascaramento: uma tecla inócua enquanto Win/Alt estão pressionados faz o
/// Windows não abrir o Iniciar/menu quando eles forem soltos.
pub fn send_mask_key() {
    if send_keys(&[(VK_MASK, false), (VK_MASK, true)]) != 2 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "máscara de Win/Alt não enviada");
    }
}

pub fn caps_lock_on() -> bool {
    // SAFETY: sem pré-condições; bit baixo = toggle ligado.
    unsafe { GetKeyState(i32::from(VK_CAPITAL)) & 1 != 0 }
}

/// Desfaz a troca de Caps Lock causada por um atalho de tecla única com Caps Lock.
pub fn restore_caps_lock(was_on: bool) {
    if caps_lock_on() != was_on {
        send_keys(&[(VK_CAPITAL, false), (VK_CAPITAL, true)]);
    }
}
