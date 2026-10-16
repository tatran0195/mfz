//! Deterministic, bounded target selection independent of SDK/network fixtures.
use std::collections::BTreeSet;

use crate::error::{AppError, Result};

#[derive(Default)]
pub(crate) struct UniqueInstanceSelection {
    instance: Option<String>,
    pages: usize,
    tokens: BTreeSet<String>,
}

impl UniqueInstanceSelection {
    pub(crate) fn observe(&mut self, instance: &str) -> Result<()> {
        if instance.trim().is_empty() {
            return Err(AppError::Aws("EC2 returned an empty instance ID".into()));
        }
        match &self.instance {
            Some(existing) if existing != instance => Err(AppError::Aws(
                "Instance tag matches multiple running instances; use a unique tag or an explicit instance ID".into(),
            )),
            Some(_) => Ok(()),
            None => { self.instance = Some(instance.to_owned()); Ok(()) }
        }
    }

    pub(crate) fn next_page(&mut self, token: Option<&str>) -> Result<Option<String>> {
        self.pages += 1;
        let Some(token) = token.filter(|token| !token.is_empty()) else {
            return Ok(None);
        };
        if self.pages >= 100 || !self.tokens.insert(token.to_owned()) {
            return Err(AppError::Aws(
                "EC2 pagination did not converge within the bounded search".into(),
            ));
        }
        Ok(Some(token.to_owned()))
    }

    pub(crate) fn finish(self) -> Result<String> {
        self.instance
            .ok_or_else(|| AppError::Aws("No running instance matches the specified tag".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pages_and_duplicate_records_preserve_one_unique_target() {
        let mut selection = UniqueInstanceSelection::default();
        assert_eq!(
            selection.next_page(Some("page2")).unwrap().as_deref(),
            Some("page2")
        );
        selection.observe("i-target").unwrap();
        selection.observe("i-target").unwrap();
        assert!(selection.next_page(None).unwrap().is_none());
        assert_eq!(selection.finish().unwrap(), "i-target");
    }

    #[test]
    fn another_target_on_any_page_is_not_arbitrarily_selected() {
        let mut selection = UniqueInstanceSelection::default();
        selection.observe("i-staging").unwrap();
        selection.next_page(Some("next")).unwrap();
        assert!(selection.observe("i-production").is_err());
        assert!(UniqueInstanceSelection::default().finish().is_err());
    }

    #[test]
    fn repeated_and_unbounded_page_tokens_fail_explicitly() {
        let mut selection = UniqueInstanceSelection::default();
        selection.next_page(Some("repeat")).unwrap();
        assert!(selection.next_page(Some("repeat")).is_err());
        let mut selection = UniqueInstanceSelection::default();
        for page in 0..99 {
            selection.next_page(Some(&page.to_string())).unwrap();
        }
        assert!(selection.next_page(Some("limit")).is_err());
        assert!(UniqueInstanceSelection::default().observe("").is_err());
    }
}
