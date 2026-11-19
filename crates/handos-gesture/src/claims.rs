use handos_hands::{Role, features::Finger};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Hand(Role),
    Pinch(Role, Finger),
    Pointer,
}

#[derive(Default)]
pub struct Claims(Vec<(Resource, u8, u8)>);

impl Claims {
    pub fn owner(&self, resource: Resource) -> Option<u8> {
        self.0
            .iter()
            .find(|claim| claim.0 == resource)
            .map(|claim| claim.1)
    }

    /// Claims all resources or none. Returns owners whose entire claim was taken.
    pub fn acquire(&mut self, owner: u8, priority: u8, resources: &[Resource]) -> Option<Vec<u8>> {
        let mut displaced = Vec::new();
        for &(resource, other, current) in &self.0 {
            if other != owner && resources.contains(&resource) {
                if current >= priority {
                    return None;
                }
                if !displaced.contains(&other) {
                    displaced.push(other);
                }
            }
        }
        self.0.retain(|claim| !displaced.contains(&claim.1));
        for &resource in resources {
            if self.owner(resource).is_none() {
                self.0.push((resource, owner, priority));
            }
        }
        Some(displaced)
    }

    pub fn release(&mut self, owner: u8) {
        self.0.retain(|claim| claim.1 != owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn priority_takes_all_resources_from_displaced_owner() {
        let mut claims = Claims::default();
        let hand = Resource::Hand(Role::Right);
        assert_eq!(
            claims.acquire(0, 0, &[hand, Resource::Pointer]),
            Some(vec![])
        );
        assert_eq!(claims.acquire(1, 0, &[Resource::Pointer]), None);
        assert_eq!(claims.acquire(1, 1, &[Resource::Pointer]), Some(vec![0]));
        assert_eq!(claims.owner(hand), None);
    }

    proptest! {
        #[test]
        fn no_duplicate_owners(actions in proptest::collection::vec((any::<bool>(), 0_u8..5, any::<u8>()), 0..100)) {
            let mut claims = Claims::default();
            for (acquire, id, priority) in actions {
                if acquire {
                    let _ = claims.acquire(id, priority, &[Resource::Pointer]);
                } else {
                    claims.release(id);
                }
                prop_assert!(claims.0.len() <= 1);
            }
        }
    }
}
