//! Bounded in-memory registry of recently-seen chat users + credibility scoring.
//! RAM-bounded (LRU-ish: evicts the least-recently-seen when over capacity).
//! Account age is filled lazily via Helix; everything else comes from IRC badges.

use crate::types::ChatEvent;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct UserProfile {
    pub user_id: String,
    pub login: String,
    pub display: String,
    pub sub_months: u32,
    pub is_mod: bool,
    pub is_vip: bool,
    pub is_sub: bool,
    pub is_founder: bool,
    pub is_broadcaster: bool,
    pub msg_count: u32,
    pub account_age_days: Option<i64>,
    pub first_seen: Instant,
    pub last_seen: Instant,
}

pub struct Registry {
    cap: usize,
    users: HashMap<String, UserProfile>,
}

impl Registry {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            users: HashMap::new(),
        }
    }

    /// Insert or update a profile from a chat event.
    pub fn observe(&mut self, ev: &ChatEvent) {
        let now = Instant::now();
        if let Some(p) = self.users.get_mut(&ev.user_id) {
            p.display = ev.user.clone();
            p.login = ev.login.clone();
            p.sub_months = ev.sub_months.max(p.sub_months);
            p.is_mod = ev.is_mod;
            p.is_vip = ev.is_vip;
            p.is_sub = ev.is_sub;
            p.is_founder = ev.is_founder;
            p.is_broadcaster = ev.is_broadcaster;
            p.msg_count = p.msg_count.saturating_add(1);
            p.last_seen = now;
            return;
        }
        if self.users.len() >= self.cap {
            self.evict_oldest();
        }
        self.users.insert(
            ev.user_id.clone(),
            UserProfile {
                user_id: ev.user_id.clone(),
                login: ev.login.clone(),
                display: ev.user.clone(),
                sub_months: ev.sub_months,
                is_mod: ev.is_mod,
                is_vip: ev.is_vip,
                is_sub: ev.is_sub,
                is_founder: ev.is_founder,
                is_broadcaster: ev.is_broadcaster,
                msg_count: 1,
                account_age_days: None,
                first_seen: now,
                last_seen: now,
            },
        );
    }

    fn evict_oldest(&mut self) {
        if let Some(id) = self
            .users
            .iter()
            .min_by_key(|(_, p)| p.last_seen)
            .map(|(id, _)| id.clone())
        {
            self.users.remove(&id);
        }
    }

    pub fn get(&self, user_id: &str) -> Option<&UserProfile> {
        self.users.get(user_id)
    }

    pub fn needs_age(&self, user_id: &str) -> bool {
        self.users
            .get(user_id)
            .map(|p| p.account_age_days.is_none())
            .unwrap_or(false)
    }

    pub fn set_age(&mut self, user_id: &str, days: i64) {
        if let Some(p) = self.users.get_mut(user_id) {
            p.account_age_days = Some(days);
        }
    }

    pub fn len(&self) -> usize {
        self.users.len()
    }

    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }
}

/// Derived credibility / "tempo de casa".
pub struct Credibility {
    pub tier: &'static str,
    /// 0.0 (brand-new) .. 1.0 (broadcaster).
    pub trust: f32,
}

pub fn credibility(p: &UserProfile) -> Credibility {
    if p.is_broadcaster {
        return Credibility { tier: "a própria streamer", trust: 1.0 };
    }
    if p.is_mod {
        return Credibility { tier: "moderador", trust: 0.95 };
    }
    if p.is_vip {
        return Credibility { tier: "VIP", trust: 0.85 };
    }
    if p.is_founder {
        return Credibility { tier: "founder", trust: 0.8 };
    }
    let mut trust = 0.2;
    if p.is_sub {
        trust += 0.3;
    }
    trust += (p.sub_months as f32 / 24.0).min(0.3);
    if let Some(days) = p.account_age_days {
        trust += (days as f32 / 365.0 / 5.0).min(0.2);
    }
    let trust = trust.min(1.0);
    Credibility {
        tier: if trust >= 0.5 { "regular" } else { "viewer novato" },
        trust,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str) -> ChatEvent {
        ChatEvent {
            user: id.into(),
            login: id.to_lowercase(),
            user_id: id.into(),
            message_id: "m".into(),
            text: "oi".into(),
            sub_months: 0,
            is_mod: false,
            is_vip: false,
            is_sub: false,
            is_founder: false,
            is_broadcaster: false,
        }
    }

    #[test]
    fn observe_counts_and_caps() {
        let mut r = Registry::new(2);
        r.observe(&ev("a"));
        r.observe(&ev("a"));
        assert_eq!(r.get("a").unwrap().msg_count, 2);
        r.observe(&ev("b"));
        r.observe(&ev("c"));
        assert!(r.len() <= 2, "registry must stay within capacity");
    }

    #[test]
    fn credibility_tiers() {
        let mut bc = ev("s");
        bc.is_broadcaster = true;
        assert!((credibility(&{ let mut r = Registry::new(4); r.observe(&bc); r.get("s").unwrap().clone() }).trust - 1.0).abs() < 1e-6);

        let newbie = {
            let mut r = Registry::new(4);
            r.observe(&ev("n"));
            r.get("n").unwrap().clone()
        };
        let c = credibility(&newbie);
        assert!(c.trust < 0.5);
        assert_eq!(c.tier, "viewer novato");
    }

    #[test]
    fn long_sub_is_regular() {
        let mut e = ev("u");
        e.is_sub = true;
        e.sub_months = 20;
        let mut r = Registry::new(4);
        r.observe(&e);
        let c = credibility(r.get("u").unwrap());
        assert!(c.trust >= 0.5);
        assert_eq!(c.tier, "regular");
    }
}
