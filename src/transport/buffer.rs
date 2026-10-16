//! Keep byte credits meaningful: a small shared Bytes slice can otherwise pin
//! an entire WebSocket/reassembly allocation. Use the public conversion to Vec
//! to expose the ORIGINAL allocation capacity (not just capacity after a slice).
use bytes::Bytes;

pub fn retain_bytes(bytes: Bytes) -> Bytes {
    let size = bytes.len();
    // Reuses uniquely-owned storage; shared/static storage is copied to size.
    let owned: Vec<u8> = bytes.into();
    if owned.capacity() > size.saturating_add(128).saturating_mul(2) {
        Bytes::copy_from_slice(&owned)
    } else {
        Bytes::from(owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_and_unique_tiny_tail_slices_do_not_pin_large_allocations() {
        for shared in [false, true] {
            let mut allocation = vec![0; 1024 * 1024];
            allocation[1024 * 1024 - 1] = 42;
            let whole = Bytes::from(allocation);
            let tail = whole.slice(whole.len() - 1..);
            let sibling = if shared {
                Some(whole)
            } else {
                drop(whole);
                None
            };
            let retained = retain_bytes(tail);
            assert_eq!(&retained[..], &[42]);
            let owned: Vec<u8> = retained.into();
            assert_eq!(owned.capacity(), 1);
            drop(sibling);
        }
    }
    #[test]
    fn normal_owned_storage_is_reused_instead_of_unconditionally_copying() {
        let bytes = Bytes::from(vec![7; 1024]);
        let original = bytes.as_ptr();
        let retained = retain_bytes(bytes);
        assert_eq!(retained.as_ptr(), original);
    }
}
