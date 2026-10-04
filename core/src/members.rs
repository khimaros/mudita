//! who else serves a file, as one of its peers knows it (DESIGN.md "peer
//! exchange"): the routes read in slots, the callers that gave theirs, and
//! the ones others spoke of. pure: the caller supplies the time.

use crate::protocol::{PEERS_MAX, SLOT_STALE_SECONDS};

// enough to pass on a full reply with room for ones only heard of.
const MEMBERS_MAX: usize = 4 * PEERS_MAX;

struct Member {
    route: Vec<u8>,
    // when there was last word of it, first or second hand.
    heard: u64,
    // whether this peer has its own evidence that the member is up: its
    // fresh slot, a call from it, or an answer from it.
    met: bool,
}

/// the other peers of one file.
#[derive(Default)]
pub struct Members {
    members: Vec<Member>,
}

impl Members {
    fn fresh(member: &Member, now: u64) -> bool {
        now.saturating_sub(member.heard) <= SLOT_STALE_SECONDS
    }

    // the oldest go first when there are too many, after the stale ones.
    fn trim(&mut self, now: u64) {
        self.members.retain(|member| Self::fresh(member, now));
        self.members
            .sort_by_key(|member| std::cmp::Reverse(member.heard));
        self.members.truncate(MEMBERS_MAX);
    }

    /// there is direct word of the peer at `route`: its slot was read fresh,
    /// it called, or it answered.
    pub fn met(&mut self, route: &[u8], now: u64) {
        if route.is_empty() {
            return;
        }
        self.members.retain(|member| member.route != route);
        self.members.push(Member {
            route: route.to_vec(),
            heard: now,
            met: true,
        });
        self.trim(now);
    }

    /// another peer spoke of the one at `route`. that makes it worth
    /// calling, but it is not passed on until it has been met: otherwise two
    /// peers could keep one that is gone alive by telling each other of it
    /// for ever. for the same reason hearing of a known one changes nothing.
    pub fn heard_of(&mut self, route: &[u8], now: u64) {
        let known = self.members.iter().any(|member| member.route == route);
        if route.is_empty() || known {
            return;
        }
        self.members.push(Member {
            route: route.to_vec(),
            heard: now,
            met: false,
        });
        self.trim(now);
    }

    /// the peers to tell a caller of: those met, most recent first, without
    /// the caller itself and without this peer's `own` route.
    pub fn to_pass_on(&self, now: u64, caller: &[u8], own: &[u8]) -> Vec<Vec<u8>> {
        let mut met: Vec<&Member> = self
            .members
            .iter()
            .filter(|member| member.met && Self::fresh(member, now))
            .filter(|member| member.route != caller && member.route != own)
            .collect();
        met.sort_by_key(|member| std::cmp::Reverse(member.heard));
        let routes = met.into_iter().take(PEERS_MAX);
        routes.map(|member| member.route.clone()).collect()
    }

    /// the peers worth calling: every one there is fresh word of, met or
    /// not, without this peer's `own` route.
    pub fn to_call(&self, now: u64, own: &[u8]) -> Vec<Vec<u8>> {
        let fresh = self
            .members
            .iter()
            .filter(|member| Self::fresh(member, now) && member.route != own);
        fresh.map(|member| member.route.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn route(n: u8) -> Vec<u8> {
        vec![n]
    }

    #[test]
    fn peers_met_are_passed_on_most_recent_first() {
        let mut members = Members::default();
        members.met(&route(1), NOW);
        members.met(&route(2), NOW + 5);
        members.met(&route(1), NOW + 9);
        assert_eq!(members.to_pass_on(NOW + 9, &[], &[]), [route(1), route(2)]);
    }

    #[test]
    fn a_caller_is_not_told_of_itself_or_of_us() {
        let mut members = Members::default();
        for n in 1..=3 {
            members.met(&route(n), NOW);
        }
        let told = members.to_pass_on(NOW, &route(2), &route(3));
        assert_eq!(told, [route(1)]);
    }

    #[test]
    fn a_peer_not_heard_from_goes_stale() {
        let mut members = Members::default();
        members.met(&route(1), NOW);
        let later = NOW + SLOT_STALE_SECONDS;
        assert_eq!(members.to_pass_on(later, &[], &[]), [route(1)]);
        assert!(members.to_pass_on(later + 1, &[], &[]).is_empty());
        assert!(members.to_call(later + 1, &[]).is_empty());
    }

    #[test]
    fn a_peer_only_heard_of_is_called_but_not_passed_on() {
        let mut members = Members::default();
        members.heard_of(&route(7), NOW);
        assert!(members.to_pass_on(NOW, &[], &[]).is_empty());
        assert_eq!(members.to_call(NOW, &[]), [route(7)]);
        members.met(&route(7), NOW + 1);
        assert_eq!(members.to_pass_on(NOW + 1, &[], &[]), [route(7)]);
    }

    #[test]
    fn hearing_of_a_peer_again_does_not_keep_it_alive() {
        let mut members = Members::default();
        members.met(&route(1), NOW);
        members.heard_of(&route(1), NOW + SLOT_STALE_SECONDS);
        assert!(members
            .to_pass_on(NOW + SLOT_STALE_SECONDS + 1, &[], &[])
            .is_empty());
        // nor does being heard of twice refresh one never met.
        members.heard_of(&route(2), NOW);
        members.heard_of(&route(2), NOW + SLOT_STALE_SECONDS);
        assert!(members
            .to_call(NOW + SLOT_STALE_SECONDS + 1, &[])
            .is_empty());
    }

    #[test]
    fn an_empty_route_names_nobody() {
        let mut members = Members::default();
        members.met(&[], NOW);
        members.heard_of(&[], NOW);
        assert!(members.to_call(NOW, &[]).is_empty());
    }

    #[test]
    fn a_reply_holds_at_most_the_newest_and_memory_is_bounded() {
        let mut members = Members::default();
        for n in 0..200u8 {
            members.met(&route(n), NOW + u64::from(n));
        }
        let told = members.to_pass_on(NOW + 200, &[], &[]);
        assert_eq!(told.len(), PEERS_MAX);
        assert_eq!(told[0], route(199));
        assert_eq!(members.to_call(NOW + 200, &[]).len(), MEMBERS_MAX);
    }
}
