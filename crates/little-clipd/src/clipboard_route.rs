//! One identity-scoped clipboard override, including its terminal observation.

use little_clip_core::{HandSide, ipc::ClipboardRouteStatus};
use tokio::time::Instant;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct ClipboardRoute {
    current: Option<Route>,
}

struct Route {
    id: Option<Uuid>,
    hand: HandSide,
    expires: Instant,
    restore: bool,
    status: ClipboardRouteStatus,
}

impl ClipboardRoute {
    pub(super) fn install(
        &mut self,
        id: Option<Uuid>,
        hand: HandSide,
        expires: Instant,
        restore: bool,
    ) {
        self.current = Some(Route {
            id,
            hand,
            expires,
            restore,
            status: ClipboardRouteStatus::Pending,
        });
    }

    pub(super) fn status(&self, id: Uuid, now: Instant) -> ClipboardRouteStatus {
        match &self.current {
            Some(route) if route.id == Some(id) => route.status(now),
            _ => ClipboardRouteStatus::Superseded,
        }
    }

    pub(super) fn cancel(&mut self, id: Uuid, now: Instant) -> ClipboardRouteStatus {
        if let Some(route) = &mut self.current {
            if route.id == Some(id) && route.status(now) == ClipboardRouteStatus::Pending {
                route.status = ClipboardRouteStatus::Cancelled;
            }
        }
        self.status(id, now)
    }

    pub(super) fn take(&mut self, now: Instant) -> (HandSide, bool) {
        if let Some(route) = &mut self.current {
            if route.status(now) == ClipboardRouteStatus::Pending {
                route.status = ClipboardRouteStatus::Claimed;
                return (route.hand, route.restore);
            }
        }
        (HandSide::Left, false)
    }

    pub(super) fn targets_right(&self, now: Instant) -> bool {
        self.current.as_ref().is_some_and(|route| {
            route.hand == HandSide::Right && route.status(now) == ClipboardRouteStatus::Pending
        })
    }
}

impl Route {
    fn status(&self, now: Instant) -> ClipboardRouteStatus {
        if self.status == ClipboardRouteStatus::Pending && now >= self.expires {
            ClipboardRouteStatus::Expired
        } else {
            self.status
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cancellation_cannot_remove_a_newer_route_or_revoke_a_claim() {
        let mut route = ClipboardRoute::default();
        let now = Instant::now();
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        route.install(
            Some(old),
            HandSide::Left,
            now + Duration::from_secs(2),
            false,
        );
        route.install(
            Some(new),
            HandSide::Right,
            now + Duration::from_secs(2),
            true,
        );
        assert_eq!(route.cancel(old, now), ClipboardRouteStatus::Superseded);
        assert!(route.targets_right(now));
        assert_eq!(route.take(now), (HandSide::Right, true));
        assert_eq!(route.cancel(new, now), ClipboardRouteStatus::Claimed);
        assert_eq!(route.take(now), (HandSide::Left, false));
    }

    #[test]
    fn cancellation_expiry_and_legacy_routes_do_not_affect_unrelated_capture() {
        let mut route = ClipboardRoute::default();
        let now = Instant::now();
        let id = Uuid::new_v4();
        route.install(
            Some(id),
            HandSide::Right,
            now + Duration::from_secs(2),
            true,
        );
        assert_eq!(route.cancel(id, now), ClipboardRouteStatus::Cancelled);
        assert_eq!(route.take(now), (HandSide::Left, false));
        route.install(Some(id), HandSide::Right, now, true);
        assert_eq!(route.cancel(id, now), ClipboardRouteStatus::Expired);
        assert_eq!(route.take(now), (HandSide::Left, false));
        route.install(None, HandSide::Right, now + Duration::from_secs(2), true);
        assert_eq!(route.cancel(id, now), ClipboardRouteStatus::Superseded);
        assert_eq!(route.take(now), (HandSide::Right, true));
    }
}
