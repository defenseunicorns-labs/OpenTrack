//! The account policy, fixed at the DoD ASD Application Security and
//! Development STIG values. None of it is a setting: older saved sign-in
//! settings that still hold these values are read and ignored.

use serde_json::{Value, json};

use super::Role;

/// A password is at least this many characters (IA-5(1)).
pub const PASSWORD_MIN_LENGTH: usize = 15;
/// A new password may not be any of the last this many (IA-5(1)).
pub const PASSWORD_HISTORY: usize = 5;
/// Changing your own password, at least this many characters must differ
/// from the current one (edit distance; IA-5(1)).
pub const PASSWORD_MIN_CHANGED_CHARS: usize = 8;
/// Hours before you may change your own password again; an admin's reset
/// and a required change are exempt (IA-5(1)).
pub const PASSWORD_MIN_AGE_HOURS: i64 = 24;
/// Days a password lasts; then it must change at the next sign-in (IA-5(1)).
pub const PASSWORD_MAX_AGE_DAYS: i64 = 60;

/// Consecutive failed password sign-ins that lock an account (AC-7)...
pub const LOCKOUT_FAILURES: u32 = 3;
/// ...within this many minutes...
pub const LOCKOUT_WINDOW_MINUTES: i64 = 15;
/// ...lock it until an admin unlocks it (Settings -> Users, `opentrack user
/// unlock`): the Container Platform SRG's AC-7 (V-233165), stricter than the
/// ASD STIG's timed lock.
pub const LOCK_UNTIL_UNLOCKED: bool = true;

/// Minutes without use that end a browser session (AC-11, AC-12). API
/// tokens are not sessions: they only expire.
pub const IDLE_MINUTES: i64 = 15;
/// The same for admins.
pub const ADMIN_IDLE_MINUTES: i64 = 10;
/// Sessions an account may have at once; a new sign-in ends the oldest
/// (AC-10).
pub const SESSIONS_PER_ACCOUNT: usize = 3;
/// How long a session lasts at most, used or not (AC-12).
pub const SESSION_HOURS: i64 = 24;

/// A temporary or emergency account is turned off this many hours after it
/// is made (AC-2(2)).
pub const TEMPORARY_ACCOUNT_HOURS: i64 = 72;

/// Accounts not signed in for this many days are turned off, except the
/// break-glass accounts listed in the sign-in settings (AC-2(3)).
pub const DISABLE_INACTIVE_AFTER_DAYS: i64 = 35;

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

pub const PASSWORD_MIN_AGE_MS: i64 = PASSWORD_MIN_AGE_HOURS * HOUR_MS;
pub const PASSWORD_MAX_AGE_MS: i64 = PASSWORD_MAX_AGE_DAYS * DAY_MS;
pub const LOCKOUT_WINDOW_MS: i64 = LOCKOUT_WINDOW_MINUTES * MINUTE_MS;
/// How long a lock lasts, as the store takes it: 0 is until an admin unlocks.
pub const LOCK_MS: i64 = if LOCK_UNTIL_UNLOCKED {
    0
} else {
    15 * MINUTE_MS
};
pub const INACTIVE_MS: i64 = DISABLE_INACTIVE_AFTER_DAYS * DAY_MS;

/// The idle timeout for an account of `role`, in ms.
pub fn idle_ms(role: Role) -> i64 {
    match role {
        Role::Admin => ADMIN_IDLE_MINUTES * MINUTE_MS,
        _ => IDLE_MINUTES * MINUTE_MS,
    }
}

/// The password rules, as `/auth/me` shows them to the password forms.
pub fn password_policy() -> Value {
    json!({
        "min_length": PASSWORD_MIN_LENGTH,
        "require_upper": true,
        "require_lower": true,
        "require_digit": true,
        "require_special": true,
        "history": PASSWORD_HISTORY,
        "min_changed_chars": PASSWORD_MIN_CHANGED_CHARS,
        "min_age_hours": PASSWORD_MIN_AGE_HOURS,
        "max_age_days": PASSWORD_MAX_AGE_DAYS,
        "refuse_common": true,
    })
}
