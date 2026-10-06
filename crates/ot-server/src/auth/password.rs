//! Password rules (fixed: [`super::stig`]): length and character classes
//! when a password is set, how much a change must differ, and a generator
//! for passwords that meet them.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use super::stig::{PASSWORD_MIN_CHANGED_CHARS, PASSWORD_MIN_LENGTH};

/// The bundled list of common passwords (IA-5(1)(a)).
const COMMON: &str = include_str!("common-passwords.txt");
/// A site's own list, beside the database, added to the bundled one.
pub const SITE_LIST: &str = "common-passwords.txt";
/// A password's letters spelling a listed password this long or longer
/// count as that password.
const MIN_CORE: usize = 6;

static LIST: OnceLock<HashSet<String>> = OnceLock::new();
static SITE_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Where the site's own list is looked for (the data directory). Call once,
/// before the first check.
pub fn set_site_dir(dir: &Path) {
    let _ = SITE_DIR.set(dir.to_path_buf());
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_lowercase)
}

fn list() -> &'static HashSet<String> {
    LIST.get_or_init(|| {
        let mut set: HashSet<String> = words(COMMON).collect();
        if let Some(dir) = SITE_DIR.get() {
            let path = dir.join(SITE_LIST);
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let before = set.len();
                    set.extend(words(&text));
                    tracing::info!(path = %path.display(), added = set.len() - before, "site password list loaded");
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "site password list not read"),
            }
        }
        set
    })
}

/// Whether `p` is a common or compromised password: one on the list
/// (ignoring case), or whose letters alone spell one (`Password2026!!!`).
pub fn is_common(p: &str) -> bool {
    let l = list();
    let lower = p.to_lowercase();
    if l.contains(&lower) {
        return true;
    }
    let core: String = lower.chars().filter(|c| c.is_alphabetic()).collect();
    core.chars().count() >= MIN_CORE && l.contains(&core)
}

/// Longest password accepted (hashing is slow on purpose).
const MAX_LENGTH: usize = 256;

/// Whether `p` may be set as a password; the error lists every rule it
/// misses.
pub fn check(p: &str) -> Result<(), String> {
    super::check_password(p)?;
    let n = p.chars().count();
    if n > MAX_LENGTH {
        return Err(format!("a password has at most {MAX_LENGTH} characters"));
    }
    let mut missing = Vec::new();
    if n < PASSWORD_MIN_LENGTH {
        missing.push(format!("at least {PASSWORD_MIN_LENGTH} characters"));
    }
    let has = |f: fn(char) -> bool| p.chars().any(f);
    if !has(char::is_uppercase) {
        missing.push("an upper-case letter".into());
    }
    if !has(char::is_lowercase) {
        missing.push("a lower-case letter".into());
    }
    if !has(|c| c.is_ascii_digit()) {
        missing.push("a digit".into());
    }
    if !has(|c| !c.is_alphanumeric()) {
        missing.push("a special character (not a letter or digit)".into());
    }
    if !missing.is_empty() {
        return Err(format!("the password needs {}", missing.join(", ")));
    }
    if is_common(p) {
        return Err(
            "the password is, or is built from, a commonly used password: choose another".into(),
        );
    }
    Ok(())
}

/// Whether `new` differs enough from `current` (your own change).
pub fn check_change(current: &str, new: &str) -> Result<(), String> {
    if changed_chars(current, new) < PASSWORD_MIN_CHANGED_CHARS {
        return Err(format!(
            "change at least {PASSWORD_MIN_CHANGED_CHARS} characters of the current password"
        ));
    }
    Ok(())
}

/// Characters to insert, delete or replace to turn `a` into `b`.
pub fn changed_chars(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = if ca == cb {
                diag
            } else {
                1 + diag.min(row[j]).min(row[j + 1])
            };
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// A random password meeting any policy up to 26 characters: 80 bits of
/// randomness with every character class.
pub fn generate() -> String {
    // Random from the FIPS module's DRBG; the fixed parts give every class.
    // Drawn again in the rare case its letters spell a common password.
    loop {
        let a = crate::fips::random_hex::<5>();
        let b = crate::fips::random_hex::<5>();
        let p = format!("Ot-{a}-{}!7a", b.to_uppercase());
        if !is_common(&p) {
            return p;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_passwords_and_ones_built_from_them_are_refused() {
        assert!(is_common("password"));
        assert!(is_common("PASSWORD"));
        assert!(
            check("Password2026!!!").is_err(),
            "letters spell a listed password"
        );
        assert!(check("Qwerty!!2026qwerty").is_err());
        assert!(check("Tangerine-Otter-42-Glass").is_ok());
        assert!(!is_common("x7!Kv2#pQ9zL$m4W"));
    }

    #[test]
    fn passwords_meet_the_policy() {
        assert!(check("Correct-Horse-7-Battery").is_ok());
        let e = check("correcthorsebattery").unwrap_err();
        assert!(
            e.contains("upper-case") && e.contains("digit") && e.contains("special"),
            "{e}"
        );
        assert!(check("Sh0rt!pass").unwrap_err().contains("15"));
        assert!(check("short").is_err(), "the floor always holds");
        assert!(check(&"Aa1!".repeat(100)).is_err(), "too long");
        assert!(check(&generate()).is_ok());
        assert_ne!(generate(), generate());
    }

    #[test]
    fn a_change_must_differ_enough() {
        assert_eq!(changed_chars("kitten", "sitting"), 3);
        assert_eq!(changed_chars("", "abc"), 3);
        assert!(check_change("Correct-Horse-7-Battery", "Correct-Horse-8-Battery").is_err());
        assert!(check_change("Correct-Horse-7-Battery", "Purple-Monkey-42-Dishwasher").is_ok());
    }
}
