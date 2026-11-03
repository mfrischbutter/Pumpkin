//! The dragon's phases, one for each of vanilla's `EnderDragonPhase`.

use std::sync::Arc;

use pumpkin_util::math::vector3::Vector3;

use super::EnderDragonEntity;
use super::path::{DragonPath, INNER_NODES_START};
use crate::entity::player::Player;

pub mod charging;
pub mod dying;
pub mod holding_pattern;
pub mod hover;
pub mod landing;
pub mod landing_approach;
pub mod sitting_attacking;
pub mod sitting_flaming;
pub mod sitting_scanning;
pub mod strafe_player;
pub mod takeoff;

pub use charging::ChargingPhase;
pub use dying::DyingPhase;
pub use holding_pattern::HoldingPatternPhase;
pub use hover::HoverPhase;
pub use landing::LandingPhase;
pub use landing_approach::LandingApproachPhase;
pub use sitting_attacking::SittingAttackingPhase;
pub use sitting_flaming::SittingFlamingPhase;
pub use sitting_scanning::SittingScanningPhase;
pub use strafe_player::StrafePlayerPhase;
pub use takeoff::TakeoffPhase;

/// Vanilla `AbstractDragonPhaseInstance`.
pub trait Phase: Send + Sync {
    fn tick(&self, dragon: &EnderDragonEntity);

    fn begin(&self) {}

    fn end(&self) {}

    fn fly_speed(&self) -> f32 {
        0.6
    }

    fn turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let rot_speed = dragon.horizontal_speed() + 1.0;
        0.7 / rot_speed.min(40.0) / rot_speed
    }

    fn fly_target(&self) -> Option<Vector3<f64>> {
        None
    }

    fn on_crystal_destroyed(&self, _dragon: &EnderDragonEntity, _player: Option<&Arc<Player>>) {}
}

/// The phases by vanilla's ids, which clients read and worlds save.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnderDragonPhase {
    HoldingPattern = 0,
    StrafePlayer = 1,
    LandingApproach = 2,
    Landing = 3,
    Takeoff = 4,
    SittingFlaming = 5,
    SittingScanning = 6,
    SittingAttacking = 7,
    ChargingPlayer = 8,
    Dying = 9,
    #[default]
    Hovering = 10,
}

impl EnderDragonPhase {
    const ALL: [Self; 11] = [
        Self::HoldingPattern,
        Self::StrafePlayer,
        Self::LandingApproach,
        Self::Landing,
        Self::Takeoff,
        Self::SittingFlaming,
        Self::SittingScanning,
        Self::SittingAttacking,
        Self::ChargingPlayer,
        Self::Dying,
        Self::Hovering,
    ];

    /// Vanilla `isSitting`, which also holds for hovering.
    #[must_use]
    pub const fn is_sitting(self) -> bool {
        matches!(
            self,
            Self::SittingFlaming | Self::SittingScanning | Self::SittingAttacking | Self::Hovering
        )
    }

    /// Vanilla `AbstractDragonSittingPhase`: these burn the arrows and wind charges they catch.
    #[must_use]
    pub const fn stops_projectiles(self) -> bool {
        matches!(
            self,
            Self::SittingFlaming | Self::SittingScanning | Self::SittingAttacking
        )
    }

    #[must_use]
    pub const fn network_id(self) -> i32 {
        self as i32
    }

    /// Vanilla `getById`: what is out of range is holding the pattern.
    #[must_use]
    pub fn from_id(id: i32) -> Self {
        usize::try_from(id)
            .ok()
            .and_then(|id| Self::ALL.get(id).copied())
            .unwrap_or(Self::HoldingPattern)
    }
}

#[derive(Default)]
pub struct PhaseManager {
    holding_pattern: HoldingPatternPhase,
    strafe_player: StrafePlayerPhase,
    landing_approach: LandingApproachPhase,
    landing: LandingPhase,
    takeoff: TakeoffPhase,
    sitting_flaming: SittingFlamingPhase,
    sitting_scanning: SittingScanningPhase,
    sitting_attacking: SittingAttackingPhase,
    charging_player: ChargingPhase,
    dying: DyingPhase,
    hovering: HoverPhase,
}

impl PhaseManager {
    #[must_use]
    pub fn get(&self, phase: EnderDragonPhase) -> &dyn Phase {
        match phase {
            EnderDragonPhase::HoldingPattern => &self.holding_pattern,
            EnderDragonPhase::StrafePlayer => &self.strafe_player,
            EnderDragonPhase::LandingApproach => &self.landing_approach,
            EnderDragonPhase::Landing => &self.landing,
            EnderDragonPhase::Takeoff => &self.takeoff,
            EnderDragonPhase::SittingFlaming => &self.sitting_flaming,
            EnderDragonPhase::SittingScanning => &self.sitting_scanning,
            EnderDragonPhase::SittingAttacking => &self.sitting_attacking,
            EnderDragonPhase::ChargingPlayer => &self.charging_player,
            EnderDragonPhase::Dying => &self.dying,
            EnderDragonPhase::Hovering => &self.hovering,
        }
    }

    pub const fn strafe_player(&self) -> &StrafePlayerPhase {
        &self.strafe_player
    }

    pub const fn charging_player(&self) -> &ChargingPhase {
        &self.charging_player
    }

    pub const fn sitting_flaming(&self) -> &SittingFlamingPhase {
        &self.sitting_flaming
    }
}

/// Vanilla `navigateToNextPathNode`: flies to the path's next node, up to 20 above it.
fn next_path_target(path: &mut DragonPath) -> Option<Vector3<f64>> {
    let node = path.next_pos()?;
    path.advance();
    Some(Vector3::new(
        f64::from(node.x),
        f64::from(node.y) + rand::random::<f64>() * 20.0,
        f64::from(node.z),
    ))
}

/// Keeps a node index on its ring: the outer one, or the middle one by `& 7`.
const fn on_ring(index: i32, outer: bool) -> usize {
    if outer {
        index.rem_euclid(INNER_NODES_START as i32) as usize
    } else {
        (((index - INNER_NODES_START as i32) & 7) + INNER_NODES_START as i32) as usize
    }
}

/// The node to circle to next, shared by holding the pattern and strafing: the one beside it, or
/// one in 8 times a change of direction across the ring.
fn circling_node(current: usize, clockwise: &mut bool) -> i32 {
    let mut target = current as i32;
    if rand::random_range(0..8) == 0 {
        *clockwise = !*clockwise;
        target += 6;
    }
    if *clockwise { target + 1 } else { target - 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_ids_are_vanillas() {
        assert_eq!(EnderDragonPhase::HoldingPattern.network_id(), 0);
        assert_eq!(EnderDragonPhase::SittingScanning.network_id(), 6);
        assert_eq!(EnderDragonPhase::Hovering.network_id(), 10);
        for phase in EnderDragonPhase::ALL {
            assert_eq!(EnderDragonPhase::from_id(phase.network_id()), phase);
        }
        assert_eq!(
            EnderDragonPhase::from_id(99),
            EnderDragonPhase::HoldingPattern
        );
    }

    #[test]
    fn hovering_sits_but_lets_arrows_through() {
        assert!(EnderDragonPhase::Hovering.is_sitting());
        assert!(!EnderDragonPhase::Hovering.stops_projectiles());
        assert!(EnderDragonPhase::SittingAttacking.stops_projectiles());
        assert!(!EnderDragonPhase::Landing.is_sitting());
        assert!(!EnderDragonPhase::Takeoff.is_sitting());
    }

    #[test]
    fn rings_wrap_like_vanillas() {
        assert_eq!(on_ring(-1, true), 11);
        assert_eq!(on_ring(12, true), 0);
        assert_eq!(on_ring(11, false), 19);
        assert_eq!(on_ring(20, false), 12);
    }
}
