//! Password strength, passphrase generation, and the duress distance rule.

use zeroize::Zeroizing;

/// EFF large wordlist, CC BY 3.0 US, https://www.eff.org/dice
const WORDLIST: &str = include_str!("eff_large_wordlist.txt");
const WORDS: usize = 6;

/// `None` if strong enough (zxcvbn score 3 of 4), else what to fix.
pub fn weakness(password: &str, avoid: &[&str]) -> Option<String> {
    let estimate = zxcvbn::zxcvbn(password, avoid);
    if u8::from(estimate.score()) >= 3 {
        return None;
    }
    let hint = estimate
        .feedback()
        .and_then(|f| {
            f.warning()
                .map(|w| w.to_string())
                .or_else(|| f.suggestions().first().map(|s| s.to_string()))
        })
        .unwrap_or_default();
    Some(format!("password too weak (Ctrl+G suggests one). {hint}"))
}

/// Six random words, ~77 bits.
pub fn generate() -> Zeroizing<String> {
    let words: Vec<&str> = WORDLIST
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .collect();
    let mut out = Zeroizing::new(String::new());
    for i in 0..WORDS {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(words[uniform(words.len())]);
    }
    out
}

/// Rejection sampling, so every word is equally likely.
fn uniform(n: usize) -> usize {
    let n = n as u32;
    let limit = u32::MAX - u32::MAX % n;
    loop {
        // a failing OS RNG yields u32::MAX and loops; getrandom only fails on broken systems.
        let r = getrandom::u32().unwrap_or(u32::MAX);
        if r < limit {
            return (r % n) as usize;
        }
    }
}

/// Levenshtein distance in characters. ADR-003: duress must differ by at least 4,
/// so a typo in the real password never wipes the data.
pub fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_phrase_is_strong_and_varies() {
        let a = generate();
        assert_eq!(a.split(' ').count(), WORDS);
        assert_eq!(weakness(&a, &[]), None);
        assert_ne!(*a, *generate());
        assert!(weakness("password123", &[]).is_some());
    }

    #[test]
    fn levenshtein() {
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("пароль", "пароль1"), 1);
    }
}
