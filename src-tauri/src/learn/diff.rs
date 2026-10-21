//! Parte pura do auto-aprendizado (V24): achar no campo o trecho colado, comparar palavra
//! a palavra com o que o usuário deixou e filtrar o que vale virar vocabulário.

/// Correção aprendida: o termo certo e o que o ASR tinha escrito no lugar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learned {
    pub phrase: String,
    pub observed: String,
}

/// Até quantos candidatos por ditado e quantas palavras por lado (spec V24).
pub const MAX_CANDIDATES: usize = 4;
pub const MAX_WORDS: usize = 4;
/// Mais que isso de palavras trocadas = o usuário reescreveu, não corrigiu.
const MAX_CHANGED_RATIO: f64 = 0.5;
/// Teto de palavras analisadas (a comparação é quadrática).
const MAX_TOKENS: usize = 300;
const MAX_TERM_CHARS: usize = 40;

const STOPWORDS_PT: &[&str] = &[
    "a", "o", "as", "os", "um", "uma", "uns", "umas", "de", "do", "da", "dos", "das", "em", "no",
    "na", "nos", "nas", "por", "para", "pra", "com", "sem", "e", "ou", "mas", "que", "se", "é",
    "ser", "foi", "era", "eu", "tu", "ele", "ela", "nós", "eles", "elas", "você", "isso", "isto",
    "aquilo", "esse", "essa", "este", "esta", "meu", "minha", "seu", "sua", "não", "sim", "já",
    "mais", "muito", "bem", "aqui", "ali", "lá", "então", "também", "ao", "à", "aos", "às", "pelo",
    "pela", "como", "quando", "onde",
];

const STOPWORDS_EN: &[&str] = &[
    "the", "an", "and", "or", "but", "of", "to", "in", "on", "at", "for", "with", "by", "is",
    "are", "was", "were", "be", "it", "this", "that", "i", "you", "he", "she", "we", "they", "my",
    "your", "not", "yes", "no", "so", "too", "very", "as", "if",
];

/// Palavra sem pontuação nas pontas.
fn trim_word(w: &str) -> &str {
    w.trim_matches(|c: char| !c.is_alphanumeric())
}

fn norm(w: &str) -> String {
    trim_word(w).to_lowercase()
}

fn is_stopword(w: &str) -> bool {
    let w = norm(w);
    STOPWORDS_PT.contains(&w.as_str()) || STOPWORDS_EN.contains(&w.as_str())
}

/// O trecho do campo entre o texto que havia antes e depois do ponto da colagem.
/// `None` se as âncoras não aparecem (o usuário mexeu nelas ou o cursor foi longe).
pub fn locate<'a>(field: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = if prefix.is_empty() {
        0
    } else {
        field.rfind(prefix)? + prefix.len()
    };
    let rest = &field[start..];
    let end = if suffix.is_empty() {
        rest.len()
    } else {
        rest.find(suffix)?
    };
    Some(&rest[..end])
}

/// Pares (de, para) das trocas entre o texto colado e o editado; `None` se foi reescrita.
pub fn substitutions(pasted: &str, edited: &str) -> Option<Vec<(String, String)>> {
    let a: Vec<&str> = pasted.split_whitespace().take(MAX_TOKENS).collect();
    let b: Vec<&str> = edited.split_whitespace().take(MAX_TOKENS).collect();
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let (na, nb): (Vec<String>, Vec<String>) = (
        a.iter().map(|w| norm(w)).collect(),
        b.iter().map(|w| norm(w)).collect(),
    );
    // LCS por palavra normalizada.
    let mut dp = vec![vec![0u16; nb.len() + 1]; na.len() + 1];
    for i in (0..na.len()).rev() {
        for j in (0..nb.len()).rev() {
            dp[i][j] = if na[i] == nb[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut pairs = Vec::new();
    let mut changed = 0usize;
    let (mut from, mut to): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    let flush = |from: &mut Vec<&str>, to: &mut Vec<&str>, pairs: &mut Vec<(String, String)>| {
        // Só troca (dos dois lados); inserção ou remoção pura não tem erro de ASR a aprender.
        if !from.is_empty() && !to.is_empty() {
            pairs.push((join(from), join(to)));
        }
        from.clear();
        to.clear();
    };
    while i < na.len() || j < nb.len() {
        if i < na.len() && j < nb.len() && na[i] == nb[j] {
            flush(&mut from, &mut to, &mut pairs);
            // Mesma palavra, só a caixa mudou para maiúscula: nome próprio ("vyrp" → "Vyrp").
            let (x, y) = (trim_word(a[i]), trim_word(b[j]));
            if x != y && y.chars().any(char::is_uppercase) {
                pairs.push((x.to_string(), y.to_string()));
                changed += 1;
            }
            i += 1;
            j += 1;
        } else if j < nb.len() && (i == na.len() || dp[i][j + 1] >= dp[i + 1][j]) {
            to.push(b[j]);
            j += 1;
        } else {
            from.push(a[i]);
            changed += 1;
            i += 1;
        }
    }
    flush(&mut from, &mut to, &mut pairs);
    if changed as f64 > a.len() as f64 * MAX_CHANGED_RATIO {
        return None;
    }
    Some(pairs)
}

fn join(words: &[&str]) -> String {
    let joined = words.join(" ");
    trim_word(&joined).to_string()
}

/// Correções que valem virar vocabulário. `known(termo)` = já está no dicionário.
pub fn candidates(pasted: &str, edited: &str, known: &dyn Fn(&str) -> bool) -> Vec<Learned> {
    let Some(pairs) = substitutions(pasted, edited) else {
        return Vec::new();
    };
    let mut out: Vec<Learned> = Vec::new();
    for (from, to) in pairs {
        let words = to.split_whitespace().count();
        let from_words = from.split_whitespace().count();
        let good = (1..=MAX_WORDS).contains(&words)
            && (1..=MAX_WORDS).contains(&from_words)
            && (2..=MAX_TERM_CHARS).contains(&to.chars().count())
            && to.chars().any(char::is_alphabetic)
            && !to.split_whitespace().all(is_stopword)
            && to != from
            && !known(&to)
            && !out.iter().any(|l| l.phrase.eq_ignore_ascii_case(&to));
        if good {
            out.push(Learned {
                phrase: to,
                observed: from,
            });
        }
        if out.len() == MAX_CANDIDATES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn locate_finds_the_pasted_stretch_between_anchors() {
        let field = "Oi pessoal, o Vyrp funciona bem. Até mais";
        assert_eq!(
            locate(field, "Oi pessoal, ", " Até mais"),
            Some("o Vyrp funciona bem.")
        );
        assert_eq!(
            locate(field, "", " Até mais"),
            Some("Oi pessoal, o Vyrp funciona bem.")
        );
        assert_eq!(
            locate(field, "Oi pessoal, ", ""),
            Some("o Vyrp funciona bem. Até mais")
        );
        assert_eq!(locate(field, "não está", ""), None);
    }

    #[test]
    fn learns_a_corrected_term_with_the_asr_error() {
        let got = candidates("o vai arpe funciona bem", "o Vyrp funciona bem", &none);
        assert_eq!(
            got,
            vec![Learned {
                phrase: "Vyrp".into(),
                observed: "vai arpe".into()
            }]
        );
    }

    #[test]
    fn learns_proper_noun_capitalization() {
        let got = candidates(
            "falei com a maria ontem.",
            "falei com a Maria ontem.",
            &none,
        );
        assert_eq!(got[0].phrase, "Maria");
        assert_eq!(got[0].observed, "maria");
    }

    #[test]
    fn ignores_punctuation_stopwords_known_and_pure_insertions() {
        assert!(candidates("isso é bom", "isso é bom!", &none).is_empty());
        assert!(
            candidates("eu vou de carro", "eu vou com carro", &none).is_empty(),
            "stopword"
        );
        assert!(candidates("o vai arpe", "o Vyrp", &|t: &str| t == "Vyrp").is_empty());
        assert!(
            candidates("ligar amanhã", "ligar para Ana amanhã", &none).is_empty(),
            "inserção pura"
        );
        assert!(
            candidates("teste 1", "teste 12", &none).is_empty(),
            "só número"
        );
    }

    #[test]
    fn rewrite_is_not_learning() {
        let got = candidates(
            "o relatório sai amanhã cedo",
            "vamos conversar sobre isso depois do almoço",
            &none,
        );
        assert!(got.is_empty());
    }

    #[test]
    fn caps_candidates_and_words() {
        let pasted = "a1 x b1 x c1 x d1 x e1 x f1 x g1 x h1 x i1 x j1";
        let edited = "Aa x Bb x Cc x Dd x Ee x f1 x g1 x h1 x i1 x j1";
        let got = candidates(pasted, edited, &none);
        assert_eq!(got.len(), MAX_CANDIDATES);
        // Troca de 5 palavras por 5 passa do teto de palavras.
        assert!(
            candidates(
                "um dois três quatro cinco seis sete oito nove dez onze doze",
                "um Alfa Beta Gama Delta Épsilon seis sete oito nove dez onze doze",
                &none
            )
            .is_empty()
        );
    }
}
