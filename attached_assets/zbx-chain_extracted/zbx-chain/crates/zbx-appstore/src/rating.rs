//! App rating system — per-user 1–5 star ratings with aggregate summary.

use serde::{Deserialize, Serialize};
use crate::error::AppStoreError;

/// A single rating submitted by a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatingRecord {
    /// Reviewer wallet address.
    pub reviewer: String,
    /// Star rating: 1–5.
    pub stars: u8,
    /// Optional review text (max 1024 chars).
    pub review: Option<String>,
    /// Block number when rating was submitted.
    pub block_number: u64,
}

impl RatingRecord {
    pub fn new(reviewer: String, stars: u8, review: Option<String>, block_number: u64)
        -> Result<Self, AppStoreError>
    {
        if stars < 1 || stars > 5 {
            return Err(AppStoreError::InvalidRating(stars));
        }
        Ok(Self { reviewer, stars, review, block_number })
    }
}

/// Aggregate rating summary for an app.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RatingSummary {
    /// Total number of ratings.
    pub count: u64,
    /// Sum of all star values (for computing average).
    pub total_stars: u64,
    /// Distribution: index 0 = 1-star count, index 4 = 5-star count.
    pub distribution: [u64; 5],
}

impl RatingSummary {
    /// Compute the average rating (0.0 if no ratings).
    pub fn average(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.total_stars as f64 / self.count as f64
        }
    }

    /// Add a new rating to the summary.
    pub fn add(&mut self, stars: u8) {
        self.count += 1;
        self.total_stars += stars as u64;
        self.distribution[(stars - 1) as usize] += 1;
    }

    /// Remove a previous rating from the summary (for rating updates).
    pub fn remove(&mut self, old_stars: u8) {
        if self.count > 0 {
            self.count -= 1;
            self.total_stars = self.total_stars.saturating_sub(old_stars as u64);
            self.distribution[(old_stars - 1) as usize] =
                self.distribution[(old_stars - 1) as usize].saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_average() {
        let mut s = RatingSummary::default();
        s.add(5);
        s.add(3);
        s.add(4);
        assert!((s.average() - 4.0).abs() < 1e-9);
        assert_eq!(s.count, 3);
    }

    #[test]
    fn invalid_stars_rejected() {
        assert!(RatingRecord::new("0xabc".into(), 0, None, 1).is_err());
        assert!(RatingRecord::new("0xabc".into(), 6, None, 1).is_err());
    }

    #[test]
    fn valid_stars_accepted() {
        for s in 1u8..=5 {
            assert!(RatingRecord::new("0xabc".into(), s, None, 1).is_ok());
        }
    }

    #[test]
    fn remove_rating_does_not_underflow() {
        let mut s = RatingSummary::default();
        s.remove(5); // no-op on empty
        assert_eq!(s.count, 0);
    }
}
