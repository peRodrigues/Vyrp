//! Máquina de gestos (V6): `KeyEvent` → [`GestureEvent`]. Pura e sem timers: hold
//! mínimo e janela de duplo toque são avaliados nos próprios eventos.
//!
//! Regras:
//! - **PTT**: o atalho fechado inicia (`PttStart`) e soltar qualquer tecla dele termina
//!   (`PttEnd`). Se ficou pressionado menos que `min_hold`, foi um **toque**: `Cancel`
//!   (o orquestrador descarta o clipe).
//! - **Duplo toque**: um toque seguido de novo PTT dentro de `double_tap` → `LockStart`
//!   (mãos livres). O próximo PTT, ou o atalho de trava, termina (`LockEnd`).
//! - **Upgrade**: durante o PTT, fechar o atalho de trava (superconjunto) → `LockStart`
//!   (a sessão atual vira mãos livres).
//! - `Esc` com sessão ativa → `Cancel`.
//! - **Debounce**: novo início dentro de `debounce` após o fim de uma sessão é ignorado.
//! - **Chord fantasma**: ao fechar um atalho, as outras teclas dele têm de estar
//!   fisicamente pressionadas ([`KeyState`]); senão o estado interno estava velho.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::keys::{self, VK_ESCAPE};
use crate::core::types::{ActionId, GestureEvent, KeyEvent};

/// Atalho: 1 a 3 teclas (VKs; modificador genérico casa com os dois lados).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord(pub Vec<u16>);

impl Chord {
    fn satisfied_by(&self, down: &[u16]) -> bool {
        self.0
            .iter()
            .all(|&want| down.iter().any(|&d| keys::same_key(want, d)))
    }

    /// Fechado exatamente: todas as teclas do atalho, e nenhuma a mais.
    fn exact(&self, down: &[u16]) -> bool {
        down.len() == self.0.len() && self.satisfied_by(down)
    }

    fn contains(&self, vk: u16) -> bool {
        self.0.iter().any(|&want| keys::same_key(want, vk))
    }

    fn needs_mask(&self) -> bool {
        self.0.iter().any(|&vk| keys::needs_mask(vk))
    }

    pub fn label(&self) -> String {
        self.0
            .iter()
            .map(|&vk| keys::name(vk))
            .collect::<Vec<_>>()
            .join("+")
    }
}

/// Atalhos e tempos dos gestos; é o que o Settings guarda (`settings.gestures`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GestureConfig {
    pub ptt: Chord,
    pub lock: Option<Chord>,
    pub cancel: u16,
    pub actions: Vec<(Chord, ActionId)>,
    /// Segurar menos que isso é toque (cancela; conta para o duplo toque).
    pub min_hold_ms: u32,
    /// Janela do duplo toque que trava o mãos livres.
    pub double_tap_ms: u32,
    /// Ignora um novo início logo depois do fim de uma sessão.
    pub debounce_ms: u32,
}

impl Default for GestureConfig {
    /// Padrões do Windows: PTT = Ctrl+Win, trava = Ctrl+Win+Espaço, Esc cancela.
    fn default() -> Self {
        Self {
            ptt: Chord(vec![keys::VK_CONTROL, keys::VK_WIN_ANY]),
            lock: Some(Chord(vec![
                keys::VK_CONTROL,
                keys::VK_WIN_ANY,
                keys::VK_SPACE,
            ])),
            cancel: VK_ESCAPE,
            actions: Vec::new(),
            min_hold_ms: 150,
            double_tap_ms: 350,
            debounce_ms: 500,
        }
    }
}

impl GestureConfig {
    fn chords(&self) -> impl Iterator<Item = &Chord> {
        std::iter::once(&self.ptt)
            .chain(self.lock.iter())
            .chain(self.actions.iter().map(|(c, _)| c))
    }

    /// A tecla aparece em algum atalho? (Só essas vão para o log: nada de keylogging.)
    pub fn is_relevant(&self, vk: u16) -> bool {
        vk == self.cancel || self.chords().any(|c| c.contains(vk))
    }

    /// Pode ir para o log? Modificadores, botões de mouse e a tecla de cancelar dos
    /// atalhos, sim; uma tecla comum de atalho (ex.: Espaço) só com um modificador já
    /// pressionado, para não registrar o ritmo da digitação.
    pub fn worth_logging(&self, vk: u16, pressed: &[u16]) -> bool {
        if !self.is_relevant(vk) {
            return false;
        }
        if keys::is_modifier(vk) || keys::is_mouse_button(vk) || vk == self.cancel {
            return true;
        }
        pressed.iter().any(|&k| k != vk && keys::is_modifier(k))
    }

    /// Limita os tempos a faixas sensatas (valores vindos da UI ou de arquivo antigo).
    pub fn clamp_timings(&mut self) {
        self.min_hold_ms = self.min_hold_ms.clamp(0, 2_000);
        self.double_tap_ms = self.double_tap_ms.clamp(100, 1_000);
        self.debounce_ms = self.debounce_ms.clamp(0, 2_000);
    }

    fn min_hold(&self) -> Duration {
        Duration::from_millis(u64::from(self.min_hold_ms))
    }

    fn double_tap(&self) -> Duration {
        Duration::from_millis(u64::from(self.double_tap_ms))
    }

    fn debounce(&self) -> Duration {
        Duration::from_millis(u64::from(self.debounce_ms))
    }

    /// Valida os atalhos; devolve a lista de problemas (vazia = ok).
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let chords: Vec<&Chord> = self.chords().collect();
        for chord in &chords {
            let n = chord.0.len();
            if !(1..=3).contains(&n) {
                problems.push(format!("{}: atalho precisa de 1 a 3 teclas", chord.label()));
            }
            let mut sorted = chord.0.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.len() != n {
                problems.push(format!("{}: tecla repetida", chord.label()));
            }
            if !chord
                .0
                .iter()
                .any(|&vk| keys::is_modifier(vk) || keys::is_mouse_button(vk))
            {
                problems.push(format!(
                    "{}: precisa de um modificador ou botão extra do mouse",
                    chord.label()
                ));
            }
            if chord.contains(self.cancel) {
                problems.push(format!(
                    "{}: não pode usar a tecla de cancelar",
                    chord.label()
                ));
            }
        }
        for (i, a) in chords.iter().enumerate() {
            for b in &chords[i + 1..] {
                if a.0.len() == b.0.len() && a.satisfied_by(&b.0) {
                    problems.push(format!("{} e {}: atalhos iguais", a.label(), b.label()));
                }
            }
        }
        problems
    }
}

/// Estado físico das teclas (no Windows, `GetAsyncKeyState`).
pub trait KeyState {
    fn is_down(&self, vk: u16) -> bool;
}

/// Saída da máquina: gesto, pedido de máscara (Win/Alt) ou aviso de chord fantasma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Gesture(GestureEvent),
    /// Injetar a tecla inócua agora, com o atalho ainda pressionado, para Win/Alt não
    /// abrirem o Iniciar/menu ao serem soltos.
    Mask,
    /// Atalho descartado: teclas que o estado dava como pressionadas não estavam.
    Phantom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Ptt { started: Instant },
    Locked,
}

#[derive(Debug)]
pub struct GestureFsm {
    cfg: GestureConfig,
    down: Vec<u16>,
    mode: Mode,
    last_tap: Option<Instant>,
    last_end: Option<Instant>,
}

impl GestureFsm {
    pub fn new(cfg: GestureConfig) -> Self {
        Self {
            cfg,
            down: Vec::new(),
            mode: Mode::Idle,
            last_tap: None,
            last_end: None,
        }
    }

    pub fn config(&self) -> &GestureConfig {
        &self.cfg
    }

    pub fn pressed(&self) -> &[u16] {
        &self.down
    }

    pub fn on_key(&mut self, ev: KeyEvent, ks: &dyn KeyState) -> Vec<Output> {
        if ev.injected {
            return Vec::new();
        }
        if ev.down {
            if self.down.contains(&ev.vk) {
                return Vec::new(); // auto-repeat
            }
            self.down.push(ev.vk);
            self.on_press(ev, ks)
        } else {
            self.down.retain(|&vk| vk != ev.vk);
            self.on_release(ev)
        }
    }

    /// Solta as teclas que o estado dá como pressionadas mas não estão (key-up perdido:
    /// tela de bloqueio, UAC, foco em app elevado). Chamar periodicamente.
    pub fn reconcile(&mut self, ks: &dyn KeyState, now: Instant) -> Vec<Output> {
        let stale: Vec<u16> = self
            .down
            .iter()
            .copied()
            .filter(|&vk| !ks.is_down(vk))
            .collect();
        let mut out = Vec::new();
        for vk in stale {
            out.extend(self.on_key(
                KeyEvent {
                    vk,
                    down: false,
                    injected: false,
                    t: now,
                },
                ks,
            ));
        }
        out
    }

    fn on_press(&mut self, ev: KeyEvent, ks: &dyn KeyState) -> Vec<Output> {
        let now = ev.t;
        if ev.vk == self.cfg.cancel {
            if self.mode != Mode::Idle {
                self.end(now);
                return vec![Output::Gesture(GestureEvent::Cancel)];
            }
            return Vec::new();
        }
        match self.mode {
            Mode::Idle => self.press_idle(ev, ks),
            Mode::Ptt { .. } => {
                if self.lock_exact() {
                    self.mode = Mode::Locked;
                    return vec![Output::Gesture(GestureEvent::LockStart)];
                }
                Vec::new()
            }
            Mode::Locked => {
                if self.cfg.ptt.exact(&self.down) || self.lock_exact() {
                    self.end(now);
                    return vec![Output::Gesture(GestureEvent::LockEnd)];
                }
                Vec::new()
            }
        }
    }

    fn press_idle(&mut self, ev: KeyEvent, ks: &dyn KeyState) -> Vec<Output> {
        let now = ev.t;
        let hit = if let Some(i) = self
            .cfg
            .actions
            .iter()
            .position(|(c, _)| c.exact(&self.down))
        {
            Some((
                self.cfg.actions[i].0.clone(),
                Some(self.cfg.actions[i].1),
                false,
            ))
        } else if self.lock_exact() {
            self.cfg.lock.clone().map(|c| (c, None, true))
        } else if self.cfg.ptt.exact(&self.down) {
            Some((self.cfg.ptt.clone(), None, false))
        } else {
            None
        };
        let Some((chord, action, is_lock)) = hit else {
            return Vec::new();
        };
        if self.is_phantom(ev.vk, ks) {
            // Estado velho: fica só a tecla deste evento.
            self.down.retain(|&vk| vk == ev.vk);
            return vec![Output::Phantom];
        }
        let mut out = Vec::new();
        if chord.needs_mask() {
            out.push(Output::Mask);
        }
        if let Some(action) = action {
            out.push(Output::Gesture(GestureEvent::Action(action)));
            return out;
        }
        if self.in_debounce(now) {
            return out; // ignora o início, mas mantém a máscara de Win/Alt
        }
        let double_tap = self
            .last_tap
            .is_some_and(|tap| now.duration_since(tap) <= self.cfg.double_tap());
        self.last_tap = None;
        if is_lock || double_tap {
            self.mode = Mode::Locked;
            out.push(Output::Gesture(GestureEvent::LockStart));
        } else {
            self.mode = Mode::Ptt { started: now };
            out.push(Output::Gesture(GestureEvent::PttStart));
        }
        out
    }

    fn on_release(&mut self, ev: KeyEvent) -> Vec<Output> {
        let Mode::Ptt { started } = self.mode else {
            return Vec::new();
        };
        if !self.cfg.ptt.contains(ev.vk) {
            return Vec::new();
        }
        let now = ev.t;
        self.mode = Mode::Idle;
        if now.duration_since(started) < self.cfg.min_hold() {
            self.last_tap = Some(now);
            return vec![Output::Gesture(GestureEvent::Cancel)];
        }
        self.last_end = Some(now);
        vec![Output::Gesture(GestureEvent::PttEnd)]
    }

    fn lock_exact(&self) -> bool {
        self.cfg.lock.as_ref().is_some_and(|c| c.exact(&self.down))
    }

    fn in_debounce(&self, now: Instant) -> bool {
        self.last_end
            .is_some_and(|end| now.duration_since(end) < self.cfg.debounce())
    }

    fn is_phantom(&self, current: u16, ks: &dyn KeyState) -> bool {
        self.down.iter().any(|&vk| vk != current && !ks.is_down(vk))
    }

    fn end(&mut self, now: Instant) {
        self.mode = Mode::Idle;
        self.last_end = Some(now);
        self.last_tap = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::keys::*;
    use std::cell::RefCell;

    /// Teclado de teste: o "estado físico" segue os eventos, salvo teclas marcadas como fantasma.
    struct Board {
        fsm: GestureFsm,
        t0: Instant,
        physical: RefCell<Vec<u16>>,
    }

    impl KeyState for RefCell<Vec<u16>> {
        fn is_down(&self, vk: u16) -> bool {
            self.borrow().contains(&vk)
        }
    }

    impl Board {
        fn new() -> Self {
            Self::with(GestureConfig::default())
        }
        fn with(cfg: GestureConfig) -> Self {
            Self {
                fsm: GestureFsm::new(cfg),
                t0: Instant::now(),
                physical: RefCell::new(Vec::new()),
            }
        }
        fn at(&self, ms: u64) -> Instant {
            self.t0 + Duration::from_millis(ms)
        }
        fn key(&mut self, ms: u64, vk: u16, down: bool) -> Vec<GestureEvent> {
            {
                let mut p = self.physical.borrow_mut();
                if down {
                    p.push(vk);
                } else {
                    p.retain(|&k| k != vk);
                }
            }
            let ev = KeyEvent {
                vk,
                down,
                injected: false,
                t: self.at(ms),
            };
            gestures(self.fsm.on_key(ev, &self.physical))
        }
        /// Pressiona Ctrl+Win (esquerdos) em `ms`, solta em `ms + hold`.
        fn ptt(&mut self, ms: u64, hold: u64) -> Vec<GestureEvent> {
            let mut out = self.key(ms, VK_LCONTROL, true);
            out.extend(self.key(ms + 5, VK_LWIN, true));
            out.extend(self.key(ms + hold, VK_LWIN, false));
            out.extend(self.key(ms + hold + 5, VK_LCONTROL, false));
            out
        }
    }

    fn gestures(out: Vec<Output>) -> Vec<GestureEvent> {
        out.into_iter()
            .filter_map(|o| match o {
                Output::Gesture(g) => Some(g),
                _ => None,
            })
            .collect()
    }

    use GestureEvent::*;

    #[test]
    fn hold_is_push_to_talk() {
        let mut b = Board::new();
        assert_eq!(b.ptt(0, 1_000), vec![PttStart, PttEnd]);
    }

    #[test]
    fn win_chord_requests_mask() {
        let mut b = Board::new();
        b.key(0, VK_LCONTROL, true);
        let ev = KeyEvent {
            vk: VK_LWIN,
            down: true,
            injected: false,
            t: b.at(5),
        };
        b.physical.borrow_mut().push(VK_LWIN);
        let out = b.fsm.on_key(ev, &b.physical);
        assert_eq!(out, vec![Output::Mask, Output::Gesture(PttStart)]);
    }

    #[test]
    fn short_tap_cancels_and_double_tap_locks() {
        let mut b = Board::new();
        assert_eq!(b.ptt(0, 80), vec![PttStart, Cancel]);
        // Segundo toque dentro de 350 ms: trava.
        assert_eq!(b.ptt(200, 80), vec![LockStart]);
        // Fala à vontade; o próximo PTT encerra.
        assert_eq!(b.ptt(5_000, 300), vec![LockEnd]);
    }

    #[test]
    fn slow_second_tap_is_plain_ptt() {
        let mut b = Board::new();
        assert_eq!(b.ptt(0, 80), vec![PttStart, Cancel]);
        assert_eq!(b.ptt(1_000, 600), vec![PttStart, PttEnd]);
    }

    #[test]
    fn lock_chord_and_upgrade() {
        let mut b = Board::new();
        b.key(0, VK_LCONTROL, true);
        b.key(5, VK_LWIN, true);
        assert_eq!(b.key(10, VK_SPACE, true), vec![LockStart]);
        b.key(20, VK_SPACE, false);
        b.key(25, VK_LWIN, false);
        b.key(30, VK_LCONTROL, false);
        // Upgrade: começou como PTT, Espaço transforma em mãos livres.
        let mut c = Board::new();
        c.key(0, VK_LCONTROL, true);
        assert_eq!(c.key(5, VK_LWIN, true), vec![PttStart]);
        assert_eq!(c.key(800, VK_SPACE, true), vec![LockStart]);
        assert!(
            c.key(900, VK_LWIN, false).is_empty(),
            "soltar não encerra a trava"
        );
    }

    #[test]
    fn escape_cancels_active_session_only() {
        let mut b = Board::new();
        assert!(b.key(0, VK_ESCAPE, true).is_empty());
        b.key(10, VK_ESCAPE, false);
        b.key(100, VK_LCONTROL, true);
        assert_eq!(b.key(105, VK_LWIN, true), vec![PttStart]);
        assert_eq!(b.key(600, VK_ESCAPE, true), vec![Cancel]);
        assert!(b.key(700, VK_LWIN, false).is_empty());
    }

    #[test]
    fn debounce_after_session_end() {
        let mut b = Board::new();
        assert_eq!(b.ptt(0, 1_000), vec![PttStart, PttEnd]);
        assert!(b.ptt(1_200, 600).is_empty(), "dentro de 500 ms do fim");
        assert_eq!(b.ptt(2_000, 600), vec![PttStart, PttEnd]);
    }

    #[test]
    fn extra_key_blocks_exact_match() {
        let mut b = Board::new();
        b.key(0, VK_LSHIFT, true);
        b.key(5, VK_LCONTROL, true);
        assert!(b.key(10, VK_LWIN, true).is_empty());
    }

    #[test]
    fn side_specific_chord() {
        let cfg = GestureConfig {
            ptt: Chord(vec![VK_RCONTROL, VK_RMENU]),
            lock: None,
            ..GestureConfig::default()
        };
        let mut b = Board::with(cfg);
        b.key(0, VK_LCONTROL, true);
        assert!(
            b.key(5, VK_RMENU, true).is_empty(),
            "Ctrl esquerdo não serve"
        );
        b.key(10, VK_LCONTROL, false);
        b.key(15, VK_RMENU, false);
        b.key(1_000, VK_RCONTROL, true);
        assert_eq!(b.key(1_005, VK_RMENU, true), vec![PttStart]);
    }

    #[test]
    fn phantom_chord_is_dropped() {
        let mut b = Board::new();
        b.key(0, VK_LCONTROL, true);
        // Key-up do Ctrl perdido: o estado acha que ainda está pressionado.
        b.physical.borrow_mut().retain(|&k| k != VK_LCONTROL);
        b.physical.borrow_mut().push(VK_LWIN);
        let ev = KeyEvent {
            vk: VK_LWIN,
            down: true,
            injected: false,
            t: b.at(5_000),
        };
        assert_eq!(b.fsm.on_key(ev, &b.physical), vec![Output::Phantom]);
        assert_eq!(b.fsm.pressed(), &[VK_LWIN]);
    }

    #[test]
    fn reconcile_releases_stale_keys() {
        let mut b = Board::new();
        b.key(0, VK_LCONTROL, true);
        assert_eq!(b.key(5, VK_LWIN, true), vec![PttStart]);
        // Os key-ups se perderam (tela de bloqueio).
        b.physical.borrow_mut().clear();
        let out = gestures(b.fsm.reconcile(&b.physical, b.at(2_000)));
        assert_eq!(out, vec![PttEnd]);
        assert!(b.fsm.pressed().is_empty());
    }

    #[test]
    fn actions_and_injected_events() {
        let mut cfg = GestureConfig::default();
        cfg.actions
            .push((Chord(vec![VK_LSHIFT, VK_MENU, 0x5A]), ActionId::PasteLast));
        let mut b = Board::with(cfg);
        b.key(0, VK_LSHIFT, true);
        b.key(5, VK_LMENU, true);
        assert_eq!(b.key(10, 0x5A, true), vec![Action(ActionId::PasteLast)]);
        // Eventos injetados são ignorados por inteiro.
        let inj = KeyEvent {
            vk: VK_ESCAPE,
            down: true,
            injected: true,
            t: b.at(20),
        };
        assert!(b.fsm.on_key(inj, &b.physical).is_empty());
    }

    #[test]
    fn autorepeat_does_not_retrigger() {
        let mut b = Board::new();
        b.key(0, VK_LCONTROL, true);
        assert_eq!(b.key(5, VK_LWIN, true), vec![PttStart]);
        assert!(b.key(40, VK_LWIN, true).is_empty());
    }

    #[test]
    fn validation() {
        assert!(GestureConfig::default().validate().is_empty());
        let mut cfg = GestureConfig {
            lock: Some(Chord(vec![VK_WIN_ANY, VK_CONTROL])), // igual ao PTT
            ..GestureConfig::default()
        };
        cfg.actions.push((Chord(vec![0x41]), ActionId::CopyLast)); // sem modificador
        cfg.actions
            .push((Chord(vec![VK_CONTROL, VK_ESCAPE]), ActionId::OpenHub));
        cfg.actions.push((
            Chord(vec![VK_CONTROL, VK_SHIFT, VK_MENU, 0x41]),
            ActionId::Transform,
        ));
        let problems = cfg.validate();
        assert!(
            problems.iter().any(|p| p.contains("iguais")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("modificador")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("cancelar")),
            "{problems:?}"
        );
        assert!(problems.iter().any(|p| p.contains("1 a 3")), "{problems:?}");
    }

    #[test]
    fn config_round_trips_as_readable_json() {
        let cfg = GestureConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["ptt"], serde_json::json!([VK_CONTROL, VK_WIN_ANY]));
        assert_eq!(json["double_tap_ms"], 350);
        assert_eq!(serde_json::from_value::<GestureConfig>(json).unwrap(), cfg);
        // Campos ausentes ficam com o padrão.
        let partial: GestureConfig =
            serde_json::from_value(serde_json::json!({"ptt": [VK_RCONTROL, VK_RMENU]})).unwrap();
        assert_eq!(partial.min_hold_ms, 150);
        let mut wild = GestureConfig {
            double_tap_ms: 5,
            ..GestureConfig::default()
        };
        wild.clamp_timings();
        assert_eq!(wild.double_tap_ms, 100);
    }

    #[test]
    fn relevance_filter_for_logging() {
        let cfg = GestureConfig::default();
        assert!(
            cfg.is_relevant(VK_LCONTROL) && cfg.is_relevant(VK_RWIN) && cfg.is_relevant(VK_ESCAPE)
        );
        assert!(!cfg.is_relevant(0x41), "letras comuns nunca vão para o log");
        // Espaço é do atalho de trava, mas digitando texto não vai para o log.
        assert!(!cfg.worth_logging(VK_SPACE, &[]));
        assert!(cfg.worth_logging(VK_SPACE, &[VK_LCONTROL, VK_LWIN]));
        assert!(cfg.worth_logging(VK_LCONTROL, &[]));
        assert!(!cfg.worth_logging(0x41, &[VK_LCONTROL]));
    }
}
