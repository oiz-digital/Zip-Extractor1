//! Integration tests for zbx-sync — real state machine, no stubs.
//!
//! These tests drive `SyncCoordinator` with an in-memory mock peer so they
//! run entirely without a live network. Each test covers a distinct sync path.

#[cfg(test)]
mod sync_integration {
    use zbx_sync::{
        coordinator::{SyncCoordinator, SyncPeer, FastSyncOutcome, SnapshotMeta},
        error::SyncError,
    };
    use zbx_types::{H256, BlockHeader};
    use std::sync::Arc;

    // ── Mock peer ─────────────────────────────────────────────────────────────

    struct MockPeer {
        headers: Vec<BlockHeader>,
        best_block: u64,
    }

    impl MockPeer {
        fn with_chain_length(n: usize) -> Self {
            let headers = (0..n as u64)
                .map(|i| BlockHeader {
                    number: i,
                    parent_hash: if i == 0 {
                        H256::zero()
                    } else {
                        H256::from_low_u64_be(i - 1)
                    },
                    state_root: H256::from_low_u64_be(i * 100),
                    hash: H256::from_low_u64_be(i),
                    ..Default::default()
                })
                .collect();
            MockPeer { headers, best_block: n as u64 - 1 }
        }
    }

    #[async_trait::async_trait]
    impl SyncPeer for MockPeer {
        fn best_block(&self) -> u64 { self.best_block }

        async fn get_header(&self, number: u64) -> Result<BlockHeader, SyncError> {
            self.headers.get(number as usize)
                .cloned()
                .ok_or(SyncError::HeaderNotFound(number))
        }

        async fn get_snapshot(&self) -> Result<SnapshotMeta, SyncError> {
            let tip = &self.headers[self.best_block as usize];
            Ok(SnapshotMeta {
                block_number: tip.number,
                state_root: tip.state_root,
                chunk_count: 8,
            })
        }

        async fn get_snapshot_chunk(&self, index: usize) -> Result<Vec<u8>, SyncError> {
            // Return deterministic chunk data for testing state reconstruction.
            let chunk: Vec<u8> = (0..64).map(|i| (index as u8 ^ i)).collect();
            Ok(chunk)
        }
    }

    // ── Test 1: Fast sync downloads headers in order ──────────────────────────

    #[test]
    fn fast_sync_downloads_headers_in_order() {
        let peer = Arc::new(MockPeer::with_chain_length(100));
        let mut coordinator = SyncCoordinator::new_for_test();

        let outcome = tokio::runtime::Builder::new_current_thread()
            .build().unwrap()
            .block_on(coordinator.fast_sync(peer.as_ref()))
            .expect("fast sync must not error");

        assert_eq!(outcome.best_block, 99,
            "after fast sync, best block must equal peer tip");

        // Verify headers were downloaded in strictly ascending order.
        let downloaded = coordinator.downloaded_header_numbers();
        for window in downloaded.windows(2) {
            assert!(window[0] < window[1],
                "headers must be imported in ascending order: {:?}", window);
        }
    }

    // ── Test 2: Snap sync reconstructs state root ────────────────────────────

    #[test]
    fn snap_sync_reconstructs_state_root() {
        let chain_len = 50usize;
        let peer = Arc::new(MockPeer::with_chain_length(chain_len));
        let expected_root = peer.headers.last().unwrap().state_root;

        let mut coordinator = SyncCoordinator::new_for_test();

        let reconstructed_root = tokio::runtime::Builder::new_current_thread()
            .build().unwrap()
            .block_on(coordinator.snap_sync(peer.as_ref()))
            .expect("snap sync must not error");

        assert_eq!(
            reconstructed_root, expected_root,
            "reconstructed state root must match archive node's root"
        );
    }

    // ── Test 3: Live sync follows new blocks ──────────────────────────────────

    #[test]
    fn live_sync_follows_new_blocks() {
        let peer = Arc::new(MockPeer::with_chain_length(11));
        let mut coordinator = SyncCoordinator::new_for_test();

        // Start at block 9 (already synced).
        coordinator.set_best_block(9);

        // Live sync imports the next block (block 10).
        tokio::runtime::Builder::new_current_thread()
            .build().unwrap()
            .block_on(coordinator.live_sync_step(peer.as_ref()))
            .expect("live sync step must not error");

        assert_eq!(coordinator.best_block(), 10,
            "live sync must advance best_block to the new tip");
    }

    // ── Test 4: Fork choice prefers longer chain ──────────────────────────────

    #[test]
    fn fork_choice_prefers_longer_chain() {
        // Fork at block 8: Fork A has length 101 (blocks 0..100),
        // Fork B has length 60 (blocks 0..59).
        let fork_a = Arc::new(MockPeer::with_chain_length(101));
        let fork_b = Arc::new(MockPeer::with_chain_length(60));

        let mut coordinator = SyncCoordinator::new_for_test();

        let chosen = coordinator.fork_choice(
            &[fork_a.as_ref(), fork_b.as_ref()]
        );

        assert_eq!(chosen.best_block(), 100,
            "fork choice must select the longest chain (Fork A, tip 100)");
    }
}
