//! Tracking mappers — `Tracking` → `TrackingModel`.

use docbit_contracts::tracking::TrackingModel;

use crate::entities::Tracking;

impl From<&Tracking> for TrackingModel {
    fn from(e: &Tracking) -> Self {
        Self {
            id: e.id.clone(),
            path: e.path.clone(),
            method: e.method.clone(),
            ip: e.ip.clone(),
            user_agent: e.user_agent.clone(),
            referer: e.referer.clone(),
            status: e.status,
            duration_ms: e.duration_ms,
            visited_at: e.visited_at,
        }
    }
}
