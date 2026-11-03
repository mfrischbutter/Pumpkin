//! The dragon's body in flight: how it steers, where its parts go and what they run into.

use pumpkin_data::Block;
use pumpkin_data::damage::DamageType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::part::{BODY, HEAD, NECK, TAIL, WING_1, WING_2};
use super::phase::Phase;
use super::{EnderDragonEntity, HURT_RECENTLY_COOLDOWN, normalized};
use crate::entity::EntityBase;

impl EnderDragonEntity {
    /// Flies a step toward `target`, the part of vanilla's `aiStep` under `getFlyTargetLocation`.
    pub(super) fn fly_toward(&self, target: Vector3<f64>, phase: &dyn Phase) {
        let entity = &self.mob_entity.living_entity.entity;
        let pos = entity.pos.load();
        let (xdd, zdd) = (target.x - pos.x, target.z - pos.z);
        let mut ydd = target.y - pos.y;
        let dist_sq = xdd * xdd + ydd * ydd + zdd * zdd;
        let max = f64::from(phase.fly_speed());
        let horizontal = xdd.hypot(zdd);
        if horizontal > 0.0 {
            ydd = (ydd / horizontal).clamp(-max, max);
        }
        let mut velocity = entity.velocity.load();
        velocity.y += ydd * 0.01;

        let mut yaw = wrap_degrees(entity.yaw.load());
        let aim = normalized(target - pos);
        let (sin, cos) = yaw.to_radians().sin_cos();
        let dir = normalized(Vector3::new(f64::from(sin), velocity.y, -f64::from(cos)));
        let dot = ((dir.dot(&aim) as f32 + 0.5) / 1.5).max(0.0);
        if xdd.abs() > 1.0E-5 || zdd.abs() > 1.0E-5 {
            let delta =
                wrap_degrees(180.0 - xdd.atan2(zdd).to_degrees() as f32 - yaw).clamp(-50.0, 50.0);
            yaw += self.add_yaw_acceleration(delta * phase.turn_speed(self)) * 0.1;
        }
        entity.yaw.store(yaw);

        // `moveRelative` along (0, 0, -1): dragons fly with their tail first in their own frame.
        let span = (2.0 / (dist_sq + 1.0)) as f32;
        let speed = f64::from(0.06 * (dot * span + (1.0 - span)));
        let (sin, cos) = yaw.to_radians().sin_cos();
        velocity.x += speed * f64::from(sin);
        velocity.z -= speed * f64::from(cos);

        let scale = if self.in_wall.load(Ordering::Relaxed) {
            0.8
        } else {
            1.0
        };
        entity.set_pos(pos + velocity.multiply(scale, scale, scale));

        let slide = 0.8 + 0.15 * (normalized(velocity).dot(&dir) + 1.0) / 2.0;
        entity.velocity.store(velocity.multiply(slide, 0.91, slide));
    }

    fn set_part(&self, index: usize, offset: Vector3<f64>) {
        self.parts[index].entity.set_pos(self.position() + offset);
    }

    pub(super) fn part_box(&self, index: usize) -> BoundingBox {
        self.parts[index].entity.bounding_box.load()
    }

    /// Places the parts around the dragon, and what they run into takes the hit on the way.
    pub(super) fn tick_parts(&self) {
        let (p0, p5, p10, tails) = {
            let history = self
                .flight_history
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                history.get(0),
                history.get(5),
                history.get(10),
                [history.get(12), history.get(14), history.get(16)],
            )
        };
        let tilt = ((p5.y - p10.y) as f32 * 10.0).to_radians();
        let (ss_tilt, cc_tilt) = (f64::from(tilt.sin()), f64::from(tilt.cos()));

        let entity = &self.mob_entity.living_entity.entity;
        let yaw = entity.yaw.load();
        let (ss1, cc1) = {
            let (sin, cos) = yaw.to_radians().sin_cos();
            (f64::from(sin), f64::from(cos))
        };
        self.set_part(BODY, Vector3::new(ss1 * 0.5, 0.0, -cc1 * 0.5));
        self.set_part(WING_1, Vector3::new(cc1 * 4.5, 2.0, ss1 * 4.5));
        self.set_part(WING_2, Vector3::new(cc1 * -4.5, 2.0, ss1 * -4.5));

        let hurt_recently = self
            .mob_entity
            .living_entity
            .hurt_cooldown
            .load(Ordering::Relaxed)
            > HURT_RECENTLY_COOLDOWN;
        if !hurt_recently {
            self.knock_back(self.wing_reach(WING_1));
            self.knock_back(self.wing_reach(WING_2));
            self.hurt_touching(self.part_box(HEAD).expand(1.0, 1.0, 1.0));
            self.hurt_touching(self.part_box(NECK).expand(1.0, 1.0, 1.0));
        }

        let accel = *self
            .yaw_rot_accel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (ss2, cc2) = {
            let (sin, cos) = (yaw.to_radians() - accel * 0.01).sin_cos();
            (f64::from(sin), f64::from(cos))
        };
        let y_offset = if self.phase_type().is_sitting() {
            -1.0
        } else {
            p5.y - p0.y
        };
        for (index, length) in [(HEAD, 6.5), (NECK, 5.5)] {
            self.set_part(
                index,
                Vector3::new(
                    ss2 * length * cc_tilt,
                    y_offset + ss_tilt * length,
                    -cc2 * length * cc_tilt,
                ),
            );
        }

        for (i, sample) in tails.iter().enumerate() {
            let rot = yaw.to_radians() + wrap_degrees(sample.y_rot - p5.y_rot).to_radians();
            let (ss, cc) = (f64::from(rot.sin()), f64::from(rot.cos()));
            let dd = (i + 1) as f64 * 2.0;
            self.set_part(
                TAIL + i,
                Vector3::new(
                    -(ss1 * 1.5 + ss * dd) * cc_tilt,
                    sample.y - p5.y - (dd + 1.5) * ss_tilt + 1.5,
                    (cc1 * 1.5 + cc * dd) * cc_tilt,
                ),
            );
        }
    }

    fn wing_reach(&self, wing: usize) -> BoundingBox {
        let reach = self.part_box(wing).expand(4.0, 2.0, 4.0);
        BoundingBox::new(
            reach.min + Vector3::new(0.0, -2.0, 0.0),
            reach.max + Vector3::new(0.0, -2.0, 0.0),
        )
    }

    /// The living things in `area` the dragon hits: not itself, creative or spectating players.
    fn targets_in(&self, area: &BoundingBox) -> Vec<Arc<dyn EntityBase>> {
        let world = self.mob_entity.living_entity.entity.world.load();
        let own_id = self.mob_entity.living_entity.entity.entity_id;
        world
            .get_all_at_box(area)
            .into_iter()
            .filter(|target| {
                target.get_entity().entity_id != own_id
                    && target.get_living_entity().is_some()
                    && target
                        .get_player()
                        .is_none_or(|p| !p.is_creative() && !p.is_spectator())
            })
            .collect()
    }

    /// Vanilla `knockBack`: the wings throw what they reach, and hurt it unless the dragon sits.
    fn knock_back(&self, area: BoundingBox) {
        let body = self.part_box(BODY);
        let (xm, zm) = (
            f64::midpoint(body.min.x, body.max.x),
            f64::midpoint(body.min.z, body.max.z),
        );
        let sitting = self.phase_type().is_sitting();
        for target in self.targets_in(&area) {
            let entity = target.get_entity();
            let pos = entity.pos.load();
            let (xd, zd) = (pos.x - xm, pos.z - zm);
            let dd = (xd * xd + zd * zd).max(0.1);
            let velocity = entity.velocity.load();
            entity
                .velocity
                .store(velocity + Vector3::new(xd / dd * 4.0, 0.2, zd / dd * 4.0));
            if target.get_player().is_some() {
                entity.send_velocity();
            }
            if !sitting && (self as &dyn EntityBase).do_teams_allow_damage(target.as_ref()) {
                target.damage_with_context(
                    target.as_ref(),
                    5.0,
                    DamageType::MOB_ATTACK,
                    None,
                    Some(self),
                    Some(self),
                );
            }
        }
    }

    /// Vanilla `hurt(level, entities)`: the head and neck bite for 10.
    fn hurt_touching(&self, area: BoundingBox) {
        for target in self.targets_in(&area) {
            if (self as &dyn EntityBase).do_teams_allow_damage(target.as_ref()) {
                target.damage_with_context(
                    target.as_ref(),
                    10.0,
                    DamageType::MOB_ATTACK,
                    None,
                    Some(self),
                    Some(self),
                );
            }
        }
    }

    /// Vanilla `checkWalls`: breaks through what it may break, and is slowed by the rest.
    pub(super) fn check_walls(&self, area: &BoundingBox) -> bool {
        let world = self.mob_entity.living_entity.entity.world.load_full();
        let griefing = world.level_info.load().game_rules.mob_griefing;
        let min = area.min_block_pos();
        let max = area.max_block_pos();
        let mut hit_wall = false;
        let mut destroyed = false;
        for pos in BlockPos::iterate(min, max) {
            if world.get_block_state(&pos).is_air() {
                continue;
            }
            let block = world.get_block(&pos);
            if block.has_tag(&tag::Block::MINECRAFT_DRAGON_TRANSPARENT) {
                continue;
            }
            if griefing && !block.has_tag(&tag::Block::MINECRAFT_DRAGON_IMMUNE) {
                world.set_block_state(&pos, Block::AIR.default_state.id, BlockFlags::NOTIFY_ALL);
                destroyed = true;
            } else {
                hit_wall = true;
            }
        }
        if destroyed {
            let at = BlockPos::new(
                rand::random_range(min.0.x..=max.0.x),
                rand::random_range(min.0.y..=max.0.y),
                rand::random_range(min.0.z..=max.0.z),
            );
            world.sync_world_event(WorldEvent::ParticlesDragonBlockBreak, at, 0);
        }
        hit_wall
    }
}
