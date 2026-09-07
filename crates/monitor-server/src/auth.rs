use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{Error as PasswordHashError, SaltString},
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
const ARGON2_MMAP_THRESHOLD_BYTES: usize = 16 * 1024 * 1024;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn mallopt(option: i32, value: i32) -> i32;
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
const M_MMAP_THRESHOLD: i32 = -3;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
static PASSWORD_ALLOCATOR_PREPARED: std::sync::Once = std::sync::Once::new();

pub const PASSWORD_MIN_BYTES: usize = 1;
pub const PASSWORD_MAX_BYTES: usize = 1_024;
pub const SESSION_DURATION_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const LOGIN_FAILURE_LIMIT: u8 = 5;
pub const LOGIN_COOLDOWN_SECONDS: u64 = 5 * 60;
pub const LOGIN_LIMITER_MAX_ENTRIES: usize = 1_024;

#[derive(Debug)]
pub enum AuthError {
    InvalidPasswordLength,
    Clock,
    Random,
    PasswordHash,
    Worker(tokio::task::JoinError),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPasswordLength => write!(
                formatter,
                "password must contain {PASSWORD_MIN_BYTES} to {PASSWORD_MAX_BYTES} bytes"
            ),
            Self::Clock => formatter.write_str("system clock is before the Unix epoch"),
            Self::Random => formatter.write_str("operating system random source failed"),
            Self::PasswordHash => formatter.write_str("password hashing failed"),
            Self::Worker(source) => write!(formatter, "password worker failed: {source}"),
        }
    }
}

impl std::error::Error for AuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Worker(source) => Some(source),
            Self::InvalidPasswordLength | Self::Clock | Self::Random | Self::PasswordHash => None,
        }
    }
}

pub fn validate_password(password: &str) -> Result<(), AuthError> {
    if (PASSWORD_MIN_BYTES..=PASSWORD_MAX_BYTES).contains(&password.len()) {
        Ok(())
    } else {
        Err(AuthError::InvalidPasswordLength)
    }
}

pub async fn hash_password(password: String) -> Result<String, AuthError> {
    validate_password(&password)?;
    prepare_password_allocator();
    tokio::task::spawn_blocking(move || hash_password_blocking(&password))
        .await
        .map_err(AuthError::Worker)?
}

pub async fn verify_password(password: String, encoded_hash: String) -> Result<bool, AuthError> {
    validate_password(&password)?;
    prepare_password_allocator();
    tokio::task::spawn_blocking(move || verify_password_blocking(&password, &encoded_hash))
        .await
        .map_err(AuthError::Worker)?
}

#[inline]
fn prepare_password_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        PASSWORD_ALLOCATOR_PREPARED.call_once(|| {
            // SAFETY: mallopt is the GNU libc allocator API, compiled only for
            // linux/gnu targets. No pointer ownership is involved; this
            // process-global setting is made once before password work begins.
            unsafe {
                let _ = mallopt(M_MMAP_THRESHOLD, ARGON2_MMAP_THRESHOLD_BYTES as i32);
            }
        });
    }
}

fn hash_password_blocking(password: &str) -> Result<String, AuthError> {
    let mut salt_bytes = [0_u8; 16];
    getrandom::fill(&mut salt_bytes).map_err(|_| AuthError::Random)?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|_| AuthError::PasswordHash)?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthError::PasswordHash)
}

fn verify_password_blocking(password: &str, encoded_hash: &str) -> Result<bool, AuthError> {
    let password_hash = PasswordHash::new(encoded_hash).map_err(|_| AuthError::PasswordHash)?;
    match Argon2::default().verify_password(password.as_bytes(), &password_hash) {
        Ok(()) => Ok(true),
        Err(PasswordHashError::Password) => Ok(false),
        Err(_) => Err(AuthError::PasswordHash),
    }
}

pub fn unix_timestamp() -> Result<i64, AuthError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::Clock)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| AuthError::Clock)
}

pub fn random_token() -> Result<[u8; 32], AuthError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| AuthError::Random)?;
    Ok(bytes)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn encode_hex(bytes: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = [0_u8; 64];
    for (index, byte) in bytes.iter().copied().enumerate() {
        output[index * 2] = DIGITS[usize::from(byte >> 4)];
        output[index * 2 + 1] = DIGITS[usize::from(byte & 0x0f)];
    }
    String::from_utf8(output.to_vec()).expect("lowercase hexadecimal is valid UTF-8")
}

pub fn decode_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }

    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = decode_nibble(pair[0])? << 4 | decode_nibble(pair[1])?;
    }
    Some(output)
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

pub fn constant_time_token_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    bool::from(left.ct_eq(right))
}

pub struct LoginLimiter {
    entries: Mutex<HashMap<IpAddr, FailureState>>,
}

#[derive(Clone, Copy)]
struct FailureState {
    failures: u8,
    last_failure: Instant,
    blocked_until: Option<Instant>,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn retry_after(&self, address: IpAddr) -> Option<u64> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|lock| lock.into_inner());
        let state = entries.get(&address).copied()?;
        match state.blocked_until {
            Some(blocked_until) if blocked_until > now => {
                Some(seconds_rounded_up(blocked_until.duration_since(now)))
            }
            Some(_) => {
                entries.remove(&address);
                None
            }
            None => None,
        }
    }

    pub fn record_failure(&self, address: IpAddr) -> Option<u64> {
        let now = Instant::now();
        let cooldown = Duration::from_secs(LOGIN_COOLDOWN_SECONDS);
        let mut entries = self.entries.lock().unwrap_or_else(|lock| lock.into_inner());
        entries.retain(|_, state| now.duration_since(state.last_failure) < cooldown);

        if !entries.contains_key(&address) && entries.len() >= LOGIN_LIMITER_MAX_ENTRIES {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, state)| state.last_failure)
                .map(|(address, _)| *address)
            {
                entries.remove(&oldest);
            }
        }

        let state = entries.entry(address).or_insert(FailureState {
            failures: 0,
            last_failure: now,
            blocked_until: None,
        });
        state.failures = state.failures.saturating_add(1);
        state.last_failure = now;
        if state.failures >= LOGIN_FAILURE_LIMIT {
            let blocked_until = now + cooldown;
            state.blocked_until = Some(blocked_until);
            Some(LOGIN_COOLDOWN_SECONDS)
        } else {
            None
        }
    }

    pub fn clear(&self, address: IpAddr) {
        self.entries
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .remove(&address);
    }
}

impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new()
    }
}

fn seconds_rounded_up(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0))
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    #[test]
    fn strict_hex_and_constant_time_comparison_work() {
        let token = [0xab; 32];
        let encoded = encode_hex(&token);
        assert_eq!(decode_hex(&encoded), Some(token));
        assert!(constant_time_token_eq(&token, &token));

        let different = [0xac; 32];
        assert!(!constant_time_token_eq(&token, &different));
        assert_eq!(decode_hex(&encoded.to_uppercase()), None);
        assert_eq!(decode_hex("00"), None);
    }

    #[test]
    fn mmap_threshold_stays_below_default_argon2_working_set() {
        let default_working_set_bytes = argon2::Params::DEFAULT_M_COST as usize * 1024;
        assert!(ARGON2_MMAP_THRESHOLD_BYTES < default_working_set_bytes);
    }

    #[test]
    fn login_limiter_has_a_hard_entry_bound() {
        let limiter = LoginLimiter::new();
        for index in 0..(LOGIN_LIMITER_MAX_ENTRIES + 100) {
            limiter.record_failure(IpAddr::V6(Ipv6Addr::from(index as u128)));
        }
        assert_eq!(
            limiter
                .entries
                .lock()
                .unwrap_or_else(|lock| lock.into_inner())
                .len(),
            LOGIN_LIMITER_MAX_ENTRIES
        );
    }
}
