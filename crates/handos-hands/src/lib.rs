//! Stable hand roles and conditioned landmarks. Both capture and wall clocks are supplied by the caller.

pub mod features;
pub mod identity;
pub mod one_euro;
pub mod outlier;
pub mod predict;
pub mod quality;

use handos_proto::{Hand, HandFrame};
pub use identity::Role;
pub use quality::Quality;

use crate::{
    identity::Identity,
    one_euro::{OneEuro, Params},
    outlier::Outlier,
    quality::{MIN_PRESENCE, quality},
};

pub type Landmarks = [[f32; 3]; 21];

#[derive(Clone, Debug)]
pub struct HandState {
    pub role: Role,
    pub track_id: u32,
    pub quality: Quality,
    /// Filtered image points in mirror space.
    pub image: Landmarks,
    /// Filtered world points stay in the original camera coordinates.
    pub world: Option<Landmarks>,
    pub velocity: [[f32; 2]; 21],
    pub t_capture_us: u64,
}

#[derive(Default, Debug)]
pub struct Hands {
    pub left: Option<HandState>,
    pub right: Option<HandState>,
}

impl Hands {
    pub fn get(&self, role: Role) -> Option<&HandState> {
        match role {
            Role::Left => self.left.as_ref(),
            Role::Right => self.right.as_ref(),
        }
    }
}

struct Slot {
    last_accepted_us: u64,
    image: OneEuro<63>,
    world: OneEuro<63>,
    outlier: Outlier,
    hand: HandState,
}

impl Slot {
    fn new(role: Role, track_id: u32, image: Params, world: Params) -> Self {
        Self {
            last_accepted_us: 0,
            image: OneEuro::new(image),
            world: OneEuro::new(world),
            outlier: Outlier::default(),
            hand: HandState {
                role,
                track_id,
                quality: Quality::Lost,
                image: [[0.0; 3]; 21],
                world: None,
                velocity: [[0.0; 2]; 21],
                t_capture_us: 0,
            },
        }
    }

    fn update(
        &mut self,
        sample: Option<&Hand>,
        capture_us: u64,
        now_us: u64,
        world_params: Params,
    ) {
        let accepted = sample.filter(|hand| self.outlier.accept(&hand.landmarks));
        if let Some(sample) = accepted {
            let image = sample.landmarks.map(|[x, y, z]| [1.0 - x, y, z]);
            let raw = std::array::from_fn(|j| image[j / 3][j % 3]);
            let image = self.image.update(raw, capture_us);
            self.hand.image =
                std::array::from_fn(|i| [image[3 * i], image[3 * i + 1], image[3 * i + 2]]);
            self.hand.velocity = std::array::from_fn(|i| {
                let v = self.image.velocity();
                [v[3 * i], v[3 * i + 1]]
            });
            self.hand.world = sample.world.map(|points| {
                let raw = std::array::from_fn(|j| points[j / 3][j % 3]);
                let w = self.world.update(raw, capture_us);
                std::array::from_fn(|i| [w[3 * i], w[3 * i + 1], w[3 * i + 2]])
            });
            if sample.world.is_none() {
                self.world = OneEuro::new(world_params);
            }
            self.hand.t_capture_us = capture_us;
            self.last_accepted_us = now_us;
        }
        let last = (self.hand.quality != Quality::Lost || accepted.is_some())
            .then_some(self.last_accepted_us);
        self.hand.quality = quality(accepted.is_some(), last, now_us);
    }
}

pub struct Conditioner {
    identity: Identity,
    slots: [Option<Slot>; 2],
    hands: Hands,
    image: Params,
    world: Params,
}

impl Default for Conditioner {
    fn default() -> Self {
        Self::new(one_euro::IMAGE, one_euro::WORLD)
    }
}

impl Conditioner {
    pub fn new(image: Params, world: Params) -> Self {
        Self {
            identity: Identity::default(),
            slots: [None, None],
            hands: Hands::default(),
            image,
            world,
        }
    }

    pub fn update(&mut self, frame: &HandFrame, now_us: u64) -> &Hands {
        let ids = self.identity.update(&frame.hands, now_us);
        for (index, output) in [&mut self.hands.left, &mut self.hands.right]
            .into_iter()
            .enumerate()
        {
            let slot = &mut self.slots[index];
            let Some(id) = ids[index] else {
                *slot = None;
                *output = None;
                continue;
            };
            let role = [Role::Left, Role::Right][index];
            let sample = frame
                .hands
                .iter()
                .find(|hand| hand.track_id == id && hand.presence >= MIN_PRESENCE);
            if slot.as_ref().is_none_or(|slot| slot.hand.track_id != id)
                || (sample.is_some()
                    && slot
                        .as_ref()
                        .is_some_and(|slot| slot.hand.quality == Quality::Lost))
            {
                *slot = Some(Slot::new(role, id, self.image, self.world));
            }
            if let Some(slot) = slot {
                slot.update(sample, frame.t_capture_us, now_us, self.world);
                *output = (slot.hand.quality != Quality::Lost).then(|| slot.hand.clone());
            }
        }
        &self.hands
    }
}
