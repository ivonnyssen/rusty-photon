//! [`InMemoryTargetStore`], a clock-free, filesystem-free [`TargetStore`]
//! test double.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::error::TargetStoreError;
use crate::model::{validate_goals, AcquisitionGoal, Target, TargetSlug};
use crate::TargetStore;

/// In-memory [`TargetStore`] test double: a `BTreeMap` behind a `Mutex`,
/// gated the same as `MockTargetStore` (`cfg(any(test, feature =
/// "mock"))`). Gives `rp`'s planner deterministic unit tests without a
/// temp database. Offered alongside — not instead of — the `mockall`
/// automock for tests that want call assertions.
#[derive(Debug, Default)]
pub struct InMemoryTargetStore {
    targets: Mutex<BTreeMap<String, Target>>,
}

impl InMemoryTargetStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TargetStore for InMemoryTargetStore {
    async fn upsert_target(&self, mut target: Target) -> Result<(), TargetStoreError> {
        validate_goals(&target.goals)?;
        let mut targets = self.targets.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = targets.get(target.slug.as_str()) {
            target.created_at.clone_from(&existing.created_at);
        }
        targets.insert(target.slug.as_str().to_string(), target);
        Ok(())
    }

    async fn get_target(&self, slug: &TargetSlug) -> Result<Option<Target>, TargetStoreError> {
        let targets = self.targets.lock().unwrap_or_else(|p| p.into_inner());
        Ok(targets.get(slug.as_str()).cloned())
    }

    async fn list_targets(&self) -> Result<Vec<Target>, TargetStoreError> {
        let targets = self.targets.lock().unwrap_or_else(|p| p.into_inner());
        Ok(targets.values().cloned().collect())
    }

    async fn delete_target(&self, slug: &TargetSlug) -> Result<bool, TargetStoreError> {
        let mut targets = self.targets.lock().unwrap_or_else(|p| p.into_inner());
        Ok(targets.remove(slug.as_str()).is_some())
    }

    async fn set_goals(
        &self,
        slug: &TargetSlug,
        goals: Vec<AcquisitionGoal>,
    ) -> Result<(), TargetStoreError> {
        validate_goals(&goals)?;
        let mut targets = self.targets.lock().unwrap_or_else(|p| p.into_inner());
        let target = targets
            .get_mut(slug.as_str())
            .ok_or_else(|| TargetStoreError::NotFound {
                slug: slug.as_str().to_string(),
            })?;
        target.goals = goals;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn sample_target(slug: &str) -> Target {
        Target {
            slug: TargetSlug::new(slug).unwrap(),
            display_name: "M33".to_string(),
            ra_hours: 1.5642,
            dec_degrees: 30.6602,
            catalog_ref: Some("M33".to_string()),
            object_type: Some("Galaxy".to_string()),
            magnitude: Some(5.7),
            size_arcmin: Some(62.0),
            priority: 0,
            active: true,
            goals: Vec::new(),
            scheduling: None,
            grading: None,
            notes: None,
            created_at: "2026-07-22T00:00:00Z".to_string(),
            updated_at: "2026-07-22T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn upsert_then_get_round_trips() {
        let store = InMemoryTargetStore::new();
        let target = sample_target("m33");
        store.upsert_target(target.clone()).await.unwrap();
        let slug = TargetSlug::new("m33").unwrap();
        assert_eq!(store.get_target(&slug).await.unwrap(), Some(target));
    }

    #[tokio::test]
    async fn upsert_of_existing_slug_overwrites_in_place() {
        let store = InMemoryTargetStore::new();
        store.upsert_target(sample_target("m33")).await.unwrap();

        let mut updated = sample_target("m33");
        updated.priority = 5;
        store.upsert_target(updated).await.unwrap();

        assert_eq!(store.list_targets().await.unwrap().len(), 1);
        let slug = TargetSlug::new("m33").unwrap();
        assert_eq!(store.get_target(&slug).await.unwrap().unwrap().priority, 5);
    }

    #[tokio::test]
    async fn delete_present_vs_absent() {
        let store = InMemoryTargetStore::new();
        store.upsert_target(sample_target("m33")).await.unwrap();
        let slug = TargetSlug::new("m33").unwrap();
        assert!(store.delete_target(&slug).await.unwrap());
        assert!(!store.delete_target(&slug).await.unwrap());
    }

    #[tokio::test]
    async fn set_goals_on_absent_slug_is_not_found() {
        let store = InMemoryTargetStore::new();
        let slug = TargetSlug::new("m33").unwrap();
        let err = store.set_goals(&slug, Vec::new()).await.unwrap_err();
        assert!(matches!(err, TargetStoreError::NotFound { .. }));
    }
}
