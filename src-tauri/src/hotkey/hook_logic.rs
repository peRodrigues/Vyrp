//! Parte pura do hook (V5): tradução das mensagens LL em [`KeyEvent`], decisão do
//! watchdog e detecção de "tempestade" de reinstalações. Testável sem Win32.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::keys::{VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2};
use crate::core::types::KeyEvent;

/// Marca em `dwExtraInfo` das entradas que o próprio Vyrp injeta (SendInput):
/// o hook as descarta antes de chegarem à máquina de gestos.
pub const VYRP_INJECT_MAGIC: usize = 0x5659_5250; // "VYRP"

pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_XBUTTONDOWN: u32 = 0x020B;
pub const WM_XBUTTONUP: u32 = 0x020C;
/// `KBDLLHOOKSTRUCT.flags`: evento sintético.
pub const LLKHF_INJECTED: u32 = 0x10;
/// `MSLLHOOKSTRUCT.flags`: evento sintético.
pub const LLMHF_INJECTED: u32 = 0x01;

/// Teclado: mensagem LL → evento, ou `None` (mensagem irrelevante ou injeção do Vyrp).
pub fn translate_key(msg: u32, vk: u32, flags: u32, extra: usize, t: Instant) -> Option<KeyEvent> {
    let down = match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return None,
    };
    if extra == VYRP_INJECT_MAGIC || vk > 0xFF {
        return None;
    }
    Some(KeyEvent {
        vk: vk as u16,
        down,
        injected: flags & LLKHF_INJECTED != 0,
        t,
    })
}

/// Mouse: só botões que podem compor atalho (meio, 4 e 5). `mouse_data` = `MSLLHOOKSTRUCT.mouseData`.
pub fn translate_mouse(
    msg: u32,
    mouse_data: u32,
    flags: u32,
    extra: usize,
    t: Instant,
) -> Option<KeyEvent> {
    let (vk, down) = match msg {
        WM_MBUTTONDOWN => (VK_MBUTTON, true),
        WM_MBUTTONUP => (VK_MBUTTON, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let vk = match mouse_data >> 16 {
                1 => VK_XBUTTON1,
                2 => VK_XBUTTON2,
                _ => return None,
            };
            (vk, msg == WM_XBUTTONDOWN)
        }
        _ => return None,
    };
    if extra == VYRP_INJECT_MAGIC {
        return None;
    }
    Some(KeyEvent {
        vk,
        down,
        injected: flags & LLMHF_INJECTED != 0,
        t,
    })
}

/// O Windows remove em silêncio um hook LL que demora a responder. Sinal disso: o
/// sistema registrou entrada (`GetLastInputInfo`) bem depois do último evento que o
/// hook viu. Os dois valores são `GetTickCount` (ms, dá a volta a cada ~49 dias).
pub fn hook_seems_dead(hook_last_tick: u32, system_last_tick: u32, grace_ms: u32) -> bool {
    let ahead = system_last_tick.wrapping_sub(hook_last_tick) as i32;
    ahead > grace_ms as i32
}

/// Conta reinstalações numa janela deslizante; acima do limite é "tempestade"
/// (algo trava o callback de forma repetida e reinstalar não resolve).
#[derive(Debug)]
pub struct StormDetector {
    window: Duration,
    threshold: usize,
    hits: VecDeque<Instant>,
}

impl StormDetector {
    pub fn new(window: Duration, threshold: usize) -> Self {
        Self {
            window,
            threshold,
            hits: VecDeque::new(),
        }
    }

    /// Registra uma reinstalação; devolve `true` se virou tempestade.
    pub fn record(&mut self, now: Instant) -> bool {
        self.hits.push_back(now);
        while let Some(first) = self.hits.front() {
            if now.duration_since(*first) > self.window {
                self.hits.pop_front();
            } else {
                break;
            }
        }
        self.hits.len() >= self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::keys::{VK_LCONTROL, VK_LMENU};

    #[test]
    fn translates_keyboard_messages() {
        let t = Instant::now();
        let ev = translate_key(WM_KEYDOWN, VK_LCONTROL as u32, 0, 0, t).unwrap();
        assert!(ev.down && !ev.injected && ev.vk == VK_LCONTROL);
        // Alt chega como SYSKEY.
        let up = translate_key(WM_SYSKEYUP, VK_LMENU as u32, 0, 0, t).unwrap();
        assert!(!up.down && up.vk == VK_LMENU);
        let inj = translate_key(WM_KEYDOWN, 0x41, LLKHF_INJECTED, 0, t).unwrap();
        assert!(inj.injected);
    }

    #[test]
    fn drops_own_injections_and_noise() {
        let t = Instant::now();
        assert!(translate_key(WM_KEYDOWN, 0x41, LLKHF_INJECTED, VYRP_INJECT_MAGIC, t).is_none());
        assert!(translate_key(0x0200, 0x41, 0, 0, t).is_none());
        assert!(translate_mouse(0x0200, 0, 0, 0, t).is_none()); // movimento
        assert!(translate_mouse(WM_XBUTTONDOWN, 2 << 16, 0, VYRP_INJECT_MAGIC, t).is_none());
    }

    #[test]
    fn translates_extra_mouse_buttons() {
        let t = Instant::now();
        let x1 = translate_mouse(WM_XBUTTONDOWN, 1 << 16, 0, 0, t).unwrap();
        assert_eq!((x1.vk, x1.down), (VK_XBUTTON1, true));
        let x2 = translate_mouse(WM_XBUTTONUP, 2 << 16, 0, 0, t).unwrap();
        assert_eq!((x2.vk, x2.down), (VK_XBUTTON2, false));
        let m = translate_mouse(WM_MBUTTONDOWN, 0, LLMHF_INJECTED, 0, t).unwrap();
        assert!(m.injected && m.vk == VK_MBUTTON);
    }

    #[test]
    fn watchdog_handles_tick_wraparound() {
        assert!(!hook_seems_dead(1_000, 1_500, 2_000));
        assert!(hook_seems_dead(1_000, 5_000, 2_000));
        // Hook viu algo depois do sistema (corrida): vivo.
        assert!(!hook_seems_dead(5_000, 4_000, 2_000));
        // Volta do contador de 32 bits.
        assert!(hook_seems_dead(u32::MAX - 100, 3_000, 2_000));
        assert!(!hook_seems_dead(u32::MAX - 100, 100, 2_000));
    }

    #[test]
    fn storm_after_threshold_inside_window() {
        let t0 = Instant::now();
        let mut s = StormDetector::new(Duration::from_secs(60), 3);
        assert!(!s.record(t0));
        assert!(!s.record(t0 + Duration::from_secs(10)));
        assert!(s.record(t0 + Duration::from_secs(20)));
        // Os antigos saem da janela.
        assert!(!s.record(t0 + Duration::from_secs(200)));
    }
}
