//! Escolha do microfone (pura): preferido do usuário → lista ranqueada → padrão do SO.

/// Microfone a usar entre os `available` (nomes como o SO mostra).
pub fn choose(
    available: &[String],
    system_default: Option<&str>,
    preferred: Option<&str>,
    ranking: &[String],
) -> Option<String> {
    let present = |name: &str| available.iter().any(|a| a == name);
    preferred
        .filter(|p| present(p))
        .map(str::to_string)
        .or_else(|| ranking.iter().find(|r| present(r)).cloned())
        .or_else(|| system_default.filter(|d| present(d)).map(str::to_string))
        .or_else(|| available.first().cloned())
}

/// Microfones que apareceram entre duas varreduras.
pub fn newly_connected(before: &[String], after: &[String]) -> Vec<String> {
    after
        .iter()
        .filter(|a| !before.contains(a))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn preference_order() {
        let avail = names(&["Headset", "Embutido", "USB"]);
        let rank = names(&["Sumiu", "USB", "Headset"]);
        assert_eq!(
            choose(&avail, Some("Embutido"), Some("Headset"), &rank).unwrap(),
            "Headset"
        );
        assert_eq!(
            choose(&avail, Some("Embutido"), Some("Sumiu"), &rank).unwrap(),
            "USB"
        );
        assert_eq!(
            choose(&avail, Some("Embutido"), None, &[]).unwrap(),
            "Embutido"
        );
        assert_eq!(choose(&avail, None, None, &[]).unwrap(), "Headset");
        assert_eq!(choose(&[], Some("x"), None, &[]), None);
    }

    #[test]
    fn detects_new_devices() {
        let before = names(&["A"]);
        let after = names(&["A", "B"]);
        assert_eq!(newly_connected(&before, &after), names(&["B"]));
        assert!(newly_connected(&after, &before).is_empty());
    }
}
