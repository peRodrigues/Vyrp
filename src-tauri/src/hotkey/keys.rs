//! Códigos virtual-key do Windows usados pelos atalhos, sem depender da API Win32
//! (a lógica de gestos é testável em qualquer plataforma).

pub const VK_LBUTTON: u16 = 0x01;
pub const VK_MBUTTON: u16 = 0x04;
pub const VK_XBUTTON1: u16 = 0x05;
pub const VK_XBUTTON2: u16 = 0x06;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_CAPITAL: u16 = 0x14;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_SPACE: u16 = 0x20;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
/// Código sem atribuição no Windows: tecla "inócua" da sequência de mascaramento.
pub const VK_MASK: u16 = 0xE8;

/// Pseudo-código para "qualquer Win" (o Windows não tem VK genérico para Win).
pub const VK_WIN_ANY: u16 = 0x0100;

/// Lado genérico de um modificador específico (`VK_LCONTROL` → `VK_CONTROL`).
pub fn generic(vk: u16) -> u16 {
    match vk {
        VK_LSHIFT | VK_RSHIFT => VK_SHIFT,
        VK_LCONTROL | VK_RCONTROL => VK_CONTROL,
        VK_LMENU | VK_RMENU => VK_MENU,
        VK_LWIN | VK_RWIN => VK_WIN_ANY,
        other => other,
    }
}

/// `wanted` (como está no atalho) casa com `actual` (como chegou do hook)?
/// Um atalho com lado definido (`VK_LCONTROL`) só casa com aquele lado; um genérico
/// (`VK_CONTROL`, [`VK_WIN_ANY`]) casa com os dois.
pub fn same_key(wanted: u16, actual: u16) -> bool {
    wanted == actual || wanted == generic(actual)
}

pub fn is_modifier(vk: u16) -> bool {
    matches!(generic(vk), VK_SHIFT | VK_CONTROL | VK_MENU | VK_WIN_ANY)
}

pub fn is_mouse_button(vk: u16) -> bool {
    matches!(vk, VK_MBUTTON | VK_XBUTTON1 | VK_XBUTTON2)
}

/// Win e Alt soltos sozinhos abrem o Iniciar / a barra de menu: o atalho precisa de máscara.
pub fn needs_mask(vk: u16) -> bool {
    matches!(generic(vk), VK_WIN_ANY | VK_MENU)
}

/// Nome legível para log e UI (só para teclas de atalho; nunca para texto digitado).
pub fn name(vk: u16) -> String {
    let fixed = match vk {
        VK_LBUTTON => "MouseEsq",
        VK_MBUTTON => "MouseMeio",
        VK_XBUTTON1 => "Mouse4",
        VK_XBUTTON2 => "Mouse5",
        VK_SHIFT => "Shift",
        VK_CONTROL => "Ctrl",
        VK_MENU => "Alt",
        VK_CAPITAL => "CapsLock",
        VK_ESCAPE => "Esc",
        VK_SPACE => "Espaço",
        VK_LWIN => "WinEsq",
        VK_RWIN => "WinDir",
        VK_WIN_ANY => "Win",
        VK_LSHIFT => "ShiftEsq",
        VK_RSHIFT => "ShiftDir",
        VK_LCONTROL => "CtrlEsq",
        VK_RCONTROL => "CtrlDir",
        VK_LMENU => "AltEsq",
        VK_RMENU => "AltDir",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("VK_{vk:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_matches_both_sides_specific_only_one() {
        assert!(same_key(VK_CONTROL, VK_LCONTROL));
        assert!(same_key(VK_CONTROL, VK_RCONTROL));
        assert!(same_key(VK_LCONTROL, VK_LCONTROL));
        assert!(!same_key(VK_LCONTROL, VK_RCONTROL));
        assert!(same_key(VK_WIN_ANY, VK_RWIN));
        assert!(!same_key(VK_SPACE, VK_LCONTROL));
    }

    #[test]
    fn names_and_classes() {
        assert_eq!(name(VK_LCONTROL), "CtrlEsq");
        assert_eq!(name(0x56), "V");
        assert_eq!(name(0x70), "F1");
        assert!(is_modifier(VK_RMENU));
        assert!(needs_mask(VK_LWIN) && needs_mask(VK_MENU));
        assert!(!needs_mask(VK_CONTROL));
        assert!(is_mouse_button(VK_XBUTTON2));
    }
}
