//! Vanilla `ServerExplosion.hurtEntities` with the entities' `ignoreExplosion` overrides.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::{
    damage::DamageType,
    entity::{EntityPose, EntityType},
};
use pumpkin_util::math::{boundingbox::BoundingBox, vector3::Vector3};
use rustc_hash::FxHashMap;

use super::{DefaultExplosionDamageCalculator, Explosion, ExplosionDamageCalculator};
use crate::{
    entity::{Entity, EntityBase, projectile::hurting, vehicle::minecart::MinecartEntity},
    net::ClientPlatform,
    world::World,
};

/// Vanilla `Explosion.getIndirectSourceEntity`: the living thing behind the explosion.
pub(super) fn indirect_source(source: Option<&Arc<dyn EntityBase>>) -> Option<Arc<dyn EntityBase>> {
    let source = source?;
    if source.get_living_entity().is_some() {
        return Some(source.clone());
    }
    source
        .get_owner_entity()
        .filter(|owner| owner.get_living_entity().is_some())
}

/// Vanilla `DamageSources.explosion`.
const fn explosion_damage(has_cause: bool) -> DamageType {
    if has_cause {
        DamageType::PLAYER_EXPLOSION
    } else {
        DamageType::EXPLOSION
    }
}

impl Explosion {
    /// Vanilla `Explosion.shouldAffectBlocklikeEntities`.
    fn should_affect_blocklike_entities(&self, world: &World) -> bool {
        !self.source_is(&EntityType::WIND_CHARGE)
            && !self.source_is(&EntityType::BREEZE_WIND_CHARGE)
            && (world.level_info.load().game_rules.mob_griefing
                || matches!(
                    self.block_interaction,
                    super::BlockInteraction::Destroy | super::BlockInteraction::DestroyWithDecay
                ))
    }

    /// Vanilla `Entity.ignoreExplosion` and its overrides.
    fn ignores_entity(
        &self,
        world: &World,
        target: &dyn EntityBase,
        source: Option<&dyn EntityBase>,
    ) -> bool {
        if target
            .cast_any()
            .downcast_ref::<MinecartEntity>()
            .is_some_and(|minecart| minecart.ignores_explosion(source))
        {
            return true;
        }
        let entity = target.get_entity();
        let kind = entity.entity_type;
        if kind == &EntityType::WARDEN {
            return matches!(
                entity.pose.load(),
                EntityPose::Digging | EntityPose::Emerging
            );
        }
        if kind == &EntityType::ITEM {
            return !self.should_affect_blocklike_entities(world);
        }
        target.ignore_explosion(
            self.should_affect_blocklike_entities(world),
            source.is_some_and(|source| source.get_entity().is_in_water()),
        )
    }

    /// Vanilla `ServerExplosion.hurtEntities`. Returns the knockback of every Java player hit,
    /// which vanilla sends in the explosion packet.
    pub(super) fn damage_entities(
        &self,
        world: &Arc<World>,
        source: Option<&Arc<dyn EntityBase>>,
    ) -> FxHashMap<i32, Vector3<f64>> {
        let mut hit_players = FxHashMap::default();
        // Explosion is too small
        if self.power < 1.0e-5 {
            return hit_players;
        }

        let radius = self.power as f64 * 2.0;

        let default_calc = DefaultExplosionDamageCalculator;
        let calc: &dyn ExplosionDamageCalculator = match &self.damage_calculator {
            Some(c) => c.as_ref(),
            None => &default_calc,
        };

        let direct: Option<&dyn EntityBase> = source.map(|source| &**source);
        let cause = indirect_source(source);

        for target in world.get_all_at_box(&self.search_box(radius)) {
            let target: &dyn EntityBase = &*target;
            // Vanilla `getEntities(this.source, ...)` leaves the source out.
            if Some(target.get_entity().entity_id) == self.source_entity_id
                || target.is_spectator()
                || self.ignores_entity(world, target, direct)
            {
                continue;
            }
            let entity = target.get_entity();

            let distance = (entity.pos.load().squared_distance_to_vec(&self.pos)).sqrt() / radius;
            if distance > 1.0 {
                continue;
            }

            let should_damage = calc.should_damage_entity(self, target);
            let knockback_multiplier = calc.get_knockback_multiplier(target) as f64;
            let exposure = if !should_damage && knockback_multiplier == 0.0 {
                0.0
            } else {
                Self::calculate_exposure(&self.pos, entity, world) as f64
            };

            // Vanilla also hurts what the blast does not reach: for one damage, at exposure 0.
            if should_damage && direct.is_none_or(|source| source.do_teams_allow_damage(target)) {
                let damage = calc.get_entity_damage_amount(self, target, exposure as f32);
                target.damage_with_context(
                    target,
                    damage,
                    explosion_damage(cause.is_some()),
                    None,
                    direct,
                    cause.as_deref(),
                );
            }

            let knockback = self.knockback(target, distance, exposure * knockback_multiplier);
            push_from_explosion(target, knockback, &mut hit_players);
            if hurting::is_redirectable(entity)
                && let Some(owner) = target.get_projectile_owner()
            {
                owner.set(cause.as_deref().map(EntityBase::get_entity));
            }
            // Vanilla `ServerPlayer.onExplosionHit`: the launch by a player's wind charge costs
            // no fall damage; a breeze's and every other explosion's does.
            if let Some(player) = target.get_player() {
                player.living_entity.current_impulse.set_ignore_fall_damage(
                    self.source_is(&EntityType::WIND_CHARGE),
                    entity.pos.load().y,
                );
            }
        }
        hit_players
    }

    fn search_box(&self, radius: f64) -> BoundingBox {
        BoundingBox::new(
            Vector3::new(
                (self.pos.x - radius - 1.0).floor(),
                (self.pos.y - radius - 1.0).floor(),
                (self.pos.z - radius - 1.0).floor(),
            ),
            Vector3::new(
                (self.pos.x + radius + 1.0).floor(),
                (self.pos.y + radius + 1.0).floor(),
                (self.pos.z + radius + 1.0).floor(),
            ),
        )
    }

    /// The knockback of vanilla `hurtEntities`; the attribute's range is 0 to 1.
    fn knockback(&self, target: &dyn EntityBase, distance: f64, strength: f64) -> Vector3<f64> {
        let entity = target.get_entity();
        let dir_pos = if entity.entity_type == &EntityType::TNT {
            entity.pos.load()
        } else {
            entity.get_eye_pos()
        };
        let direction = (dir_pos - self.pos).normalize();
        let knockback_resistance = target.get_living_entity().map_or(0.0, |living| {
            living
                .get_attribute_value(
                    &pumpkin_data::attributes::Attributes::EXPLOSION_KNOCKBACK_RESISTANCE,
                )
                .clamp(0.0, 1.0)
        });
        direction * ((1.0 - distance) * strength * (1.0 - knockback_resistance))
    }

    fn calculate_exposure(
        explosion_pos: &Vector3<f64>,
        entity: &Entity,
        world: &Arc<World>,
    ) -> f32 {
        let bbox = entity.bounding_box.load();

        let step_x = 1.0 / ((bbox.max.x - bbox.min.x) * 2.0 + 1.0);
        let step_y = 1.0 / ((bbox.max.y - bbox.min.y) * 2.0 + 1.0);
        let step_z = 1.0 / ((bbox.max.z - bbox.min.z) * 2.0 + 1.0);

        if step_x < 0.0 || step_y < 0.0 || step_z < 0.0 {
            return 0.0;
        }

        let offset_x = (1.0 - (1.0 / step_x).floor() * step_x) / 2.0;
        let offset_z = (1.0 - (1.0 / step_z).floor() * step_z) / 2.0;

        let mut visible_points = 0;
        let mut total_points = 0;

        let mut k = 0.0;
        while k <= 1.0 {
            let mut l = 0.0;
            while l <= 1.0 {
                let mut m = 0.0;
                while m <= 1.0 {
                    let n = bbox.min.x + (bbox.max.x - bbox.min.x) * k;
                    let o = bbox.min.y + (bbox.max.y - bbox.min.y) * l;
                    let p = bbox.min.z + (bbox.max.z - bbox.min.z) * m;

                    let vec3d = Vector3::new(n + offset_x, o, p + offset_z);

                    if world
                        .clip(&pumpkin_world::clip::ClipContext::new(
                            vec3d,
                            *explosion_pos,
                            pumpkin_world::clip::ClipBlock::Collider,
                            pumpkin_world::clip::ClipFluid::None,
                        ))
                        .is_miss
                    {
                        visible_points += 1;
                    }

                    total_points += 1;
                    m += step_z;
                }
                l += step_y;
            }
            k += step_x;
        }

        if total_points == 0 {
            return 0.0;
        }

        visible_points as f32 / total_points as f32
    }
}

/// Vanilla `Entity.pushFromExplosion`. A Java player learns of it from the explosion packet, so
/// that it adds to what the client knows; `AbstractWindCharge.push` does nothing.
fn push_from_explosion(
    target: &dyn EntityBase,
    knockback: Vector3<f64>,
    hit_players: &mut FxHashMap<i32, Vector3<f64>>,
) {
    let entity = target.get_entity();
    let kind = entity.entity_type;
    if kind == &EntityType::WIND_CHARGE || kind == &EntityType::BREEZE_WIND_CHARGE {
        return;
    }
    if let Some(player) = target.get_player() {
        if player.is_creative() && player.is_flying() {
            return;
        }
        if matches!(player.client.as_ref(), ClientPlatform::Java(_)) {
            entity.velocity.store(entity.velocity.load() + knockback);
            hit_players.insert(entity.entity_id, knockback);
            return;
        }
    }
    if knockback.length_squared() > 0.0 {
        target.push_from_explosion(knockback);
    }
}
