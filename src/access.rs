//! Ephemeral single-user credentials. Time is monotonic seconds since process start.
use std::{fs::File, io::Read};

pub(crate) const BOOTSTRAP_TTL: u64 = 300;
pub(crate) const SESSION_TTL: u64 = 3600;
const MAX_FAILURES: u32 = 10;

pub(crate) struct Access {
    bootstrap: String,
    session: String,
    csrf: String,
    created: u64,
    authenticated: Option<u64>,
    failures: u32,
}

fn secret() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn matches_secret(expected: &str, supplied: &str) -> bool {
    if supplied.len() != expected.len() {
        return false;
    }
    expected
        .bytes()
        .zip(supplied.bytes())
        .fold(0u8, |v, (a, b)| v | (a ^ b))
        == 0
}

impl Access {
    pub(crate) fn new(now: u64) -> std::io::Result<(Self, String)> {
        let bootstrap = secret()?;
        Ok((
            Self {
                bootstrap: bootstrap.clone(),
                session: secret()?,
                csrf: secret()?,
                created: now,
                authenticated: None,
                failures: 0,
            },
            bootstrap,
        ))
    }

    pub(crate) fn exchange(&mut self, code: &str, now: u64) -> Option<(String, String)> {
        if self.authenticated.is_some()
            || now.saturating_sub(self.created) >= BOOTSTRAP_TTL
            || self.failures >= MAX_FAILURES
        {
            return None;
        }
        if !matches_secret(&self.bootstrap, code) {
            self.failures += 1;
            return None;
        }
        self.bootstrap.clear();
        self.authenticated = Some(now);
        Some((self.session.clone(), self.csrf.clone()))
    }

    pub(crate) fn authorize(&self, session: &str, now: u64) -> bool {
        self.authenticated
            .is_some_and(|at| now.saturating_sub(at) < SESSION_TTL)
            && matches_secret(&self.session, session)
    }

    pub(crate) fn mutation_allowed(&self, session: &str, csrf: &str, now: u64) -> bool {
        self.authorize(session, now) && matches_secret(&self.csrf, csrf)
    }

    pub(crate) fn csrf(&self, session: &str, now: u64) -> Option<&str> {
        self.authorize(session, now).then_some(self.csrf.as_str())
    }

    pub(crate) fn logout(&mut self) {
        self.authenticated = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_is_single_use_and_sessions_expire() {
        let (mut access, code) = Access::new(100).unwrap();
        assert!(access.exchange("wrong", 101).is_none());
        let (session, csrf) = access.exchange(&code, 102).unwrap();
        assert!(access.exchange(&code, 103).is_none());
        assert!(access.authorize(&session, 104));
        assert!(access.mutation_allowed(&session, &csrf, 104));
        assert!(!access.mutation_allowed(&session, "wrong", 104));
        assert!(!access.authorize(&session, 102 + SESSION_TTL));
        let (fresh, _) = Access::new(100).unwrap();
        assert!(!fresh.authorize(&session, 104));
    }

    #[test]
    fn expired_or_excessively_guessed_codes_are_rejected() {
        let (mut expired, code) = Access::new(0).unwrap();
        assert!(expired.exchange(&code, BOOTSTRAP_TTL).is_none());
        let (mut locked, code) = Access::new(0).unwrap();
        for _ in 0..MAX_FAILURES {
            assert!(locked.exchange("bad", 1).is_none());
        }
        assert!(locked.exchange(&code, 1).is_none());
    }
}
